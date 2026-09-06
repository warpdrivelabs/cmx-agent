//! `web_search`：网络搜索，返回若干条结果（标题/链接/摘要）。
//!
//! 检索端点由运营方配置（env `CMX_AGENT_SEARCH_URL`，含 `{q}` 占位；默认 DuckDuckGo HTML），
//! 属**可信**来源 → 不做 SSRF 拦截（区别于 `web_fetch` 的模型可控 URL）。结果解析见 [`crate::html::parse_results`]。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{client, html};

const DEFAULT_ENDPOINT: &str = "https://html.duckduckgo.com/html/?q={q}";

/// 最小 URL 查询编码。
fn q_encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len() * 2);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            b' ' => o.push('+'),
            _ => o.push_str(&format!("%{b:02X}")),
        }
    }
    o
}

/// `web_search` 工具。`endpoint`：检索端点模板（含 `{q}`）；builder 从 env 读，默认 DuckDuckGo。
pub struct WebSearchTool {
    pub endpoint: String,
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::from_env()
    }
}

impl WebSearchTool {
    /// 从 env `CMX_AGENT_SEARCH_URL`(含{q}) 读检索端点，缺省 DuckDuckGo HTML（builder 用）。
    pub fn from_env() -> Self {
        Self {
            endpoint: std::env::var("CMX_AGENT_SEARCH_URL")
                .unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string()),
        }
    }
}

#[async_trait]
impl Tool for WebSearchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "web_search",
            "网络搜索，返回若干条结果（标题/链接/摘要）。默认 DuckDuckGo；可用 env CMX_AGENT_SEARCH_URL(含{q}) 换检索端点。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "搜索关键词" },
                "max_results": { "type": "integer", "default": 6, "description": "最多返回结果条数" }
            },
            "required": ["query"]
        }))
        .guard(GuardHints {
            requires_auth: Some("net:search".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(query) = input.get("query").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("web_search: 'query' 必填"));
        };
        let max_results = input.get("max_results").and_then(|v| v.as_u64()).unwrap_or(6) as usize;

        let endpoint = self.endpoint.replace("{q}", &q_encode(query));

        let resp = match client(15000).get(&endpoint).send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("web_search: 请求失败 {e}"))),
        };
        let body = match resp.text().await {
            Ok(t) => t,
            Err(e) => return Ok(ToolResult::err(format!("web_search: 读取响应失败 {e}"))),
        };
        let results: Vec<Value> = html::parse_results(&body)
            .into_iter()
            .take(max_results)
            .map(|(title, url, snippet)| json!({ "title": title, "url": url, "snippet": snippet }))
            .collect();

        Ok(ToolResult::ok(json!({
            "query": query,
            "count": results.len(),
            "results": results,
        })))
    }
}
