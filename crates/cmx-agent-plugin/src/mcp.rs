//! `mcp` 载体：把一个 MCP server 声明进 `cmx-plugin.json`，启动时连上、把它的工具代理进注册表。
//!
//! 统一了 U3 的 MCP 生态入口——原先 MCP 走独立 `mcp.json`，现在也可作为一类**插件**（清单 `kind:"mcp"`）
//! 与 http/command/wasm 并列。连接是异步的（`cmx_agent_mcp::connect_server`），故不走 `load_plugins`（同步），
//! 而由壳在异步上下文调用 [`connect_mcp_plugins`]，结果并入 `.mcp_tools(...)`（与 mcp.json 同一去处）。

use std::path::Path;
use std::sync::Arc;

use cmx_agent_core::Tool;
use cmx_agent_mcp::McpServerConfig;

use crate::read_manifests;

/// 扫 `<dir>/*/cmx-plugin.json` 里 `kind:"mcp"` 的清单，逐个连接 MCP server，返回其代理工具集合。
///
/// 清单字段：`command`（server 可执行）、`args`、`env`（map）。连接失败的 server 跳过（graceful，不炸）。
pub async fn connect_mcp_plugins(dir: &Path) -> Vec<Arc<dyn Tool>> {
    let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
    for (_pdir, m) in read_manifests(dir) {
        if m.kind != "mcp" {
            continue;
        }
        let Some(command) = m.command.clone() else {
            tracing::warn!("mcp 插件 {} 缺 command，跳过", m.name);
            continue;
        };
        let cfg = McpServerConfig {
            label: m.name.clone(),
            command,
            args: m.args.clone(),
            env: m.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        };
        match cmx_agent_mcp::connect_server(&cfg).await {
            Ok(mut ts) => {
                tracing::info!("mcp 插件 {} 连接成功，代理 {} 个工具", m.name, ts.len());
                tools.append(&mut ts);
            }
            Err(e) => tracing::warn!("mcp 插件 {} 连接失败：{e}", m.name),
        }
    }
    tools
}

/// 连接**一份** mcp 清单（安装/启用时热连），返回其代理工具。非 mcp/缺 command → Err。
/// 供 app 在安装 mcp 插件时异步热注册（无需重启）。
pub async fn connect_mcp_manifest(manifest: &serde_json::Value) -> Result<Vec<Arc<dyn Tool>>, String> {
    let m: crate::PluginManifest =
        serde_json::from_value(manifest.clone()).map_err(|e| format!("清单非法 {e}"))?;
    if m.kind != "mcp" {
        return Err(format!("非 mcp 载体（kind={}）", m.kind));
    }
    let command = m.command.clone().ok_or("mcp 清单缺 command")?;
    let cfg = McpServerConfig {
        label: m.name.clone(),
        command,
        args: m.args.clone(),
        env: m.env.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
    };
    cmx_agent_mcp::connect_server(&cfg)
        .await
        .map_err(|e| format!("连接 mcp server 失败：{e}"))
}
