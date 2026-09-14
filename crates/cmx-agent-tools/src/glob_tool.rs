//! `glob` —— 在工作区内按 glob 模式列出文件（如 `**/*.rs`）。结果为相对工作根的路径。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::sandbox;

pub struct GlobTool;

#[async_trait]
impl Tool for GlobTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("glob", "在工作区内按 glob 模式列出文件（如 **/*.rs），返回相对路径")
            .schema(json!({
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "glob，如 **/*.rs、src/*.ts" },
                    "max_results": { "type": "integer", "default": 500 }
                },
                "required": ["pattern"]
            }))
            .guard(GuardHints {
                requires_auth: Some("fs:read".into()),
                idempotent: true,
                ..Default::default()
            })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(pattern) = input.get("pattern").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("glob: 'pattern' is required"));
        };
        let max = input.get("max_results").and_then(|v| v.as_u64()).unwrap_or(500) as usize;
        let Some(root) = sandbox::first_root(ctx) else {
            return Ok(ToolResult::err("glob: no allowed_roots (sandbox denies all fs)"));
        };
        let pat = match glob::Pattern::new(pattern) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("glob: 非法模式 {e}"))),
        };
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: false,
            require_literal_leading_dot: false,
        };
        let mut hits = Vec::new();
        let mut truncated = false;
        for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = match entry.path().strip_prefix(root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if pat.matches_path_with(rel, opts) {
                if hits.len() >= max {
                    truncated = true;
                    break;
                }
                hits.push(rel.to_string_lossy().to_string());
            }
        }
        hits.sort();
        Ok(ToolResult::ok(json!({
            "pattern": pattern,
            "count": hits.len(),
            "truncated": truncated,
            "files": hits,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn setup() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-glob");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main(){}").unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(root.join("README.md"), "").unwrap();
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn matches_rs_files() {
        let (root, roots) = setup();
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots, session_id: "test" };
        let r = GlobTool.invoke(json!({"pattern":"**/*.rs"}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["count"], 2);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn matches_single_dir() {
        let (root, roots) = setup();
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots, session_id: "test" };
        let r = GlobTool.invoke(json!({"pattern":"*.md"}), &ctx).await.unwrap();
        assert_eq!(r.output["count"], 1);
        std::fs::remove_dir_all(&root).ok();
    }
}
