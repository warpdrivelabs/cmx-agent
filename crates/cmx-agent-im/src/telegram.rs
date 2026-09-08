//! Telegram Bot API 参考 [`ImProvider`]：长轮询 `getUpdates` + `sendMessage`。
//!
//! 配置（env）：`CMX_AGENT_IM_TOKEN`（bot token）+ 可选 `CMX_AGENT_IM_BASE`
//! （默认 `https://api.telegram.org`，可指自建/代理网关）。企业 IM 按同一 trait 另加 provider。

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::{ImProvider, InboundMsg};

pub struct TelegramProvider {
    token: String,
    base: String,
    client: reqwest::Client,
}

impl TelegramProvider {
    pub fn new(token: impl Into<String>, base_url: Option<String>) -> Self {
        Self {
            token: token.into(),
            base: base_url.unwrap_or_else(|| "https://api.telegram.org".into()),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(65))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// 从 env 读取（`CMX_AGENT_IM_TOKEN` 必需，`CMX_AGENT_IM_BASE` 可选）。
    pub fn from_env() -> Option<Self> {
        let token = std::env::var("CMX_AGENT_IM_TOKEN").ok().filter(|t| !t.is_empty())?;
        Some(Self::new(token, std::env::var("CMX_AGENT_IM_BASE").ok()))
    }

    fn url(&self, method: &str) -> String {
        format!("{}/bot{}/{}", self.base, self.token, method)
    }
}

#[async_trait]
impl ImProvider for TelegramProvider {
    async fn poll(&self, offset: i64) -> Result<(Vec<InboundMsg>, i64), String> {
        let resp = self
            .client
            .get(self.url("getUpdates"))
            .query(&[("offset", offset.to_string()), ("timeout", "50".into())])
            .send()
            .await
            .map_err(|e| format!("getUpdates 请求失败：{e}"))?;
        let v: Value = resp.json().await.map_err(|e| format!("getUpdates 解析失败：{e}"))?;
        if v.get("ok").and_then(|b| b.as_bool()) != Some(true) {
            return Err(format!("telegram getUpdates ok!=true：{v}"));
        }
        let mut out = Vec::new();
        let mut next = offset;
        if let Some(arr) = v.get("result").and_then(|r| r.as_array()) {
            for up in arr {
                let uid = up.get("update_id").and_then(|x| x.as_i64()).unwrap_or(0);
                next = next.max(uid + 1);
                if let Some(msg) = up.get("message") {
                    let chat = msg
                        .get("chat")
                        .and_then(|c| c.get("id"))
                        .map(|id| id.to_string())
                        .unwrap_or_default();
                    if let Some(text) = msg.get("text").and_then(|t| t.as_str())
                        && !chat.is_empty() {
                            out.push(InboundMsg {
                                chat_id: chat,
                                text: text.to_string(),
                                update_id: uid,
                                sender: String::new(), // Telegram 一期不绑定，留空
                            });
                        }
                }
            }
        }
        Ok((out, next))
    }

    async fn send(&self, chat_id: &str, text: &str) -> Result<(), String> {
        let resp = self
            .client
            .post(self.url("sendMessage"))
            .json(&json!({ "chat_id": chat_id, "text": text }))
            .send()
            .await
            .map_err(|e| format!("sendMessage 请求失败：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("sendMessage HTTP {}", resp.status()));
        }
        Ok(())
    }
}
