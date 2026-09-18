//! `shell` —— 在普通工作目录执行命令，带超时，回传 exit/stdout/stderr。
//!
//! Windows 经共享执行器探测 shell，保留 CREATE_NO_WINDOW 与 Job Object 进程树清理。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{paths, proc};

/// 工具描述注入平台与当前 shell 信号，不做命令语法翻译。
fn spec_description() -> String {
    let sh = proc::resolve_shell();
    let name = match sh.kind {
        proc::ShellKind::Sh => "sh",
        proc::ShellKind::PowerShell => "pwsh/powershell",
        proc::ShellKind::Cmd => "cmd",
    };
    if cfg!(windows) {
        format!(
            "在工作目录执行命令（当前平台 Windows · shell={name}），带超时，返回 exit_code/stdout/stderr。\
             生成与 {name} 兼容的命令；路径用 Windows 原生写法（C:\\dir\\file）。\
             删除/移动等操作端到端用同一种 shell 完成，执行前确认目标路径；\
             后台进程必须隐藏窗口（PowerShell 加 -WindowStyle Hidden）。"
        )
    } else {
        "在工作目录执行 shell 命令（当前平台 unix · shell=sh），带超时，返回 exit_code/stdout/stderr".into()
    }
}

pub struct ShellTool;

#[async_trait]
impl Tool for ShellTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("shell", spec_description())
            .schema(json!({
                "type": "object",
                "properties": {
                    "cmd": { "type": "string", "description": "与当前 shell 兼容的命令行" },
                    "timeout_ms": { "type": "integer", "default": 30000 }
                },
                "required": ["cmd"]
            }))
            .guard(GuardHints {
                requires_auth: Some("exec".into()),
                requires_approval: cmx_agent_core::tool::Approval::Conditional,
                idempotent: false,
                writes: true,
                ..Default::default()
            })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(cmd) = input.get("cmd").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("shell: 'cmd' is required"));
        };
        let cwd = match paths::working_dir(ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("shell: {e}"))),
        };
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(proc::DEFAULT_TIMEOUT_MS);
        Ok(ToolResult::ok(proc::run_cmd(cmd, &cwd, timeout_ms).await))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn tmp() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-shell");
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn runs_and_captures_stdout() {
        let (root, roots) = tmp();
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        let r = ShellTool
            .invoke(json!({"cmd":"echo hello-cmx"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["exit_code"], 0);
        assert!(r.output["stdout"].as_str().unwrap().contains("hello-cmx"));
        assert!(r.output.get("sandbox").is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn cwd_is_workspace_root() {
        let (root, roots) = tmp();
        std::fs::write(root.join("marker.txt"), "x").unwrap();
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        let cmd = if proc::resolve_shell().kind == proc::ShellKind::Cmd {
            "dir"
        } else {
            "ls"
        };
        let r = ShellTool.invoke(json!({"cmd":cmd}), &ctx).await.unwrap();
        assert!(r.output["stdout"].as_str().unwrap().contains("marker.txt"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn can_write_outside_workspace() {
        let (base, _) = tmp();
        let roots = vec![base.join("workspace")];
        std::fs::create_dir_all(&roots[0]).unwrap();
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        let cmd = if proc::resolve_shell().kind == proc::ShellKind::Cmd {
            "echo outside> ..\\outside.txt"
        } else {
            "echo outside > ../outside.txt"
        };
        let r = ShellTool.invoke(json!({"cmd":cmd}), &ctx).await.unwrap();
        assert_eq!(r.output["exit_code"], 0, "{r:?}");
        assert!(!std::fs::read(base.join("outside.txt")).unwrap().is_empty());
        std::fs::remove_dir_all(base).unwrap();
    }

    #[tokio::test]
    async fn nonzero_exit_reported() {
        let (root, roots) = tmp();
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        let r = ShellTool
            .invoke(json!({"cmd":"exit 3"}), &ctx)
            .await
            .unwrap();
        assert_eq!(r.output["exit_code"], 3);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn timeout_terminates() {
        let (root, roots) = tmp();
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        let cmd = match proc::resolve_shell().kind {
            proc::ShellKind::PowerShell => "Start-Sleep -Seconds 5",
            proc::ShellKind::Cmd => "ping -n 6 127.0.0.1 > nul",
            proc::ShellKind::Sh => "sleep 5",
        };
        let r = ShellTool
            .invoke(json!({"cmd":cmd,"timeout_ms":300}), &ctx)
            .await
            .unwrap();
        assert_eq!(r.output["timed_out"], true);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn spec_name_is_shell_with_platform_signal() {
        let s = ShellTool.spec();
        assert_eq!(s.name, "shell");
        assert!(s.description.contains("shell="));
        assert_eq!(
            s.guard.requires_approval,
            cmx_agent_core::tool::Approval::Conditional
        );
        assert!(s.guard.writes);
    }
}
