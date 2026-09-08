//! `fs_write` —— 在沙箱内创建/覆盖一个文本文件（自动建父目录）。需 workspace-write 沙箱。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::sandbox;

pub struct FsWriteTool;

#[async_trait]
impl Tool for FsWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "fs_write",
            "在工作区（allowed_roots）内创建或覆盖一个文本文件；自动创建父目录",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "工作区内路径（相对则基于第一个工作根）" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        }))
        .guard(GuardHints {
            requires_auth: Some("fs:write".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !ctx.sandbox.allows_write() {
            return Ok(ToolResult::err(
                "fs_write: 当前沙箱为只读（需 workspace-write）",
            ));
        }
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("fs_write: 'path' is required"));
        };
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let target = match sandbox::resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("fs_write: {e}"))),
        };
        if let Some(parent) = target.parent()
            && let Err(e) = std::fs::create_dir_all(parent) {
                return Ok(ToolResult::err(format!("fs_write: 建目录失败 {e}")));
            }
        match std::fs::write(&target, content.as_bytes()) {
            Ok(()) => Ok(ToolResult::ok(json!({
                "path": target.display().to_string(),
                "bytes": content.len(),
            }))),
            Err(e) => Ok(ToolResult::err(format!("fs_write: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn tmp() -> PathBuf {
        crate::testutil::unique_dir("cmx-fsw")
    }

    #[tokio::test]
    async fn writes_file_and_creates_parents() {
        let root = tmp();
        let roots = vec![root.clone()];
        let ctx = ToolCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            allowed_roots: &roots,
        };
        let r = FsWriteTool
            .invoke(json!({"path":"a/b/hi.txt","content":"你好"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(std::fs::read_to_string(root.join("a/b/hi.txt")).unwrap(), "你好");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn denied_in_readonly() {
        let root = tmp();
        let roots = vec![root.clone()];
        let ctx = ToolCtx {
            sandbox: SandboxMode::ReadOnly,
            allowed_roots: &roots,
        };
        let r = FsWriteTool
            .invoke(json!({"path":"x.txt","content":"y"}), &ctx)
            .await
            .unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn denied_escape() {
        let root = tmp();
        let roots = vec![root.clone()];
        let ctx = ToolCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            allowed_roots: &roots,
        };
        let r = FsWriteTool
            .invoke(json!({"path":"../evil.txt","content":"y"}), &ctx)
            .await
            .unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}
