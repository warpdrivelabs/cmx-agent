//! `command` 载体：把一条 shell 命令包装成工具。`command`/`args` 里的 `{arg}` 取自入参。
//! 写/执行类清单应置 `requires_approval:true` → 调用前过审批卡（人在环兜底任意命令）。
//!
//! S1b（方案 §6.1）：弃自有 spawn 改道共享执行器 `proc::run_profiled`（profile=Plugin）——
//! 超时钳制 / 64KB 截断 / Job Object 整树收尸 / OS 沙箱（受限令牌）一次包装全量生效；
//! cwd 由 manifest.working_dir 或工作区根决定（旧实现继承 agent 进程 cwd，语义变化见对照表）。

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

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(cmd) = self.manifest.command.as_deref() else {
            return Ok(ToolResult::err(format!("插件 {}: 缺 command", self.manifest.name)));
        };
        let program = subst(cmd, &input);
        let args: Vec<String> = self.manifest.args.iter().map(|a| subst(a, &input)).collect();
        let timeout = self.manifest.timeout_ms.unwrap_or(60_000);
        let Some(cwd) = crate::proc_cwd(ctx, &self.manifest) else {
            return Ok(ToolResult::err(format!("插件 {}: 无法解析工作目录", self.manifest.name)));
        };
        let out = crate::plugin_spawn(ctx, &program, &args, &cwd, timeout).await;

        // 保 schema：顶层键与旧版一致（service/plugin/kind/exit_code/stdout/stderr），
        // 并入 sandbox / timed_out / error 审计字段（ToolResult 通道，模型可见、落库）。
        let mut v = json!({
            "service": "cmx-plugin",
            "plugin": self.manifest.name,
            "kind": "command",
            "exit_code": out.get("exit_code").cloned().unwrap_or(Value::Null),
            "stdout": out.get("stdout").cloned().unwrap_or_else(|| json!("")),
            "stderr": out.get("stderr").cloned().unwrap_or_else(|| json!("")),
        });
        let m = v.as_object_mut().expect("command plugin result object");
        if let Some(sb) = out.get("sandbox") {
            m.insert("sandbox".into(), sb.clone());
        }
        if out.get("timed_out").and_then(|t| t.as_bool()).unwrap_or(false) {
            m.insert("timed_out".into(), Value::Bool(true));
            m.insert("error".into(), json!("插件子进程超时（已整树终止）"));
        }
        if let Some(e) = out.get("error") {
            m.insert("error".into(), e.clone());
        }
        Ok(ToolResult::ok(v))
    }
}
