//! MCP 工具代理：把一个 MCP server 的工具包装成 [`cmx_agent_core::Tool`]，invoke 时经 `tools/call`
//! 转发到 server。多个工具共享同一条 stdio 连接（`Arc<Mutex<McpClient>>` 串行访问）。
//!
//! 工具命名空间：`mcp_{label}_{tool}`，避免与内置工具/其它 server 撞名。

use std::sync::Arc;

use async_trait::async_trait;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::client::{McpClient, McpToolInfo, extract_text, is_error};

/// 一个 MCP 工具代理。
pub struct McpTool {
    /// 对外暴露的工具名（含命名空间前缀）。
    display_name: String,
    /// server 侧真实工具名。
    remote_name: String,
    description: String,
    input_schema: Value,
    client: Arc<Mutex<McpClient>>,
}

impl McpTool {
    pub fn new(label: &str, info: McpToolInfo, client: Arc<Mutex<McpClient>>) -> Self {
        Self {
            display_name: format!("mcp_{label}_{}", info.name),
            remote_name: info.name,
            description: info.description,
            input_schema: info.input_schema,
            client,
        }
    }
}

#[async_trait]
impl Tool for McpTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(self.display_name.clone(), self.description.clone())
            .schema(self.input_schema.clone())
        // MCP server 由用户在 mcp.json 显式配置 = 已信任，默认不强制审批（与 Codex/Claude Code 一致）。
        // 需要门控时可后续按 server 配置开审批。
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let mut client = self.client.lock().await;
        match client.call_tool(&self.remote_name, input).await {
            Ok(result) => {
                let text = extract_text(&result);
                if is_error(&result) {
                    Ok(ToolResult::err(format!("MCP 工具错误: {text}")))
                } else {
                    Ok(ToolResult::ok(json!({ "text": text, "raw": result })))
                }
            }
            Err(e) => Ok(ToolResult::err(format!("MCP 调用失败: {e}"))),
        }
    }
}

/// 单个 MCP server 的连接配置。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct McpServerConfig {
    /// 命名空间标签（工具前缀）。
    pub label: String,
    /// 可执行命令（如 `npx` / `python3` / 绝对路径）。
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

/// 连接一个 MCP server 并返回其全部工具代理（连接失败返回 Err，调用方可跳过该 server）。
pub async fn connect_server(cfg: &McpServerConfig) -> Result<Vec<Arc<dyn Tool>>, crate::client::McpError> {
    let mut client = McpClient::connect(&cfg.label, &cfg.command, &cfg.args, &cfg.env).await?;
    let infos = client.list_tools().await?;
    let shared = Arc::new(Mutex::new(client));
    let tools: Vec<Arc<dyn Tool>> = infos
        .into_iter()
        .map(|info| Arc::new(McpTool::new(&cfg.label, info, shared.clone())) as Arc<dyn Tool>)
        .collect();
    Ok(tools)
}

/// 从配置文件读取 MCP server 列表并逐个连接，返回全部工具代理（**单个 server 失败仅跳过并告警，不影响其余**）。
/// 文件格式：`[{ "label","command","args"?,"env"? }, ...]`。文件不存在返回空表（MCP 为 opt-in）。
pub async fn load_and_connect(config_path: &std::path::Path) -> Vec<Arc<dyn Tool>> {
    let Ok(content) = std::fs::read_to_string(config_path) else {
        return vec![]; // 无配置 = 不启用 MCP
    };
    let configs: Vec<McpServerConfig> = match serde_json::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("MCP 配置解析失败（{}）：{e}", config_path.display());
            return vec![];
        }
    };
    let mut all: Vec<Arc<dyn Tool>> = Vec::new();
    for cfg in &configs {
        match connect_server(cfg).await {
            Ok(mut tools) => {
                tracing::info!("MCP server '{}' 已连接，{} 个工具", cfg.label, tools.len());
                all.append(&mut tools);
            }
            Err(e) => tracing::warn!("MCP server '{}' 连接失败（跳过）：{e}", cfg.label),
        }
    }
    all
}
