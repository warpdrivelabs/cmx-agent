//! cmx 服务 HTTP 客户端（照抄 cmx-ontology outbound 范式）。
//!
//! 解 cmx 统一 `{code,msg,data}` 信封：`code==0` 取 `data`，否则错误。**双认证兜底**：默认带
//! `X-Tenant`/`X-User` 头（适配 auth=off 的 e2e 实例，本机实测可用）；若配了 API Key 也一并带上
//! （auth=apikey 实例）。8s 超时，rustls（免系统 openssl）。

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use serde_json::Value;

/// 共享令牌槽：登录后由前门写入 access_token，连接器调用时读出挂 `Authorization: Bearer`。
/// `Arc<RwLock<..>>` 让「认证前门」与「所有连接器 client」共享同一份活令牌（登录即生效、登出即清）。
pub type TokenStore = Arc<RwLock<Option<String>>>;

/// 全局复用一个 reqwest Client（连接池复用；OnceLock 惰性初始化）。
fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .user_agent("cmx-agent-connectors/0.1")
            .build()
            .expect("build reqwest client")
    })
}

/// 一个 cmx 服务的调用句柄。
#[derive(Debug, Clone)]
pub struct CmxServiceClient {
    /// 形如 `http://127.0.0.1:8091`（无尾斜杠）。
    pub base_url: String,
    pub tenant: String,
    pub user: String,
    /// 可选 API Key（auth=apikey 实例才需要）。
    pub api_key: Option<String>,
    /// 可选共享令牌槽（auth=on 实例：带门户登录的 `Authorization: Bearer`；None/空则不带，回退 X-Tenant）。
    token: Option<TokenStore>,
}

/// 客户端错误（转 [`cmx_agent_core::ToolError`] 回灌，不致命）。
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("请求失败: {0}")]
    Transport(String),
    #[error("服务返回 HTTP {0}")]
    Http(u16),
    #[error("cmx 业务错误 code={code}: {msg}")]
    Envelope { code: i64, msg: String },
    #[error("响应解析失败: {0}")]
    Decode(String),
}

impl CmxServiceClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            tenant: "default".into(),
            user: "admin".into(),
            api_key: None,
            token: None,
        }
    }

    pub fn with_identity(mut self, tenant: impl Into<String>, user: impl Into<String>) -> Self {
        self.tenant = tenant.into();
        self.user = user.into();
        self
    }

    pub fn with_api_key(mut self, key: Option<String>) -> Self {
        self.api_key = key;
        self
    }

    /// 注入共享令牌槽——之后 `get_data`/`post_write` 若槽中有令牌即带 `Authorization: Bearer`。
    pub fn with_token(mut self, store: TokenStore) -> Self {
        self.token = Some(store);
        self
    }

    /// 读当前活令牌（槽为空 / 未登录 → None）。
    fn bearer(&self) -> Option<String> {
        self.token
            .as_ref()
            .and_then(|t| t.read().ok().and_then(|g| g.clone()))
            .filter(|s| !s.is_empty())
    }

    /// GET 一个返回 `{code,msg,data}` 信封的端点，成功取 `data`。
    pub async fn get_data(&self, path: &str) -> Result<Value, ClientError> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = client()
            .get(&url)
            .header("X-Tenant", &self.tenant)
            .header("X-User", &self.user)
            .header("Accept", "application/json");
        if let Some(k) = &self.api_key {
            req = req.header("X-API-Key", k);
        }
        if let Some(tok) = self.bearer() {
            req = req.header("Authorization", format!("Bearer {tok}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Http(status.as_u16()));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        unwrap_envelope(body)
    }

    /// POST 一个返回 `{code,msg,data}` 信封的**写端点**，成功取 `data`。带身份头（X-Tenant/X-User/X-API-Key）——
    /// 区别于 `post_data`（认证登录用，不带身份头）。引擎写侧工具用这个。
    pub async fn post_write(&self, path: &str, body: &Value) -> Result<Value, ClientError> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = client()
            .post(&url)
            .header("X-Tenant", &self.tenant)
            .header("X-User", &self.user)
            .header("Accept", "application/json")
            .json(body);
        if let Some(k) = &self.api_key {
            req = req.header("X-API-Key", k);
        }
        if let Some(tok) = self.bearer() {
            req = req.header("Authorization", format!("Bearer {tok}"));
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Http(status.as_u16()));
        }
        let body: Value = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        unwrap_envelope(body)
    }

    /// GET 一个**原样**返回 JSON（非信封）的端点，如 `/_mon/tech-stats` 也是信封但健康探测单独处理。
    pub async fn get_raw(&self, path: &str) -> Result<Value, ClientError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = client()
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(ClientError::Http(status.as_u16()));
        }
        resp.json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// POST 一段 JSON body 到返回 `{code,msg,data}` 信封的端点，成功取 `data`。
    /// 认证端点（/api/auth/login）不带 X-Tenant/X-User（无意义），只发 body。
    pub async fn post_data(&self, path: &str, body: Value) -> Result<Value, ClientError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = client()
            .post(&url)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        // 认证端点用信封 code 表达 401（HTTP 常为 200）；仅当 HTTP 5xx/连接层失败才当传输错。
        let status = resp.status();
        let parsed: Value = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        // 若 body 是信封则按 code 解；否则遇 HTTP 错再报 Http。
        match unwrap_envelope(parsed) {
            Ok(v) => Ok(v),
            Err(e) => {
                if status.is_success() {
                    Err(e)
                } else {
                    Err(ClientError::Http(status.as_u16()))
                }
            }
        }
    }

    /// GET 一个返回信封的端点，带 `Authorization: Bearer <token>`（如 /api/auth/me）。
    pub async fn get_data_bearer(&self, path: &str, token: &str) -> Result<Value, ClientError> {
        let url = format!("{}{}", self.base_url, path);
        let resp = client()
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let status = resp.status();
        let parsed: Value = resp
            .json()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()))?;
        match unwrap_envelope(parsed) {
            Ok(v) => Ok(v),
            Err(e) => {
                if status.is_success() {
                    Err(e)
                } else {
                    Err(ClientError::Http(status.as_u16()))
                }
            }
        }
    }
}

/// 解 cmx `{code,msg,data}` 信封。`code==0`（或缺省）→ 取 `data`；否则 Envelope 错误。
pub fn unwrap_envelope(body: Value) -> Result<Value, ClientError> {
    // 有些端点直接返回裸对象/数组（无信封）——若无 code 字段，原样返回。
    let Some(code) = body.get("code").and_then(|c| c.as_i64()) else {
        return Ok(body);
    };
    if code == 0 {
        Ok(body.get("data").cloned().unwrap_or(Value::Null))
    } else {
        let msg = body
            .get("msg")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error")
            .to_string();
        Err(ClientError::Envelope { code, msg })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_ok_extracts_data() {
        let body = json!({"code":0,"msg":"success","data":{"definitions":[1,2,3]}});
        let data = unwrap_envelope(body).unwrap();
        assert_eq!(data["definitions"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn envelope_error_is_reported() {
        let body = json!({"code":401,"msg":"无效 API Key"});
        let err = unwrap_envelope(body).unwrap_err();
        matches!(err, ClientError::Envelope { code: 401, .. });
        assert!(err.to_string().contains("401"));
    }

    #[test]
    fn bare_object_without_code_passes_through() {
        let body = json!({"foo":"bar"});
        let data = unwrap_envelope(body).unwrap();
        assert_eq!(data["foo"], "bar");
    }

    #[test]
    fn client_trims_trailing_slash_and_defaults_identity() {
        let c = CmxServiceClient::new("http://127.0.0.1:8091/");
        assert_eq!(c.base_url, "http://127.0.0.1:8091");
        assert_eq!(c.tenant, "default");
        assert_eq!(c.user, "admin");
    }
}
