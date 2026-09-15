//! `run_tests` —— 自动识别工作区构建系统并跑测试（cargo/npm/pytest/go），或执行自定义测试命令。需 workspace-write。

use async_trait::async_trait;
use cmx_agent_core::guard::SandboxMode;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{proc, sandbox};

pub struct RunTestsTool;

/// 返回 (program, args, framework) 或 None。
fn detect(root: &std::path::Path) -> Option<(&'static str, Vec<String>, &'static str)> {
    if root.join("Cargo.toml").exists() {
        return Some(("cargo", vec!["test".into()], "cargo"));
    }
    if root.join("go.mod").exists() {
        return Some(("go", vec!["test".into(), "./...".into()], "go"));
    }
    if root.join("package.json").exists() {
        return Some(("npm", vec!["test".into(), "--silent".into()], "npm"));
    }
    if root.join("pyproject.toml").exists()
        || root.join("pytest.ini").exists()
        || root.join("setup.cfg").exists()
        || root.join("tests").is_dir()
    {
        return Some(("pytest", vec!["-q".into()], "pytest"));
    }
    None
}

#[async_trait]
impl Tool for RunTestsTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "run_tests",
            "自动识别工作区(cargo/npm/pytest/go)并跑测试；或用 command 指定自定义测试命令",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "可选：自定义测试命令（与当前 shell 兼容）；给了则忽略自动识别" },
                "timeout_ms": { "type": "integer", "default": 120000 }
            }
        }))
        .guard(GuardHints {
            requires_auth: Some("exec".into()),
            idempotent: false,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !ctx.sandbox.allows_write() {
            return Ok(ToolResult::err("run_tests: 需 workspace-write 沙箱"));
        }
        let Some(cwd) = sandbox::first_root(ctx) else {
            return Ok(ToolResult::err("run_tests: no allowed_roots"));
        };
        let timeout = input
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(120_000);
        // OS 沙箱（S1a）：WorkspaceWrite 档走受限令牌原生 spawn（profile=Shell：.git deny 生效，
        // 测试误删 .git 同样被拒；git 类操作归 git 工具）。辅助闭包：档位分路。
        let pctx = cmx_agent_sandbox::ProcCtx {
            sandbox: ctx.sandbox,
            roots: ctx.allowed_roots,
            profile: cmx_agent_sandbox::Profile::Shell,
        };
        let sandboxed = ctx.sandbox == SandboxMode::WorkspaceWrite;

        if let Some(cmd) = input.get("command").and_then(|v| v.as_str()) {
            // 与 shell 工具同一探测链/argv 模板（P0：不再硬编码 sh -c）。
            let out = if sandboxed {
                proc::run_cmd_profiled(&pctx, cmd, cwd, timeout).await
            } else {
                proc::run_cmd(cmd, cwd, timeout).await
            };
            return Ok(ToolResult::ok(json!({"framework":"custom","command":cmd,"result":out})));
        }
        match detect(cwd) {
            Some((prog, args, fw)) => {
                let out = if sandboxed {
                    proc::run_profiled(&pctx, prog, &args, cwd, timeout).await
                } else {
                    proc::run(prog, &args, cwd, timeout).await
                };
                Ok(ToolResult::ok(json!({
                    "framework": fw,
                    "command": format!("{prog} {}", args.join(" ")),
                    "result": out,
                })))
            }
            None => Ok(ToolResult::err(
                "run_tests: 未识别测试框架（无 Cargo.toml/package.json/pyproject/go.mod/tests）；可用 command 显式指定",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use std::path::PathBuf;

    fn tmp() -> (PathBuf, Vec<PathBuf>) {
        let root = crate::testutil::unique_dir("cmx-rt");
        (root.clone(), vec![root])
    }

    #[test]
    fn detect_cargo() {
        let (root, _) = tmp();
        std::fs::write(root.join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(detect(&root).unwrap().2, "cargo");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn detect_none() {
        let (root, _) = tmp();
        assert!(detect(&root).is_none());
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn custom_command_runs() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
        let r = RunTestsTool
            .invoke(json!({"command":"echo tests-ok"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["framework"], "custom");
        assert!(r.output["result"]["stdout"].as_str().unwrap().contains("tests-ok"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn unknown_framework_errors() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
        let r = RunTestsTool.invoke(json!({}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}
