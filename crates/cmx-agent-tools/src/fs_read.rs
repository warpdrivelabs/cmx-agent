//! `fs_read` —— 读取工作区内某文本文件的前若干字节。
//!
//! 路径经 [`crate::sandbox::resolve`] 统一解析（与 fs_write/fs_edit 同一套）：相对路径按工作根
//! （`allowed_roots[0]`）解析，绝对路径校验落在根内；`../` 逃逸被拒。needs `requires_auth = "fs:read"`。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::sandbox;

pub struct FsReadTool;

#[async_trait]
impl Tool for FsReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "fs_read",
            "读取工作区内某文本文件的前 max_bytes 字节（相对路径按工作根解析）",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "工作区内文件路径（相对工作根或绝对路径）" },
                "max_bytes": { "type": "integer", "default": 4096 }
            },
            "required": ["path"]
        }))
        .guard(GuardHints {
            requires_auth: Some("fs:read".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("fs_read: 'path' is required"));
        };
        let max_bytes = input
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(4096) as usize;

        // 与 fs_write/fs_edit 同一套解析：相对路径挂到工作根，绝对路径校验在根内。
        let abs = match sandbox::resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("fs_read: {e}"))),
        };

        match std::fs::read(&abs) {
            Ok(bytes) => {
                let n = bytes.len().min(max_bytes);
                let text = String::from_utf8_lossy(&bytes[..n]).to_string();
                Ok(ToolResult::ok(json!({
                    "path": abs.display().to_string(),
                    "bytes": n,
                    "total_bytes": bytes.len(),
                    "truncated": bytes.len() > n,
                    "text": text,
                })))
            }
            Err(e) => Ok(ToolResult::err(format!("fs_read: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn setup(name: &str, content: &str) -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-fsr");
        std::fs::write(root.join(name), content).unwrap();
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn reads_relative_path_against_workspace_root() {
        // 回归：相对路径「notes.md」应按工作根解析（此前误按进程 CWD → 逃逸拒绝）。
        let (root, roots) = setup("notes.md", "hello office");
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots, session_id: "test" };
        let r = FsReadTool.invoke(json!({"path":"notes.md"}), &ctx).await.unwrap();
        assert!(r.ok, "相对路径应被接受: {r:?}");
        assert_eq!(r.output["text"], "hello office");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn reads_absolute_path_within_root() {
        let (root, roots) = setup("a.txt", "abc");
        let abs = root.join("a.txt");
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots, session_id: "test" };
        let r = FsReadTool.invoke(json!({"path": abs.display().to_string()}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["text"], "abc");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn truncates_to_max_bytes() {
        let (root, roots) = setup("big.txt", "0123456789");
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots, session_id: "test" };
        let r = FsReadTool.invoke(json!({"path":"big.txt","max_bytes":4}), &ctx).await.unwrap();
        assert_eq!(r.output["text"], "0123");
        assert_eq!(r.output["truncated"], true);
        assert_eq!(r.output["total_bytes"], 10);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn dotdot_escape_denied() {
        let (root, roots) = setup("f.txt", "x");
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots, session_id: "test" };
        let r = FsReadTool.invoke(json!({"path":"../../../etc/passwd"}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn no_roots_denies() {
        let roots: Vec<PathBuf> = vec![];
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots, session_id: "test" };
        let r = FsReadTool.invoke(json!({"path":"x"}), &ctx).await.unwrap();
        assert!(!r.ok);
    }
}
