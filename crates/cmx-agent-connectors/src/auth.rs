//! 用户认证提供者（照抄 CMXPortalManager `src/lib/auth.js` 的口径）。
//!
//! POST `{base}/api/auth/login` `{username,password,device_type,device_id}` → `{code,msg,data}` 信封，
//! `data` 含 `access_token`/`refresh_token`；再带 Bearer 调 `/api/auth/me` 取用户身份。
//! 认证服务地址可配（默认门户 `:8080`）。与连接器同用 [`CmxServiceClient`]，reqwest 隔离在本 crate。

use serde_json::{Value, json};

use crate::client::{ClientError, CmxServiceClient};

/// 认证服务配置（缺省指向本机门户 `:8080`，即 CMXPortalManager 对接的同一服务）。
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub base_url: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:8080".into(),
        }
    }
}

/// 登录成功后的用户身份（+ 令牌，令牌仅后端持有，不下发前端）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct LoggedInUser {
    pub user_id: String,
    pub username: String,
    pub nickname: Option<String>,
    pub roles: Vec<String>,
    /// access_token（后端持有，用于后续带 Bearer 调 cmx 服务；`skip` 不进前端 JSON）。
    #[serde(skip)]
    pub access_token: String,
}

impl LoggedInUser {
    /// 前端可见的用户信息（不含令牌）。
    pub fn public_json(&self) -> Value {
        json!({
            "user_id": self.user_id,
            "username": self.username,
            "nickname": self.nickname,
            "roles": self.roles,
        })
    }
}

/// 认证提供者。
#[derive(Debug, Clone)]
pub struct AuthProvider {
    client: CmxServiceClient,
}

impl AuthProvider {
    pub fn new(cfg: AuthConfig) -> Self {
        Self {
            client: CmxServiceClient::new(cfg.base_url),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.client.base_url
    }

    /// 登录：POST /api/auth/login → 取 token → GET /api/auth/me 取身份。
    pub async fn login(&self, username: &str, password: &str) -> Result<LoggedInUser, ClientError> {
        let device_id = format!("desktop-{}", short_rand());
        let body = json!({
            "username": username,
            "password": password,
            "device_type": "desktop",
            "device_id": device_id,
        });
        let data = self.client.post_data("/api/auth/login", body).await?;
        let access_token = data
            .get("access_token")
            .and_then(|t| t.as_str())
            .ok_or_else(|| ClientError::Decode("登录响应缺少 access_token".into()))?
            .to_string();

        // 带 Bearer 取用户身份；失败不致命（至少已登录），用 username 兜底。
        let me = self
            .client
            .get_data_bearer("/api/auth/me", &access_token)
            .await
            .unwrap_or(Value::Null);

        Ok(LoggedInUser {
            user_id: json_str(&me, "user_id"),
            username: me
                .get("username")
                .and_then(|v| v.as_str())
                .unwrap_or(username)
                .to_string(),
            nickname: me
                .get("nickname")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string()),
            roles: me
                .get("roles")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|r| r.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            access_token,
        })
    }
}

/// 读一个可能是数字或字符串的字段为 String（user_id 为 52+ 位雪花 id，超 JS 安全整数，一律转字符串）。
fn json_str(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

/// 8 位随机 device_id 后缀（无需强随机；仅区分设备记录）。
fn short_rand() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos:08x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_base_is_portal() {
        assert_eq!(AuthConfig::default().base_url, "http://127.0.0.1:8080");
    }

    #[test]
    fn public_json_omits_token() {
        let u = LoggedInUser {
            user_id: "123".into(),
            username: "admin".into(),
            nickname: Some("Super Admin".into()),
            roles: vec!["admin".into()],
            access_token: "SECRET".into(),
        };
        let j = u.public_json();
        assert_eq!(j["username"], "admin");
        assert_eq!(j["user_id"], "123");
        assert!(!j.to_string().contains("SECRET"));
    }

    #[test]
    fn logged_in_user_serialize_skips_token() {
        let u = LoggedInUser {
            user_id: "1".into(),
            username: "a".into(),
            nickname: None,
            roles: vec![],
            access_token: "SECRET".into(),
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(!s.contains("SECRET"));
        assert!(!s.contains("access_token"));
    }
}
