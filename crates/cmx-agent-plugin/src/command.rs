//! `command` 载体：把一条 shell 命令包装成工具。`command`/`args` 里的 `{arg}` 取自入参。
//! 写/执行类清单应置 `requires_approval:true` → 调用前过审批卡（人在环兜底任意命令）。

use std::time::Duration;

use async_trait::async_trait;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{PluginManifest, subst};

pub struct CommandPluginTool {
    manifest: PluginManifest,
}

impl CommandPluginTool {
    pub fn new(manifest: PluginManifest) -> Self {
        Self { manifest }
    }
}

#[async_trait]
impl Tool for CommandPluginTool {
    fn spec(&self) -> ToolSpec {
        self.manifest.tool_spec()
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(cmd) = self.manifest.command.as_deref() else {
            return Ok(ToolResult::err(format!("插件 {}: 缺 command", self.manifest.name)));
        };
        let program = subst(cmd, &input);
        let args: Vec<String> = self.manifest.args.iter().map(|a| subst(a, &input)).collect();

        let fut = tokio::process::Command::new(&program)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .output();
        let out = match tokio::time::timeout(Duration::from_secs(60), fut).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => return Ok(ToolResult::err(format!("插件 {}: 启动失败 {e}", self.manifest.name))),
            Err(_) => return Ok(ToolResult::err(format!("插件 {}: 超时（>60s）", self.manifest.name))),
        };
        let stdout: String = String::from_utf8_lossy(&out.stdout).chars().take(8000).collect();
        let stderr: String = String::from_utf8_lossy(&out.stderr).chars().take(2000).collect();
        Ok(ToolResult::ok(json!({
            "service": "cmx-plugin", "plugin": self.manifest.name, "kind": "command",
            "exit_code": out.status.code(),
            "stdout": stdout.trim_end(),
            "stderr": stderr.trim_end(),
        })))
    }
}
