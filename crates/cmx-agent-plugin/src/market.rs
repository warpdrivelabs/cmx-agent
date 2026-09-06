//! 远程市场：从一个 `marketplace.json` 目录（URL）拉可安装插件清单，列出 / 按名安装。
//!
//! 目录格式：`{ "name": "...", "plugins": [ { "name","version","kind","description","manifest": {…} }, … ] }`
//! 每个条目内嵌一份完整 `cmx-plugin.json`（`manifest`），安装即把它写进本地 plugins 目录（审批门，名称净化）。
//! 市场 URL 来自工具入参 `url` 或环境变量 `CMX_AGENT_PLUGIN_MARKET`。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

/// 市场里的一个可安装条目。`manifest` 为内嵌的完整 cmx-plugin.json。
#[derive(Debug, Clone, Deserialize)]
pub struct MarketEntry {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub description: String,
    /// 信息页 HTML 链接（顶层缺省时回退内嵌 manifest.homepage）。
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub icon: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    #[serde(default)]
    pub manifest: Option<Value>,
}

impl MarketEntry {
    /// 详情/列表用摘要（含 installable + homepage；homepage 顶层优先、回退内嵌 manifest）。
    pub fn summary(&self) -> serde_json::Value {
        let mf_homepage = self
            .manifest
            .as_ref()
            .and_then(|m| m.get("homepage"))
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let mf_get = |k: &str| {
            self.manifest
                .as_ref()
                .and_then(|m| m.get(k))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        };
        serde_json::json!({
            "name": self.name,
            "kind": self.kind,
            "version": self.version,
            "description": self.description,
            "homepage": self.homepage.clone().or(mf_homepage),
            "icon": self.icon.clone().or_else(|| mf_get("icon")),
            "author": self.author.clone().or_else(|| mf_get("author")),
            "installable": self.manifest.is_some(),
            // 内嵌清单：前端「安装」按钮据此直接经前门 install_plugin 写入。
            "manifest": self.manifest,
        })
    }
}

/// 市场目录。
#[derive(Debug, Clone, Deserialize)]
pub struct Catalog {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub plugins: Vec<MarketEntry>,
}

/// 解析市场 URL：入参 `url` 优先，否则环境变量 `CMX_AGENT_PLUGIN_MARKET`。
pub(crate) fn resolve_market_url(input: &Value) -> Option<String> {
    input
        .get("url")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("CMX_AGENT_PLUGIN_MARKET").ok())
        .filter(|s| !s.is_empty())
}

/// 拉取并解析市场目录。
pub(crate) async fn fetch_catalog(client: &reqwest::Client, url: &str) -> Result<Catalog, String> {
    let resp = client.get(url).send().await.map_err(|e| format!("拉取市场失败 {e}"))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| format!("读取市场响应失败 {e}"))?;
    if !status.is_success() {
        return Err(format!("市场返回 HTTP {}", status.as_u16()));
    }
    serde_json::from_str::<Catalog>(&text).map_err(|e| format!("市场目录非法 {e}"))
}

/// 拉取远程市场目录并返回条目摘要数组（含 homepage/installable）。供 `AgentApp::list_plugins` 直接消费。
/// 内部自建带超时的 reqwest client；失败返回 Err（调用方决定是否降级为空市场）。
pub async fn fetch_market_catalog(url: &str) -> Result<Vec<Value>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());
    let cat = fetch_catalog(&client, url).await?;
    Ok(cat.plugins.iter().map(MarketEntry::summary).collect())
}

/// `plugin_marketplace`：列出远程市场里可安装的插件（只读，不改能力，无需审批）。
pub struct PluginMarketplaceTool {
    client: reqwest::Client,
}
impl Default for PluginMarketplaceTool {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}
#[async_trait]
impl Tool for PluginMarketplaceTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "plugin_marketplace",
            "浏览远程插件市场：拉取 marketplace.json 目录，列出可安装的插件（名/类型/版本/描述）。\
             之后可用 plugin_install 传 {name} 从市场安装。URL 取入参 url 或环境变量 CMX_AGENT_PLUGIN_MARKET。",
        )
        .schema(json!({
            "type": "object",
            "properties": { "url": { "type": "string", "description": "市场目录 URL（缺省用 CMX_AGENT_PLUGIN_MARKET）" } }
        }))
        .guard(GuardHints { idempotent: true, ..Default::default() })
    }
    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(url) = resolve_market_url(&input) else {
            return Ok(ToolResult::err(
                "plugin_marketplace: 未提供市场 URL（入参 url 或设 CMX_AGENT_PLUGIN_MARKET）",
            ));
        };
        let cat = match fetch_catalog(&self.client, &url).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::err(format!("plugin_marketplace: {e}"))),
        };
        let plugins: Vec<Value> = cat
            .plugins
            .iter()
            .map(MarketEntry::summary)
            .collect();
        Ok(ToolResult::ok(json!({
            "service": "cmx-plugin", "market": cat.name, "url": url,
            "count": plugins.len(), "plugins": plugins,
            "note": "用 plugin_install 传 {name}（可选 url）从市场安装",
        })))
    }
}
