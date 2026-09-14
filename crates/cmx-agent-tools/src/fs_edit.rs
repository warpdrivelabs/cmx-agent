//! `fs_edit` —— 对沙箱内文件做精确字符串替换（对齐 Claude Code Edit 语义）。
//! 默认要求 `old_string` 唯一命中；`replace_all=true` 时替换全部。需 workspace-write 沙箱。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::sandbox;

pub struct FsEditTool;

#[async_trait]
impl Tool for FsEditTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "fs_edit",
            "对工作区内文件做精确字符串替换；默认要求 old_string 唯一命中，replace_all 可替换全部",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "old_string": { "type": "string" },
                "new_string": { "type": "string" },
                "replace_all": { "type": "boolean", "default": false }
            },
            "required": ["path", "old_string", "new_string"]
        }))
        .guard(GuardHints {
            requires_auth: Some("fs:write".into()),
            idempotent: false,
            writes: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !ctx.sandbox.allows_write() {
            return Ok(ToolResult::err("fs_edit: 当前沙箱为只读（需 workspace-write）"));
        }
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("fs_edit: 'path' is required"));
        };
        let old = input.get("old_string").and_then(|v| v.as_str()).unwrap_or("");
        let new = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
        let replace_all = input
            .get("replace_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if old.is_empty() {
            return Ok(ToolResult::err("fs_edit: 'old_string' 不能为空"));
        }
        if old == new {
            return Ok(ToolResult::err("fs_edit: old_string 与 new_string 相同"));
        }
        let target = match sandbox::resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("fs_edit: {e}"))),
        };
        let content = match std::fs::read_to_string(&target) {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::err(format!("fs_edit: 读取失败 {e}"))),
        };
        let count = content.matches(old).count();
        if count == 0 {
            return Ok(ToolResult::err("fs_edit: 未找到 old_string"));
        }
        if count > 1 && !replace_all {
            return Ok(ToolResult::err(format!(
                "fs_edit: old_string 命中 {count} 处（不唯一）；请扩大上下文或用 replace_all"
            )));
        }
        let updated = if replace_all {
            content.replace(old, new)
        } else {
            content.replacen(old, new, 1)
        };
        match std::fs::write(&target, updated.as_bytes()) {
            Ok(()) => Ok(ToolResult::ok(json!({
                "path": target.display().to_string(),
                "replaced": if replace_all { count } else { 1 },
            }))),
            Err(e) => Ok(ToolResult::err(format!("fs_edit: 写入失败 {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn setup(content: &str) -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-fse");
        std::fs::write(root.join("f.txt"), content).unwrap();
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn unique_replace_ok() {
        let (root, roots) = setup("hello world");
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
        let r = FsEditTool
            .invoke(json!({"path":"f.txt","old_string":"world","new_string":"cmx"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "hello cmx");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn ambiguous_denied_without_replace_all() {
        let (root, roots) = setup("a a a");
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
        let r = FsEditTool
            .invoke(json!({"path":"f.txt","old_string":"a","new_string":"b"}), &ctx)
            .await
            .unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn replace_all_ok() {
        let (root, roots) = setup("a a a");
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
        let r = FsEditTool
            .invoke(json!({"path":"f.txt","old_string":"a","new_string":"b","replace_all":true}), &ctx)
            .await
            .unwrap();
        assert!(r.ok);
        assert_eq!(std::fs::read_to_string(root.join("f.txt")).unwrap(), "b b b");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn not_found_errors() {
        let (root, roots) = setup("xyz");
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
        let r = FsEditTool
            .invoke(json!({"path":"f.txt","old_string":"nope","new_string":"b"}), &ctx)
            .await
            .unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}
