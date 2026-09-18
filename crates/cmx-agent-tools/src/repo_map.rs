//! `repo_map` —— 生成工作区的目录树 + 统计（给模型建立仓库全局观）。只读；跳过常见忽略目录。

use std::collections::BTreeMap;

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::paths;

pub struct RepoMapTool;

const IGNORE_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", ".venv", "venv", "__pycache__",
    ".idea", ".vscode", ".cargo", "vendor", ".next", ".turbo",
];

fn ignored(name: &str) -> bool {
    IGNORE_DIRS.contains(&name)
}

#[async_trait]
impl Tool for RepoMapTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "repo_map",
            "生成工作区目录树 + 文件统计（按扩展名），给模型建立仓库全局观",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "目标目录，支持绝对路径；相对路径基于工作目录，默认 '.'" },
                "include_ignored": { "type": "boolean", "default": false, "description": "展示 .git、target 等默认省略的目录" },
                "max_depth": { "type": "integer", "default": 3 },
                "max_entries": { "type": "integer", "default": 400 }
            }
        }))
        .guard(GuardHints {
            requires_auth: Some("fs:read".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let max_depth = input.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(3) as usize;
        let max_entries = input.get("max_entries").and_then(|v| v.as_u64()).unwrap_or(400) as usize;
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = match paths::resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("repo_map: {e}"))),
        };
        let include_ignored = input.get("include_ignored").and_then(|v| v.as_bool()).unwrap_or(false);

        let mut lines: Vec<String> = Vec::new();
        let mut by_ext: BTreeMap<String, u64> = BTreeMap::new();
        let mut total_files = 0u64;
        let mut truncated = false;

        let walker = WalkDir::new(&root)
            .follow_links(true)
            .min_depth(1)
            .max_depth(max_depth)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                // 仅是摘要降噪选项，调用方可显式展示全部目录。
                include_ignored || !(e.file_type().is_dir()
                    && e.file_name().to_str().map(ignored).unwrap_or(false))
            });

        for entry in walker.flatten() {
            let depth = entry.depth();
            let name = entry.file_name().to_string_lossy().to_string();
            let is_dir = entry.file_type().is_dir();
            if lines.len() < max_entries {
                let indent = "  ".repeat(depth.saturating_sub(1));
                lines.push(format!("{indent}{}{}", name, if is_dir { "/" } else { "" }));
            } else {
                truncated = true;
            }
            if !is_dir {
                total_files += 1;
                let ext = entry
                    .path()
                    .extension()
                    .and_then(|e| e.to_str())
                    .unwrap_or("(no-ext)")
                    .to_string();
                *by_ext.entry(ext).or_insert(0) += 1;
            }
        }

        // 扩展名统计取 top（BTreeMap 已排序 by key；这里给出全量，前端可再排）
        let ext_stats: Vec<Value> = by_ext
            .iter()
            .map(|(k, v)| json!({"ext": k, "count": v}))
            .collect();

        Ok(ToolResult::ok(json!({
            "root": root.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(),
            "total_files": total_files,
            "max_depth": max_depth,
            "truncated": truncated,
            "tree": lines.join("\n"),
            "by_ext": ext_stats,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-rm");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::create_dir_all(root.join("target/debug")).unwrap(); // 应被忽略
        std::fs::write(root.join("src/main.rs"), "").unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(root.join("Cargo.toml"), "").unwrap();
        std::fs::write(root.join("target/debug/junk"), "").unwrap();
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn maps_tree_and_ignores_target() {
        let (root, roots) = setup();
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = RepoMapTool.invoke(json!({}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        let tree = r.output["tree"].as_str().unwrap();
        assert!(tree.contains("main.rs"));
        assert!(tree.contains("Cargo.toml"));
        assert!(!tree.contains("junk"), "target/ 应被忽略");
        assert_eq!(r.output["total_files"], 3); // main.rs, lib.rs, Cargo.toml
        std::fs::remove_dir_all(&root).ok();
    }
}
