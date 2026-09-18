//! `grep` —— 在指定路径（默认工作目录）用正则搜索文件内容，返回匹配行（file:line:text）。regex + walkdir。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};
use walkdir::WalkDir;

use crate::paths;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("grep", "在指定路径（默认工作目录）用正则搜索文件内容，返回匹配行（file:line:text）")
            .schema(json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "搜索路径，支持绝对路径；相对路径基于工作目录，默认 '.'" },
                    "pattern": { "type": "string", "description": "正则表达式" },
                    "glob": { "type": "string", "description": "可选，仅搜索匹配此 glob 的文件，如 **/*.rs" },
                    "ignore_case": { "type": "boolean", "default": false },
                    "max_results": { "type": "integer", "default": 200 }
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
            return Ok(ToolResult::err("grep: 'pattern' is required"));
        };
        let ignore_case = input.get("ignore_case").and_then(|v| v.as_bool()).unwrap_or(false);
        let max = input.get("max_results").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
        let file_glob = input
            .get("glob")
            .and_then(|v| v.as_str())
            .and_then(|g| glob::Pattern::new(g).ok());
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        let root = match paths::resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("grep: {e}"))),
        };
        let re = match regex::RegexBuilder::new(pattern).case_insensitive(ignore_case).build() {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("grep: 非法正则 {e}"))),
        };
        let gopts = glob::MatchOptions::new();
        let mut matches = Vec::new();
        let mut truncated = false;
        'outer: for entry in WalkDir::new(&root).follow_links(true).into_iter().flatten() {
            if !entry.file_type().is_file() {
                continue;
            }
            let rel = match entry.path().strip_prefix(&root) {
                Ok(r) => r,
                Err(_) => continue,
            };
            if let Some(g) = &file_glob
                && !g.matches_path_with(rel, gopts) {
                    continue;
                }
            // 只读文本文件；二进制/超大跳过（>2MB）。
            let meta = entry.metadata().ok();
            if meta.map(|m| m.len() > 2_000_000).unwrap_or(false) {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(entry.path()) else {
                continue; // 非 UTF-8 视作二进制，跳过
            };
            for (i, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    if matches.len() >= max {
                        truncated = true;
                        break 'outer;
                    }
                    matches.push(json!({
                        "file": rel.to_string_lossy().to_string(),
                        "line": i + 1,
                        "text": line.chars().take(400).collect::<String>(),
                    }));
                }
            }
        }
        Ok(ToolResult::ok(json!({
            "pattern": pattern,
            "count": matches.len(),
            "truncated": truncated,
            "matches": matches,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn setup() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-grep");
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/a.rs"), "fn foo() {}\nfn bar() {}\n").unwrap();
        std::fs::write(root.join("src/b.rs"), "let FOO = 1;\n").unwrap();
        std::fs::write(root.join("note.txt"), "foo here\n").unwrap();
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn finds_regex_matches() {
        let (root, roots) = setup();
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = GrepTool.invoke(json!({"pattern":"fn \\w+"}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["count"], 2); // foo + bar in a.rs
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn glob_filter_and_ignore_case() {
        let (root, roots) = setup();
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = GrepTool
            .invoke(json!({"pattern":"foo","glob":"**/*.rs","ignore_case":true}), &ctx)
            .await
            .unwrap();
        // a.rs 的 foo() + b.rs 的 FOO（忽略大小写）= 2；note.txt 被 glob 排除
        assert_eq!(r.output["count"], 2);
        std::fs::remove_dir_all(&root).ok();
    }
}
