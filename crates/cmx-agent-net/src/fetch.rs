//! `web_fetch`：抓取一个 http/https 网页，返回其可读正文文本（HTML 自动去标签抽正文 + 标题）。
//! URL 来自模型（不可信）→ 经 [`crate::ensure_public_url`] SSRF 基线拦截。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{client, ensure_public_url, html};

/// `web_fetch` 工具。`allow_private`：是否放开私网 URL（builder 从 env 读；测试可直接置 true）。
#[derive(Default)]
pub struct WebFetchTool {
    pub allow_private: bool,
}

impl WebFetchTool {
    /// 从 env `CMX_AGENT_NET_ALLOW_PRIVATE` 读取是否放开私网（builder 用）。
    pub fn from_env() -> Self {
        Self {
            allow_private: std::env::var("CMX_AGENT_NET_ALLOW_PRIVATE").is_ok(),
        }
    }
}

#[async_trait]
impl Tool for WebFetchTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "web_fetch",
            "抓取一个网页(http/https)并返回其可读正文文本（HTML 自动去标签、抽标题）。用于查资料、读在线文档。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "要抓取的网页 URL（http/https，需公网可达）" },
                "max_chars": { "type": "integer", "default": 8000, "description": "返回正文最多字符数" }
            },
            "required": ["url"]
        }))
        .guard(GuardHints {
            requires_auth: Some("net:fetch".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(url) = input.get("url").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("web_fetch: 'url' 必填"));
        };
        let max_chars = input.get("max_chars").and_then(|v| v.as_u64()).unwrap_or(8000) as usize;
        let u = match ensure_public_url(url, self.allow_private) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::err(format!("web_fetch: {e}"))),
        };
        let resp = match client(15000).get(u.clone()).send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("web_fetch: 请求失败 {e}"))),
        };
        let status = resp.status().as_u16();
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let body = match resp.text().await {
            Ok(t) => t,
            Err(e) => return Ok(ToolResult::err(format!("web_fetch: 读取响应失败 {e}"))),
        };
        let is_html = ctype.contains("html")
            || body.trim_start().starts_with("<!")
            || !html::extract_title(&body).is_empty();
        let (title, text) = if is_html {
            (html::extract_title(&body), html::html_to_text(&body))
        } else {
            (String::new(), body)
        };
        let chars = text.chars().count();
        Ok(ToolResult::ok(json!({
            "url": u.as_str(),
            "status": status,
            "title": title,
            "text": html::truncate(&text, max_chars),
            "chars": chars,
            "content_type": ctype,
        })))
    }
}
