//! 用户认证提供者（照抄 CMXPortalManager `src/lib/auth.js` 的口径）。
//!
//! POST `{base}/api/auth/login` `{username,password,device_type,device_id}` → `{code,msg,data}` 信封，
//! `data` 含 `access_token`/`refresh_token`；再带 Bearer 调 `/api/auth/me` 取用户身份。
//! 认证服务地址可配（默认门户 `:8080`）。与连接器同用 [`CmxServiceClient`]，reqwest 隔离在本 crate。

use serde_json::{Value, json};

use crate::client::{ClientError, CmxServiceClient};

/// 认证服务配置（缺省指向团队门户 `192.168.137.111:8080`，本机开发门户同端口）。
#[derive(Debug, Clone)]
pub struct AuthConfig {
    pub base_url: String,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            base_url: "http://192.168.137.111:8080".into(),
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
    /// 初始密码未修改标志（门户登录响应透出；true 时前端弹框提醒改密）。
    pub must_change_password: bool,
    /// access_token（后端持有，用于后续带 Bearer 调 cmx 服务；`skip` 不进前端 JSON）。
    #[serde(skip)]
    pub access_token: String,
    /// refresh_token（后端持有；会话落盘 + 到期续签用，`skip` 不进前端 JSON）。
    #[serde(skip)]
    pub refresh_token: String,
    /// access_token 过期时间（Unix 秒；0 = 未知，仅落盘展示用）。
    #[serde(skip)]
    pub access_expires_at: i64,
    /// refresh_token 过期时间（Unix 秒；0 = 未知）。
    #[serde(skip)]
    pub refresh_expires_at: i64,
}

impl LoggedInUser {
    /// 前端可见的用户信息（不含令牌）。
    pub fn public_json(&self) -> Value {
        json!({
            "user_id": self.user_id,
            "username": self.username,
            "nickname": self.nickname,
            "roles": self.roles,
            "must_change_password": self.must_change_password,
        })
    }
}

/// 刷新令牌返回的新令牌对（refresh 轮换：旧 refresh_token 一次性作废）。
#[derive(Debug, Clone)]
pub struct TokenPair {
    pub access_token: String,
    pub refresh_token: String,
    pub access_expires_at: i64,
    pub refresh_expires_at: i64,
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
        let me = self.me(&access_token).await.unwrap_or(Value::Null);
        let mut user = user_from_me(&me, username, access_token);
        // 令牌与过期时间取自登录响应（me 不含）；初始密码标志以登录响应为准（me 已兜底）。
        user.refresh_token = json_str(&data, "refresh_token");
        user.access_expires_at = data
            .get("access_expires_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        user.refresh_expires_at = data
            .get("refresh_expires_at")
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        if let Some(b) = json_bool(&data, "must_change_password") {
            user.must_change_password = b;
        }
        Ok(user)
    }

    /// GET /api/auth/me（Bearer）→ 原始 data。
    pub async fn me(&self, access_token: &str) -> Result<Value, ClientError> {
        self.client.get_data_bearer("/api/auth/me", access_token).await
    }

    /// 刷新令牌：POST /api/auth/refresh `{refresh_token}` → 新令牌对（refresh 轮换，旧的一次性作废）。
    pub async fn refresh(&self, refresh_token: &str) -> Result<TokenPair, ClientError> {
        let data = self
            .client
            .post_data("/api/auth/refresh", json!({ "refresh_token": refresh_token }))
            .await?;
        Ok(TokenPair {
            access_token: json_str(&data, "access_token"),
            refresh_token: json_str(&data, "refresh_token"),
            access_expires_at: data
                .get("access_expires_at")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
            refresh_expires_at: data
                .get("refresh_expires_at")
                .and_then(|v| v.as_i64())
                .unwrap_or(0),
        })
    }

    /// 修改密码：POST /api/auth/change-password `{old_password,new_password}`（Bearer 当前 token）。
    /// 门户改密成功即吊销该用户全部 token（含当前会话）——调用方随后必须重新登录。
    pub async fn change_password(
        &self,
        access_token: &str,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), ClientError> {
        let body = json!({
            "old_password": old_password,
            "new_password": new_password,
        });
        self.client
            .post_data_bearer("/api/auth/change-password", body, access_token)
            .await?;
        Ok(())
    }
}

/// 由 /api/auth/me 响应组装用户（token 由调用方附加；me 缺字段时回退 `fallback_username`）。
pub fn user_from_me(me: &Value, fallback_username: &str, access_token: String) -> LoggedInUser {
    LoggedInUser {
        user_id: json_str(me, "user_id"),
        username: me
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or(fallback_username)
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
        must_change_password: json_bool(me, "must_change_password").unwrap_or(false),
        access_token,
        refresh_token: String::new(),
        access_expires_at: 0,
        refresh_expires_at: 0,
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

/// 读一个 bool 字段（兼容 bool / 0|1 数字编码）。字段缺失返回 None。
fn json_bool(v: &Value, key: &str) -> Option<bool> {
    match v.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        Some(Value::Number(n)) => Some(n.as_i64().unwrap_or(0) != 0),
        _ => None,
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
        assert_eq!(AuthConfig::default().base_url, "http://192.168.137.111:8080");
    }

    #[test]
    fn public_json_omits_token() {
        let u = LoggedInUser {
            user_id: "123".into(),
            username: "admin".into(),
            nickname: Some("Super Admin".into()),
            roles: vec!["admin".into()],
            must_change_password: true,
            access_token: "SECRET".into(),
            refresh_token: "R-SECRET".into(),
            access_expires_at: 0,
            refresh_expires_at: 0,
        };
        let j = u.public_json();
        assert_eq!(j["username"], "admin");
        assert_eq!(j["user_id"], "123");
        assert_eq!(j["must_change_password"], true);
        assert!(!j.to_string().contains("SECRET"));
    }

    #[test]
    fn logged_in_user_serialize_skips_token() {
        let u = LoggedInUser {
            user_id: "1".into(),
            username: "a".into(),
            nickname: None,
            roles: vec![],
            must_change_password: false,
            access_token: "SECRET".into(),
            refresh_token: "R-SECRET".into(),
            access_expires_at: 0,
            refresh_expires_at: 0,
        };
        let s = serde_json::to_string(&u).unwrap();
        assert!(!s.contains("SECRET"));
        assert!(!s.contains("access_token"));
    }

    #[test]
    fn user_from_me_maps_fields() {
        let me = json!({
            "user_id": 42, "username": "u1", "nickname": "Nick",
            "roles": ["r1", "r2"], "must_change_password": 1,
        });
        let u = user_from_me(&me, "fallback", "tok".into());
        assert_eq!(u.user_id, "42");
        assert_eq!(u.username, "u1");
        assert_eq!(u.nickname.as_deref(), Some("Nick"));
        assert_eq!(u.roles, vec!["r1".to_string(), "r2".to_string()]);
        assert!(u.must_change_password);
        assert_eq!(u.access_token, "tok");
    }
}
