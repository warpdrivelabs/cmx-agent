//! `bash` —— 在工作区内执行 shell 命令（cwd=第一个工作根），带超时，回传 exit/stdout/stderr。
//!
//! 需 workspace-write 沙箱。E1 阶段为应用层围栏（cwd 限定 + 超时 + 输出截断）；OS 级隔离
//! （Seatbelt/Landlock、断网）在 E2 补齐（见方案图 5 沙箱纵深）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{proc, sandbox};

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "bash",
            "在工作区内执行 shell 命令（cwd=工作根），带超时，返回 exit_code/stdout/stderr",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "cmd": { "type": "string", "description": "sh -c 执行的命令行" },
                "timeout_ms": { "type": "integer", "default": 30000 }
            },
            "required": ["cmd"]
        }))
        .guard(GuardHints {
            requires_auth: Some("exec".into()),
            requires_approval: cmx_agent_core::tool::Approval::Conditional, // 执行 shell 命令前人在环审批（X4）
            idempotent: false,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !ctx.sandbox.allows_write() {
            return Ok(ToolResult::err("bash: 当前沙箱为只读（需 workspace-write）"));
        }
        let Some(cmd) = input.get("cmd").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("bash: 'cmd' is required"));
        };
        let Some(cwd) = sandbox::first_root(ctx) else {
            return Ok(ToolResult::err("bash: no allowed_roots (sandbox denies exec)"));
        };
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(proc::DEFAULT_TIMEOUT_MS);
        let out = proc::run("sh", &["-c".to_string(), cmd.to_string()], cwd, timeout_ms).await;
        Ok(ToolResult::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn tmp() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-bash");
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn runs_and_captures_stdout() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = BashTool.invoke(json!({"cmd":"echo hello-cmx"}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["exit_code"], 0);
        assert!(r.output["stdout"].as_str().unwrap().contains("hello-cmx"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn cwd_is_workspace_root() {
        let (root, roots) = tmp();
        std::fs::write(root.join("marker.txt"), "x").unwrap();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = BashTool.invoke(json!({"cmd":"ls"}), &ctx).await.unwrap();
        assert!(r.output["stdout"].as_str().unwrap().contains("marker.txt"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn nonzero_exit_reported() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = BashTool.invoke(json!({"cmd":"exit 3"}), &ctx).await.unwrap();
        assert_eq!(r.output["exit_code"], 3);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn timeout_terminates() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = BashTool.invoke(json!({"cmd":"sleep 5","timeout_ms":300}), &ctx).await.unwrap();
        assert_eq!(r.output["timed_out"], true);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn denied_in_readonly() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots };
        let r = BashTool.invoke(json!({"cmd":"echo x"}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}

