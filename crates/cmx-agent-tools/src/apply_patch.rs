//! `apply_patch` —— 应用 Codex 风格补丁包（Add/Update/Delete File，含 hunk）。需 workspace-write。
//!
//! 格式：
//! ```text
//! *** Begin Patch
//! *** Add File: path
//! +新内容行
//! *** Update File: path
//! @@ 可选段标记
//!  上下文行
//! -删除行
//! +新增行
//! *** Delete File: path
//! *** End Patch
//! ```
//! Update 的 hunk：以 `@@` 分段；行首 ` `=上下文、`-`=删除、`+`=新增。定位「上下文+删除」块并替换为「上下文+新增」块。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::sandbox;

pub struct ApplyPatchTool;

#[derive(Debug)]
enum Op {
    Add { path: String, content: String },
    Delete { path: String },
    Update { path: String, hunks: Vec<Hunk> },
}

#[derive(Debug, Default)]
struct Hunk {
    before: Vec<String>,
    after: Vec<String>,
}

/// 解析补丁文本为操作序列。返回 Err(原因) 表示格式错误。
fn parse_patch(text: &str) -> Result<Vec<Op>, String> {
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    // 跳过 Begin Patch（宽松：允许缺省）
    while i < lines.len() && !lines[i].starts_with("*** ") {
        i += 1;
    }
    if i < lines.len() && lines[i].trim() == "*** Begin Patch" {
        i += 1;
    }
    let mut ops = Vec::new();
    while i < lines.len() {
        let line = lines[i];
        if line.trim() == "*** End Patch" {
            break;
        }
        if let Some(p) = line.strip_prefix("*** Add File: ") {
            i += 1;
            let mut content = Vec::new();
            while i < lines.len() && !lines[i].starts_with("*** ") {
                let l = lines[i];
                content.push(l.strip_prefix('+').unwrap_or(l).to_string());
                i += 1;
            }
            ops.push(Op::Add { path: p.trim().to_string(), content: content.join("\n") });
        } else if let Some(p) = line.strip_prefix("*** Delete File: ") {
            ops.push(Op::Delete { path: p.trim().to_string() });
            i += 1;
        } else if let Some(p) = line.strip_prefix("*** Update File: ") {
            i += 1;
            let mut hunks: Vec<Hunk> = Vec::new();
            let mut cur = Hunk::default();
            let mut started = false;
            while i < lines.len() && !lines[i].starts_with("*** ") {
                let l = lines[i];
                if l.starts_with("@@") {
                    if started && (!cur.before.is_empty() || !cur.after.is_empty()) {
                        hunks.push(std::mem::take(&mut cur));
                    }
                    started = true;
                    i += 1;
                    continue;
                }
                started = true;
                match l.chars().next() {
                    Some('-') => cur.before.push(l[1..].to_string()),
                    Some('+') => cur.after.push(l[1..].to_string()),
                    Some(' ') => {
                        cur.before.push(l[1..].to_string());
                        cur.after.push(l[1..].to_string());
                    }
                    None => {
                        // 空行 = 空上下文
                        cur.before.push(String::new());
                        cur.after.push(String::new());
                    }
                    _ => { /* 忽略未知前缀行 */ }
                }
                i += 1;
            }
            if !cur.before.is_empty() || !cur.after.is_empty() {
                hunks.push(cur);
            }
            if hunks.is_empty() {
                return Err(format!("Update File {p} 无有效 hunk"));
            }
            ops.push(Op::Update { path: p.trim().to_string(), hunks });
        } else {
            // 未识别的 *** 行或杂散行，跳过
            i += 1;
        }
    }
    if ops.is_empty() {
        return Err("补丁为空或格式不可识别".into());
    }
    Ok(ops)
}

/// 对单个文件内容应用一组 hunk（纯函数，可测）。
fn apply_hunks(content: &str, hunks: &[Hunk]) -> Result<String, String> {
    let mut cur = content.to_string();
    for (hi, h) in hunks.iter().enumerate() {
        let before = h.before.join("\n");
        let after = h.after.join("\n");
        if before.is_empty() {
            // 纯新增且无上下文：追加到末尾
            if !cur.ends_with('\n') && !cur.is_empty() {
                cur.push('\n');
            }
            cur.push_str(&after);
            continue;
        }
        match cur.find(&before) {
            Some(pos) => {
                let mut next = String::with_capacity(cur.len() + after.len());
                next.push_str(&cur[..pos]);
                next.push_str(&after);
                next.push_str(&cur[pos + before.len()..]);
                cur = next;
            }
            None => {
                return Err(format!("hunk #{} 的上下文未在文件中找到", hi + 1));
            }
        }
    }
    Ok(cur)
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "apply_patch",
            "应用 Codex 风格补丁包（Add/Update/Delete File，含 hunk），一次可改多文件",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "patch": { "type": "string", "description": "*** Begin Patch ... *** End Patch 文本" }
            },
            "required": ["patch"]
        }))
        .guard(GuardHints {
            requires_auth: Some("fs:write".into()),
            idempotent: false,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !ctx.sandbox.allows_write() {
            return Ok(ToolResult::err("apply_patch: 当前沙箱为只读（需 workspace-write）"));
        }
        let Some(patch) = input.get("patch").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("apply_patch: 'patch' is required"));
        };
        let ops = match parse_patch(patch) {
            Ok(o) => o,
            Err(e) => return Ok(ToolResult::err(format!("apply_patch: {e}"))),
        };
        // 先全部校验路径在沙箱内（任一越界则整体拒绝，避免半应用）。
        for op in &ops {
            let p = match op {
                Op::Add { path, .. } | Op::Delete { path } | Op::Update { path, .. } => path,
            };
            if let Err(e) = sandbox::resolve(p, ctx) {
                return Ok(ToolResult::err(format!("apply_patch: {e}")));
            }
        }
        let mut changed = Vec::new();
        for op in &ops {
            match op {
                Op::Add { path, content } => {
                    let target = sandbox::resolve(path, ctx).unwrap();
                    if let Some(parent) = target.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if let Err(e) = std::fs::write(&target, content.as_bytes()) {
                        return Ok(ToolResult::err(format!("apply_patch: 写 {path} 失败 {e}")));
                    }
                    changed.push(json!({"op":"add","path":path}));
                }
                Op::Delete { path } => {
                    let target = sandbox::resolve(path, ctx).unwrap();
                    if let Err(e) = std::fs::remove_file(&target) {
                        return Ok(ToolResult::err(format!("apply_patch: 删 {path} 失败 {e}")));
                    }
                    changed.push(json!({"op":"delete","path":path}));
                }
                Op::Update { path, hunks } => {
                    let target = sandbox::resolve(path, ctx).unwrap();
                    let content = match std::fs::read_to_string(&target) {
                        Ok(c) => c,
                        Err(e) => return Ok(ToolResult::err(format!("apply_patch: 读 {path} 失败 {e}"))),
                    };
                    match apply_hunks(&content, hunks) {
                        Ok(updated) => {
                            if let Err(e) = std::fs::write(&target, updated.as_bytes()) {
                                return Ok(ToolResult::err(format!("apply_patch: 写 {path} 失败 {e}")));
                            }
                            changed.push(json!({"op":"update","path":path,"hunks":hunks.len()}));
                        }
                        Err(e) => return Ok(ToolResult::err(format!("apply_patch: {path}: {e}"))),
                    }
                }
            }
        }
        Ok(ToolResult::ok(json!({"changed": changed, "count": changed.len()})))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn tmp() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-patch");
        (root.clone(), vec![root])
    }

    #[test]
    fn apply_hunks_replaces_block() {
        let content = "line1\nline2\nline3\n";
        let h = Hunk {
            before: vec!["line2".into()],
            after: vec!["LINE2".into(), "line2.5".into()],
        };
        let out = apply_hunks(content, &[h]).unwrap();
        assert_eq!(out, "line1\nLINE2\nline2.5\nline3\n");
    }

    #[test]
    fn apply_hunks_missing_context_errors() {
        let h = Hunk { before: vec!["nope".into()], after: vec!["x".into()] };
        assert!(apply_hunks("a\nb\n", &[h]).is_err());
    }

    #[tokio::test]
    async fn add_update_delete_flow() {
        let (root, roots) = tmp();
        std::fs::write(root.join("upd.txt"), "keep\nold\ntail\n").unwrap();
        std::fs::write(root.join("del.txt"), "bye").unwrap();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let patch = "*** Begin Patch\n\
*** Add File: new/added.txt\n\
+hello\n\
+world\n\
*** Update File: upd.txt\n\
@@\n\
 keep\n\
-old\n\
+new\n\
 tail\n\
*** Delete File: del.txt\n\
*** End Patch\n";
        let r = ApplyPatchTool.invoke(json!({"patch":patch}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["count"], 3);
        assert_eq!(std::fs::read_to_string(root.join("new/added.txt")).unwrap(), "hello\nworld");
        assert_eq!(std::fs::read_to_string(root.join("upd.txt")).unwrap(), "keep\nnew\ntail\n");
        assert!(!root.join("del.txt").exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn escape_denied_whole_patch() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let patch = "*** Begin Patch\n*** Add File: ../evil.txt\n+x\n*** End Patch\n";
        let r = ApplyPatchTool.invoke(json!({"patch":patch}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}
