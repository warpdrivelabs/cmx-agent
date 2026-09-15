//! `wasm` 载体：把一个 WebAssembly 模块包装成工具，经 wasm 运行时（默认 `wasmer`）子进程执行。
//!
//! 两种模式（对齐 `command` 载体的子进程模型，隔离 + 可超时 + 无需在 crate 内嵌运行时）：
//! - **invoke 模式**（清单含 `invoke`）：`wasmer run <module> --invoke <fn> -- <args>`，调纯计算导出函数，取 stdout。
//! - **command 模式**（无 `invoke`）：`wasmer run <module> -- <args>`，跑 WASI 命令模块（有 `_start`）。
//!
//! `module` 为清单相对路径（相对插件自身目录）或绝对路径；`args` 里的 `{arg}` 取自入参；`.wat` 文本模块亦可直接跑。
//! 运行时可由清单 `runtime` 或环境变量 `CMX_AGENT_WASM_RUNTIME` 覆盖（默认 `wasmer`）。

use std::path::{Path, PathBuf};
use async_trait::async_trait;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{PluginManifest, subst};

pub struct WasmPluginTool {
    manifest: PluginManifest,
    /// 插件所在目录（用于解析相对 `module` 路径）。
    plugin_dir: PathBuf,
}

impl WasmPluginTool {
    pub fn new(manifest: PluginManifest, plugin_dir: impl Into<PathBuf>) -> Self {
        Self { manifest, plugin_dir: plugin_dir.into() }
    }

    /// 解析模块绝对路径：绝对则原样，相对则挂到插件目录下。
    fn module_path(&self) -> Option<PathBuf> {
        let m = self.manifest.module.as_deref()?;
        let p = Path::new(m);
        Some(if p.is_absolute() { p.to_path_buf() } else { self.plugin_dir.join(p) })
    }

    /// 运行时二进制：清单 runtime > 环境变量 > 默认 wasmer。
    fn runtime(&self) -> String {
        self.manifest
            .runtime
            .clone()
            .or_else(|| std::env::var("CMX_AGENT_WASM_RUNTIME").ok())
            .unwrap_or_else(|| "wasmer".to_string())
    }
}

#[async_trait]
impl Tool for WasmPluginTool {
    fn spec(&self) -> ToolSpec {
        self.manifest.tool_spec()
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(module) = self.module_path() else {
            return Ok(ToolResult::err(format!("插件 {}: wasm 载体缺 module", self.manifest.name)));
        };
        if !module.exists() {
            return Ok(ToolResult::err(format!(
                "插件 {}: wasm 模块不存在 {}",
                self.manifest.name,
                module.display()
            )));
        }
        let runtime = self.runtime();
        let call_args: Vec<String> = self.manifest.args.iter().map(|a| subst(a, &input)).collect();
        let timeout = self.manifest.timeout_ms.unwrap_or(60_000);
        let Some(cwd) = crate::proc_cwd(ctx, &self.manifest) else {
            return Ok(ToolResult::err(format!("插件 {}: 无法解析工作目录", self.manifest.name)));
        };

        // 组装：<runtime> run <module> [--invoke <fn>] [-- <args...>]
        let mut argv: Vec<String> = vec!["run".into(), module.display().to_string()];
        if let Some(f) = self.manifest.invoke.as_deref() {
            argv.push("--invoke".into());
            argv.push(f.to_string());
        }
        if !call_args.is_empty() {
            argv.push("--".into());
            argv.extend(call_args);
        }

        // S1b：改道共享执行器（profile=Plugin）——超时钳制/截断/Job 收尸/OS 沙箱全量生效；
        // wasmer WASI 默认 preopen cwd，manifest.working_dir 会改模块文件视图（对照表已记）。
        let out = crate::plugin_spawn(ctx, &runtime, &argv, &cwd, timeout).await;
        if let Some(e) = out.get("error").and_then(|e| e.as_str()) {
            // 沙箱拒绝（denied 字段）：透传 fail-closed 文案，不套「未安装」话术（红队2 P2-5——
            // error 键还可能来自沙箱拒绝/等待失败，统一归因运行时会指引错向）。
            if let Some(denied) = out.get("sandbox").and_then(|sb| sb.get("denied")).and_then(|d| d.as_str()) {
                return Ok(ToolResult::err(format!("插件 {}: {denied}", self.manifest.name)));
            }
            // 其余 error = 普通启动失败（如未装 wasmer）：透传可行动原因。
            return Ok(ToolResult::err(format!(
                "插件 {}: 启动 wasm 运行时 '{runtime}' 失败 {e}（未安装？可设 CMX_AGENT_WASM_RUNTIME）",
                self.manifest.name
            )));
        }
        // 保 schema：顶层键与旧版一致，并入 sandbox/timed_out 审计字段。
        let mut v = json!({
            "service": "cmx-plugin", "plugin": self.manifest.name, "kind": "wasm",
            "runtime": runtime,
            "module": module.display().to_string(),
            "invoke": self.manifest.invoke,
            "exit_code": out.get("exit_code").cloned().unwrap_or(Value::Null),
            "stdout": out.get("stdout").cloned().unwrap_or_else(|| json!("")),
            "stderr": out.get("stderr").cloned().unwrap_or_else(|| json!("")),
        });
        let m = v.as_object_mut().expect("wasm plugin result object");
        if let Some(sb) = out.get("sandbox") {
            m.insert("sandbox".into(), sb.clone());
        }
        if out.get("timed_out").and_then(|t| t.as_bool()).unwrap_or(false) {
            m.insert("timed_out".into(), Value::Bool(true));
        }
        Ok(ToolResult::ok(v))
    }
}
