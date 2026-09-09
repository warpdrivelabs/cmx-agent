//! IM 账号绑定客户端：桌面登录用户 ↔ IM 平台身份（飞书 open_id / 微信 union_id / 钉钉 …）的绑定。
//!
//! 绑定流程（验证码模式）：
//! 1. 桌面端登录后 `gen_code`（Bearer）→ 门户为该 user_id 生成一次性 6 位码（TTL 5min）。
//! 2. 用户把码发到 IM 机器人；IM 桥收到后 `verify(provider, open_id, code)`（无鉴权，白名单端点）→
//!    门户核对并核销码 → 建立 `user_id ↔ (provider, open_id)` 绑定 → 返回 `{user_id, username, roles}`。
//! 3. 之后每条 IM 消息，桥调 `lookup(provider, open_id)`（无鉴权）解析发送者身份，
//!    以绑定用户跑回合（数据权限/守卫按此人判定）；未绑定 → 提示绑定。
//!
//! 端点（门户 `{base}/api/agent/bindings/*`，`{code,msg,data}` 信封）：
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
    /// 绑定的 IM 端用户名（门户可能回填，如飞书用户名；缺失为空串，前端自行降级）。
    #[serde(default)]
    pub im_username: String,
}

/// 绑定解析出的门户身份：IM 消息按此人跑回合（Subject.user=user_id, roles=roles）。
#[derive(Debug, Clone, Serialize)]
pub struct BoundIdentity {
    pub user_id: String,
    pub username: String,
    pub roles: Vec<String>,
}

/// IM 绑定客户端（对门户 `/api/agent/bindings/*`）。
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
            .post_data_bearer("/api/agent/bindings/code", json!({}), token)
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
                "/api/agent/bindings/verify",
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
                "/api/agent/bindings/lookup?provider={provider}&open_id={open_id}"
            ))
            .await?;
        parse_identity(&data)
    }

    /// 列当前登录用户（Bearer）的绑定。
    pub async fn list(&self, token: &str) -> Result<Vec<ImBinding>, ClientError> {
        let data = self.client.get_data_bearer("/api/agent/bindings", token).await?;
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
                im_username: opt_str(b, "im_username")
                    .or_else(|| opt_str(b, "nickname"))
                    .unwrap_or_default(),
            })
            .collect())
    }

    /// 解绑（Bearer，仅本人）。
    pub async fn unbind(&self, token: &str, provider: &str, open_id: &str) -> Result<(), ClientError> {
        self.client
            .delete_data_bearer(&format!("/api/agent/bindings/{provider}/{open_id}"), token)
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

/// 同 `str_of` 但返回 `Option`（用于 `.or_else` 链式回退取多个候选字段）。
fn opt_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .map(String::from)
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

    #[test]
    fn list_parses_im_username_with_fallback() {
        // 门户 list 端点返回的 item 形态：取 im_username，缺失时回退 nickname，都没有则空串。
        let env = json!({
            "code": 0,
            "data": {
                "items": [
                    { "user_id":"u1","provider":"feishu","open_id":"ou_abc","created_at":1700000000000i64,"im_username":"张三" },
                    { "user_id":"u2","provider":"telegram","open_id":"tg_xyz","nickname":"Bob" },
                    { "user_id":"u3","provider":"feishu","open_id":"ou_def" }
                ]
            }
        });
        // 直接验证 item → ImBinding 的映射逻辑（绕过 HTTP，用 list 内部同样的 map 规则）。
        let items = env["data"]["items"].as_array().unwrap();
        let bindings: Vec<ImBinding> = items.iter().map(|b| ImBinding {
            user_id: str_of(b, "user_id"),
            provider: str_of(b, "provider"),
            open_id: str_of(b, "open_id"),
            created_at: b.get("created_at").and_then(|c| c.as_i64()).unwrap_or(0),
            im_username: opt_str(b, "im_username").or_else(|| opt_str(b, "nickname")).unwrap_or_default(),
        }).collect();
        assert_eq!(bindings.len(), 3);
        assert_eq!(bindings[0].im_username, "张三");
        assert_eq!(bindings[1].im_username, "Bob");   // 回退 nickname
        assert_eq!(bindings[2].im_username, "");       // 都没有 → 空串
        assert_eq!(bindings[0].created_at, 1700000000000i64);
    }

    /// live 端到端联调：打真实门户 :8080，覆盖绑定全流程。
    /// 标 `#[ignore]`：需 `cmx-portal-server` 运行 + 主库可达，手动跑：
    ///   CMX_AGENT_PORTAL_BASE=http://127.0.0.1:8080 \
    ///   CMX_AGENT_TEST_USER=admin CMX_AGENT_TEST_PASS=Admin@12345 \
    ///   cargo test -p cmx-agent-connectors --test im_binding_live -- --ignored --nocapture
    #[cfg(test)]
    #[ignore = "需真实门户 + 主库，手动跑 live 联调"]
    #[tokio::test]
    async fn live_binding_roundtrip() {
        use crate::auth::{AuthConfig, AuthProvider};

        let base = std::env::var("CMX_AGENT_PORTAL_BASE")
            .unwrap_or_else(|_| "http://127.0.0.1:8080".to_string());
        let user = std::env::var("CMX_AGENT_TEST_USER").unwrap_or_else(|_| "admin".into());
        let pass = std::env::var("CMX_AGENT_TEST_PASS").unwrap_or_else(|_| "Admin@12345".into());

        // 0. 登录取 token（桌面端视角，AuthProvider: login → access_token）
        let auth = AuthProvider::new(AuthConfig { base_url: base.clone() });
        let logged = auth.login(&user, &pass).await.expect("登录失败");
        let token = &logged.access_token;
        let expected_user_id = &logged.user_id;
        assert!(!token.is_empty(), "token 为空");
        assert!(!expected_user_id.is_empty(), "user_id 为空");

        let client = ImBindingClient::new(&base);
        let provider = "feishu";
        let open_id = format!("ou_e2e_live_{}", std::process::id());

        // 1. lookup 未绑定 → Envelope{code:40401}
        match client.lookup(provider, &open_id).await {
            Err(ClientError::Envelope { code: 40401, .. }) => {}
            other => panic!("未绑定时应 Envelope(40401)，实际：{other:?}"),
        }

        // 2. gen_code（Bearer）→ 6 位码 + expires_in
        let (code, expires_in) = client.gen_code(token).await.expect("取码失败");
        assert_eq!(code.len(), 6, "验证码应 6 位：{code}");
        assert_eq!(expires_in, 300, "TTL 应 300 秒");

        // 3. verify 建绑定 → 返回身份（与登录者一致）
        let id = client
            .verify(provider, &open_id, &code)
            .await
            .expect("verify 失败");
        assert_eq!(id.user_id, *expected_user_id, "user_id 不符");
        assert_eq!(id.username, user, "username 不符");
        assert!(!id.roles.is_empty(), "roles 为空");

        // 4. 二次 verify 同码（单次有效）→ Envelope(40001)
        match client.verify(provider, &open_id, &code).await {
            Err(ClientError::Envelope { code: 40001, .. }) => {}
            other => panic!("已核销码应 Envelope(40001)，实际：{other:?}"),
        }

        // 5. lookup 命中 → 同一身份
        let id2 = client.lookup(provider, &open_id).await.expect("lookup 命中失败");
        assert_eq!(id2.user_id, id.user_id);

        // 6. list 含该绑定
        let list = client.list(token).await.expect("list 失败");
        assert!(
            list.iter().any(|b| b.provider == provider && b.open_id == open_id),
            "list 未包含刚绑定的记录：{list:?}"
        );

        // 7. 幂等：再取码对本人已绑 open_id verify → 幂等成功（不报 40002）
        let (code2, _) = client.gen_code(token).await.expect("取码2失败");
        let id3 = client
            .verify(provider, &open_id, &code2)
            .await
            .expect("幂等 verify 失败");
        assert_eq!(id3.user_id, *expected_user_id);

        // 8. unbind → 成功
        client
            .unbind(token, provider, &open_id)
            .await
            .expect("解绑失败");

        // 9. 解绑后 lookup → 40401
        match client.lookup(provider, &open_id).await {
            Err(ClientError::Envelope { code: 40401, .. }) => {}
            other => panic!("解绑后应 Envelope(40401)，实际：{other:?}"),
        }

        // 10. 重复解绑本人名下已不存在 → 40401
        match client.unbind(token, provider, &open_id).await {
            Err(ClientError::Envelope { code: 40401, .. }) => {}
            other => panic!("重复解绑应 Envelope(40401)，实际：{other:?}"),
        }
    }
}
