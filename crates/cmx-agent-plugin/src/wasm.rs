//! `wasm` 载体：把一个 WebAssembly 模块包装成工具，经 wasm 运行时（默认 `wasmer`）子进程执行。
//!
//! 两种模式（对齐 `command` 载体的子进程模型，隔离 + 可超时 + 无需在 crate 内嵌运行时）：
//! - **invoke 模式**（清单含 `invoke`）：`wasmer run <module> --invoke <fn> -- <args>`，调纯计算导出函数，取 stdout。
//! - **command 模式**（无 `invoke`）：`wasmer run <module> -- <args>`，跑 WASI 命令模块（有 `_start`）。
//!
//! `module` 为清单相对路径（相对插件自身目录）或绝对路径；`args` 里的 `{arg}` 取自入参；`.wat` 文本模块亦可直接跑。
//! 运行时可由清单 `runtime` 或环境变量 `CMX_AGENT_WASM_RUNTIME` 覆盖（默认 `wasmer`）。

use std::path::{Path, PathBuf};
use std::time::Duration;

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

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
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

        let fut = tokio::process::Command::new(&runtime)
            .args(&argv)
            .stdin(std::process::Stdio::null())
            .output();
        let out = match tokio::time::timeout(Duration::from_secs(60), fut).await {
            Ok(Ok(o)) => o,
            Ok(Err(e)) => {
                return Ok(ToolResult::err(format!(
                    "插件 {}: 启动 wasm 运行时 '{runtime}' 失败 {e}（未安装？可设 CMX_AGENT_WASM_RUNTIME）",
                    self.manifest.name
                )));
            }
            Err(_) => return Ok(ToolResult::err(format!("插件 {}: 超时（>60s）", self.manifest.name))),
        };
        let stdout: String = String::from_utf8_lossy(&out.stdout).chars().take(8000).collect();
        let stderr: String = String::from_utf8_lossy(&out.stderr).chars().take(2000).collect();
        Ok(ToolResult::ok(json!({
            "service": "cmx-plugin", "plugin": self.manifest.name, "kind": "wasm",
            "runtime": runtime,
            "module": module.display().to_string(),
            "invoke": self.manifest.invoke,
            "exit_code": out.status.code(),
            "stdout": stdout.trim_end(),
            "stderr": stderr.trim_end(),
        })))
    }
}
