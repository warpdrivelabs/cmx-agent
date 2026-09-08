//! IM 账号绑定客户端：桌面登录用户 ↔ IM 平台身份（飞书 open_id / 微信 union_id / 钉钉 …）的绑定。
//!
//! 绑定流程（验证码模式）：
//! 1. 桌面端登录后 `gen_code`（Bearer）→ 门户为该 user_id 生成一次性 6 位码（TTL 5min）。
//! 2. 用户把码发到 IM 机器人；IM 桥收到后 `verify(provider, open_id, code)`（无鉴权，白名单端点）→
//!    门户核对并核销码 → 建立 `user_id ↔ (provider, open_id)` 绑定 → 返回 `{user_id, username, roles}`。
//! 3. 之后每条 IM 消息，桥调 `lookup(provider, open_id)`（无鉴权）解析发送者身份，
//!    以绑定用户跑回合（数据权限/守卫按此人判定）；未绑定 → 提示绑定。
//!
//! 端点（门户 `{base}/api/im/bindings/*`，`{code,msg,data}` 信封）：
//! - `POST code`（Bearer）：生成验证码。
//! - `POST verify`（白名单）：核销验证码 + 建绑定 + 返回身份。
//! - `GET lookup?provider=&open_id=`（白名单）：按 IM 身份查绑定（未绑定返回 Envelope 错误）。
//! - `GET /`（Bearer）：列当前登录用户的绑定。
//! - `DELETE /{provider}/{open_id}`（Bearer）：解绑。

use serde::Serialize;
use serde_json::{Value, json};

use crate::client::{ClientError, CmxServiceClient};

/// 一条已建立的 IM 绑定（门户侧权威记录的投影）。
#[derive(Debug, Clone, Serialize)]
pub struct ImBinding {
    pub user_id: String,
    pub provider: String,
    pub open_id: String,
    /// 绑定时间（门户 epoch 毫秒；缺失为 0）。
    pub created_at: i64,
}

/// 绑定解析出的门户身份：IM 消息按此人跑回合（Subject.user=user_id, roles=roles）。
#[derive(Debug, Clone, Serialize)]
pub struct BoundIdentity {
    pub user_id: String,
    pub username: String,
    pub roles: Vec<String>,
}

/// IM 绑定客户端（对门户 `/api/im/bindings/*`）。
#[derive(Debug, Clone)]
pub struct ImBindingClient {
    client: CmxServiceClient,
}

impl ImBindingClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: CmxServiceClient::new(base_url),
        }
    }

    /// 生成绑定验证码（Bearer：当前登录用户）。返回 `(code, expires_in 秒)`。
    pub async fn gen_code(&self, token: &str) -> Result<(String, u64), ClientError> {
        let data = self
            .client
            .post_data_bearer("/api/im/bindings/code", json!({}), token)
            .await?;
        let code = data
            .get("code")
            .and_then(|c| c.as_str())
            .ok_or_else(|| ClientError::Decode("绑定验证码响应缺 code".into()))?
            .to_string();
        let expires_in = data.get("expires_in").and_then(|e| e.as_u64()).unwrap_or(300);
        Ok((code, expires_in))
    }

    /// 核销验证码并建立绑定（无鉴权白名单端点，由 IM 桥调用）。返回绑定出的身份。
    pub async fn verify(
        &self,
        provider: &str,
        open_id: &str,
        code: &str,
    ) -> Result<BoundIdentity, ClientError> {
        let data = self
            .client
            .post_data(
                "/api/im/bindings/verify",
                json!({ "provider": provider, "open_id": open_id, "code": code }),
            )
            .await?;
        parse_identity(&data)
    }

    /// 按 IM 身份查绑定（无鉴权白名单端点）。未绑定 → `Err(Envelope{code:40401})`。
    pub async fn lookup(&self, provider: &str, open_id: &str) -> Result<BoundIdentity, ClientError> {
        let data = self
            .client
            .get_data(&format!(
                "/api/im/bindings/lookup?provider={provider}&open_id={open_id}"
            ))
            .await?;
        parse_identity(&data)
    }

    /// 列当前登录用户（Bearer）的绑定。
    pub async fn list(&self, token: &str) -> Result<Vec<ImBinding>, ClientError> {
        let data = self.client.get_data_bearer("/api/im/bindings", token).await?;
        let items = data
            .get("items")
            .or_else(|| data.get("bindings"))
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(items
            .iter()
            .map(|b| ImBinding {
                user_id: str_of(b, "user_id"),
                provider: str_of(b, "provider"),
                open_id: str_of(b, "open_id"),
                created_at: b.get("created_at").and_then(|c| c.as_i64()).unwrap_or(0),
            })
            .collect())
    }

    /// 解绑（Bearer，仅本人）。
    pub async fn unbind(&self, token: &str, provider: &str, open_id: &str) -> Result<(), ClientError> {
        self.client
            .delete_data_bearer(&format!("/api/im/bindings/{provider}/{open_id}"), token)
            .await?;
        Ok(())
    }
}

/// 从 verify/lookup 响应解析 `{user_id, username, roles}`。
fn parse_identity(data: &Value) -> Result<BoundIdentity, ClientError> {
    let user_id = data
        .get("user_id")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        })
        .filter(|s| !s.is_empty())
        .ok_or_else(|| ClientError::Decode("绑定身份响应缺 user_id".into()))?;
    Ok(BoundIdentity {
        username: str_of(data, "username"),
        roles: data
            .get("roles")
            .and_then(|r| r.as_array())
            .map(|a| a.iter().filter_map(|r| r.as_str().map(String::from)).collect())
            .unwrap_or_default(),
        user_id,
    })
}

fn str_of(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_identity_shapes() {
        let id = parse_identity(&json!({
            "user_id": 123456789012345678i64,
            "username": "admin",
            "roles": ["admin", "user"]
        }))
        .unwrap();
        assert_eq!(id.user_id, "123456789012345678"); // 雪花 id 转 String
        assert_eq!(id.username, "admin");
        assert_eq!(id.roles, vec!["admin", "user"]);

        let id2 = parse_identity(&json!({ "user_id": "u1", "username": "bob" })).unwrap();
        assert_eq!(id2.user_id, "u1");
        assert!(id2.roles.is_empty());

        assert!(parse_identity(&json!({ "username": "nouser" })).is_err());
    }
}
