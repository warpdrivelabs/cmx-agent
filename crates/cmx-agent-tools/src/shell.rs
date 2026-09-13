//! `shell` —— 在工作区内执行命令（cwd=第一个工作根），带超时，回传 exit/stdout/stderr。
//!
//! Windows 适配（2026-09-08 方案 P0，原 `bash` 工具改名 `shell`）：不再硬编码 `sh -c`，
//! 经 [`crate::proc::resolve_shell`] 探测（`CMX_AGENT_SHELL > pwsh > powershell > sh > cmd`），
//! 每 shell 一套 argv 模板；工具描述注入当前平台/shell 信号，命令语义由模型适配
//! （业界共识：不做 bash→PowerShell 命令翻译）。名字即信号：叫 bash 模型就输出 bash 语法。
//! 需 workspace-write 沙箱。E1 应用层围栏（cwd + 超时 + 截断 + Job Object 收尸）；
//! OS 级隔离（Seatbelt/Landlock/受限令牌）在 E2 补齐（见方案图 5 沙箱纵深）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{proc, sandbox};

/// 工具描述：注入平台 + 当前 shell 信号 + Windows 安全规则（对齐 codex windows_shell_guidance）。
fn spec_description() -> String {
    let sh = proc::resolve_shell();
    let name = match sh.kind {
        proc::ShellKind::Sh => "sh",
        proc::ShellKind::PowerShell => "pwsh/powershell",
        proc::ShellKind::Cmd => "cmd",
    };
    if cfg!(windows) {
        format!(
            "在工作区内执行命令（当前平台 Windows · shell={name}，cwd=工作根），带超时，返回 exit_code/stdout/stderr。\
             生成与 {name} 兼容的命令；路径用 Windows 原生写法（C:\\dir\\file）。\
             安全规则：删除/移动等破坏性操作端到端用同一种 shell 完成（禁止 PowerShell 枚举路径再交 cmd /c 删除）；\
             递归删除/移动前先确认解析后的绝对路径在工作区内；后台进程必须隐藏窗口（PowerShell 加 -WindowStyle Hidden）"
        )
    } else {
        "在工作区内执行 shell 命令（当前平台 unix · shell=sh，cwd=工作根），带超时，返回 exit_code/stdout/stderr".into()
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
                requires_approval: cmx_agent_core::tool::Approval::Conditional, // 执行命令前人在环审批（X4）
                idempotent: false,
                writes: true, // 命令可写盘：ReadOnly 下随 SandboxGuard 中央拒绝
                ..Default::default()
            })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !ctx.sandbox.allows_write() {
            return Ok(ToolResult::err("shell: 当前沙箱为只读（需 workspace-write）"));
        }
        let Some(cmd) = input.get("cmd").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("shell: 'cmd' is required"));
        };
        let Some(cwd) = sandbox::first_root(ctx) else {
            return Ok(ToolResult::err("shell: no allowed_roots (sandbox denies exec)"));
        };
        let timeout_ms = input
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(proc::DEFAULT_TIMEOUT_MS);
        let out = proc::run_cmd(cmd, cwd, timeout_ms).await;
        Ok(ToolResult::ok(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn tmp() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-shell");
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn runs_and_captures_stdout() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = ShellTool.invoke(json!({"cmd":"echo hello-cmx"}), &ctx).await.unwrap();
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
        let r = ShellTool.invoke(json!({"cmd":"ls"}), &ctx).await.unwrap();
        assert!(r.output["stdout"].as_str().unwrap().contains("marker.txt"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn nonzero_exit_reported() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = ShellTool.invoke(json!({"cmd":"exit 3"}), &ctx).await.unwrap();
        assert_eq!(r.output["exit_code"], 3);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn timeout_terminates() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let r = ShellTool.invoke(json!({"cmd":"sleep 5","timeout_ms":300}), &ctx).await.unwrap();
        assert_eq!(r.output["timed_out"], true);
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn denied_in_readonly() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots };
        let r = ShellTool.invoke(json!({"cmd":"echo x"}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn spec_name_is_shell_with_platform_signal() {
        let s = ShellTool.spec();
        assert_eq!(s.name, "shell");
        assert!(s.description.contains("shell="), "描述应注入当前 shell 信号");
    }
}
