//! `http` 载体：把一个 REST 端点包装成工具。`path` 里的 `{arg}` 取自入参；GET 用 query，其它用 JSON body。

use async_trait::async_trait;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{PluginManifest, subst};

pub struct HttpPluginTool {
    manifest: PluginManifest,
    client: reqwest::Client,
}

impl HttpPluginTool {
    pub fn new(manifest: PluginManifest) -> Self {
        Self {
            manifest,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }
}

#[async_trait]
impl Tool for HttpPluginTool {
    fn spec(&self) -> ToolSpec {
        self.manifest.tool_spec()
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let base = self.manifest.base_url.clone().unwrap_or_default();
        let path = subst(self.manifest.path.as_deref().unwrap_or(""), &input);
        let method = self.manifest.method.clone().unwrap_or_else(|| "GET".into()).to_uppercase();
        let url = format!("{}{}", base.trim_end_matches('/'), path);

        let req = match method.as_str() {
            "POST" => self.client.post(&url).json(&input),
            "PUT" => self.client.put(&url).json(&input),
            "DELETE" => self.client.delete(&url),
            _ => self.client.get(&url),
        };
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return Ok(ToolResult::err(format!("插件 {}: 请求失败 {e}", self.manifest.name))),
        };
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        // 尝试当 JSON，否则回文本（截断防灌爆）
        let body: Value = serde_json::from_str(&text).unwrap_or_else(|_| {
            let t: String = text.chars().take(6000).collect();
            json!(t)
        });
        Ok(ToolResult::ok(json!({
            "service": "cmx-plugin", "plugin": self.manifest.name, "kind": "http",
            "status": status, "url": url, "body": body,
        })))
    }
}
