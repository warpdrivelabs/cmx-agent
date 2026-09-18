//! `git` —— 在工作目录执行版本控制（保留受控子命令白名单）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{paths, proc};

pub struct GitTool;

/// 查询类子命令。
const READ_SUBS: &[&str] = &[
    "status", "diff", "log", "show", "branch", "ls-files", "rev-parse", "blame", "remote",
];
/// 写类子命令。
const WRITE_SUBS: &[&str] = &["add", "commit", "checkout", "restore", "switch", "stash", "init"];

#[async_trait]
impl Tool for GitTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "git",
            "工作区内的 git（受控子命令）：status/diff/log/show/branch/add/commit 等",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "subcommand": { "type": "string", "description": "如 status/diff/log/add/commit" },
                "args": { "type": "array", "items": {"type":"string"}, "description": "子命令参数" },
                "timeout_ms": { "type": "integer", "default": 30000 }
            },
            "required": ["subcommand"]
        }))
        .guard(GuardHints {
            requires_auth: Some("exec".into()),
            idempotent: false,
            // 写子命令（add/commit/checkout 等）执行前需审批；读子命令（status/diff/log）放行。
            approval_arg_values: Some((
                "subcommand".into(),
                WRITE_SUBS.iter().map(|s| s.to_string()).collect(),
            )),
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(sub) = input.get("subcommand").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("git: 'subcommand' is required"));
        };
        let is_read = READ_SUBS.contains(&sub);
        let is_write = WRITE_SUBS.contains(&sub);
        if !is_read && !is_write {
            return Ok(ToolResult::err(format!(
                "git: 子命令 '{sub}' 不在白名单（读:{READ_SUBS:?} 写:{WRITE_SUBS:?}）"
            )));
        }
        let cwd = match paths::working_dir(ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("git: {e}"))),
        };
        let mut args: Vec<String> = vec![sub.to_string()];
        if let Some(a) = input.get("args").and_then(|v| v.as_array()) {
            for x in a {
                if let Some(s) = x.as_str() {
                    args.push(s.to_string());
                }
            }
        }
        let timeout = input
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(proc::DEFAULT_TIMEOUT_MS);
        let out = proc::run("git", &args, &cwd, timeout).await;
        Ok(ToolResult::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    async fn init_repo() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-git");
        let roots = vec![root.clone()];
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        GitTool.invoke(json!({"subcommand":"init"}), &ctx).await.unwrap();
        // 配置身份，避免 commit 报错（局部配置）
        proc::run("git", &["config".into(),"user.email".into(),"t@t".into()], &root, 5000).await;
        proc::run("git", &["config".into(),"user.name".into(),"t".into()], &root, 5000).await;
        (root, roots)
    }

    #[tokio::test]
    async fn status_on_empty_repo() {
        let (root, roots) = init_repo().await;
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = GitTool.invoke(json!({"subcommand":"status","args":["--short"]}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["exit_code"], 0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn add_commit_then_log() {
        let (root, roots) = init_repo().await;
        std::fs::write(root.join("a.txt"), "hi").unwrap();
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        GitTool.invoke(json!({"subcommand":"add","args":["a.txt"]}), &ctx).await.unwrap();
        let c = GitTool.invoke(json!({"subcommand":"commit","args":["-m","first"]}), &ctx).await.unwrap();
        assert_eq!(c.output["exit_code"], 0, "{c:?}");
        let l = GitTool.invoke(json!({"subcommand":"log","args":["--oneline"]}), &ctx).await.unwrap();
        assert!(l.output["stdout"].as_str().unwrap().contains("first"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn unknown_subcommand_rejected() {
        let (root, roots) = init_repo().await;
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = GitTool.invoke(json!({"subcommand":"push"}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}
