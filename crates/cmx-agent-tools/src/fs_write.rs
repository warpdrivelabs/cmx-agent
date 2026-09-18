//! `fs_write` —— 创建/覆盖一个文本文件（自动建父目录）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::paths;

pub struct FsWriteTool;

#[async_trait]
impl Tool for FsWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "fs_write",
            "创建或覆盖一个文本文件；自动创建父目录；相对路径基于工作目录，绝对路径原样使用",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "绝对路径或相对工作目录的路径" },
                "content": { "type": "string" }
            },
            "required": ["path", "content"]
        }))
        .guard(GuardHints {
            requires_auth: Some("fs:write".into()),
            idempotent: false,
            writes: true,
            write_path_args: vec!["path".into()],
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("fs_write: 'path' is required"));
        };
        let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let target = match paths::resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("fs_write: {e}"))),
        };
        if let Some(parent) = target.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
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

    #[tokio::test]
    async fn writes_file_and_creates_parents() {
        let root = crate::testutil::unique_dir("cmx-fsw");
        let roots = vec![root.clone()];
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        let r = FsWriteTool
            .invoke(json!({"path":"a/b/hi.txt","content":"你好"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("a/b/hi.txt")).unwrap(),
            "你好"
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn writes_outside_workspace_with_relative_and_absolute_paths() {
        let base = crate::testutil::unique_dir("cmx-fsw-outside");
        let roots = vec![base.join("workspace")];
        std::fs::create_dir_all(&roots[0]).unwrap();
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        let absolute = base.join("absolute.txt");
        for path in ["../relative.txt", absolute.to_str().unwrap()] {
            let r = FsWriteTool
                .invoke(json!({"path":path,"content":"outside"}), &ctx)
                .await
                .unwrap();
            assert!(r.ok, "{r:?}");
        }
        assert_eq!(
            std::fs::read_to_string(base.join("relative.txt")).unwrap(),
            "outside"
        );
        assert_eq!(std::fs::read_to_string(absolute).unwrap(), "outside");
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn absolute_write_without_workspace() {
        let base = crate::testutil::unique_dir("cmx-fsw-no-root");
        let file = base.join("new.txt");
        let ctx = ToolCtx {
            workspace_roots: &[],
            session_id: "test",
        };
        let r = FsWriteTool
            .invoke(json!({"path":file,"content":"ok"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(std::fs::read_to_string(file).unwrap(), "ok");
        std::fs::remove_dir_all(base).unwrap();
    }
}
