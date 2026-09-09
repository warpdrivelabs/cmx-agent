//! IM 用户绑定解析：发送者 IM 身份（飞书 open_id）↔ 门户用户。
//!
//! [`ImBindingResolver`] 是桥用的小 trait（`lookup` 查绑定 / `verify` 验证码建绑定），
//! 两个实现：
//! - [`PortalBindingResolver`]：真实现，调门户 `/api/agent/bindings/*`（[`cmx_agent_connectors::ImBindingClient`]）。
//! - [`MockBindingResolver`]：内存实现，桥的绑定流程测试用。
//!
//! 语义约定（对齐门户契约）：
//! - `Ok(Some(id))`：已绑定/绑定成功 → 桥以 `id`（user_id+roles）跑回合。
//! - `Ok(None)`：未绑定 / 验证码无效 → 桥回绑定提示。
//! - `Err`：绑定服务不可达等 → 桥回服务不可用提示（fail-closed，不跑回合）。

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use cmx_agent_connectors::im_binding::{BoundIdentity, ImBindingClient};

/// 绑定解析器：桥按发送者 IM 身份查/建绑定。
#[async_trait]
pub trait ImBindingResolver: Send + Sync {
    /// 查绑定。`Ok(None)` = 该 IM 身份未绑定。
    async fn lookup(&self, provider: &str, open_id: &str) -> Result<Option<BoundIdentity>, String>;

    /// 验证码建绑定（核销码）。`Ok(None)` = 码无效/过期。
    async fn verify(
        &self,
        provider: &str,
        open_id: &str,
        code: &str,
    ) -> Result<Option<BoundIdentity>, String>;
}

/// 真实现：调门户 `/api/agent/bindings/*`。`base_url` 指门户（默认 `http://127.0.0.1:8080`）。
pub struct PortalBindingResolver {
    client: ImBindingClient,
}

impl PortalBindingResolver {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: ImBindingClient::new(base_url),
        }
    }
}

#[async_trait]
impl ImBindingResolver for PortalBindingResolver {
    async fn lookup(&self, provider: &str, open_id: &str) -> Result<Option<BoundIdentity>, String> {
        match self.client.lookup(provider, open_id).await {
            Ok(id) => Ok(Some(id)),
            // 门户契约：未绑定 → Envelope{code:40401}
            Err(cmx_agent_connectors::ClientError::Envelope { code: 40401, .. }) => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }

    async fn verify(
        &self,
        provider: &str,
        open_id: &str,
        code: &str,
    ) -> Result<Option<BoundIdentity>, String> {
        match self.client.verify(provider, open_id, code).await {
            Ok(id) => Ok(Some(id)),
            // 码无效/过期 → 未绑定语义（桥回绑定提示）
            Err(cmx_agent_connectors::ClientError::Envelope { code, .. }) if code != 0 => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 内存 mock：桥绑定流程测试用。`codes` = 待核销验证码 → 绑定出的身份；
/// `lookup` 查 `bound`（open_id → 身份）。
pub struct MockBindingResolver {
    /// 已绑定：open_id → 身份。
    bound: Mutex<HashMap<String, BoundIdentity>>,
    /// 有效验证码：code → 身份（verify 成功即核销）。
    codes: Mutex<HashMap<String, BoundIdentity>>,
    /// lookup 全部失败（模拟门户不可达）。
    pub broken: bool,
}

impl MockBindingResolver {
    pub fn new() -> Self {
        Self {
            bound: Mutex::new(HashMap::new()),
            codes: Mutex::new(HashMap::new()),
            broken: false,
        }
    }

    /// 预置一条已绑定。
    pub fn with_bound(self, open_id: &str, id: BoundIdentity) -> Self {
        self.bound
            .lock()
            .unwrap()
            .insert(open_id.into(), id);
        self
    }

    /// 预置一个有效验证码。
    pub fn with_code(self, code: &str, id: BoundIdentity) -> Self {
        self.codes.lock().unwrap().insert(code.into(), id);
        self
    }
}

impl Default for MockBindingResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ImBindingResolver for MockBindingResolver {
    async fn lookup(&self, _provider: &str, open_id: &str) -> Result<Option<BoundIdentity>, String> {
        if self.broken {
            return Err("绑定服务不可达（mock）".into());
        }
        Ok(self.bound.lock().unwrap().get(open_id).cloned())
    }

    async fn verify(
        &self,
        _provider: &str,
        open_id: &str,
        code: &str,
    ) -> Result<Option<BoundIdentity>, String> {
        if self.broken {
            return Err("绑定服务不可达（mock）".into());
        }
        let Some(id) = self.codes.lock().unwrap().remove(code) else {
            return Ok(None);
        };
        self.bound.lock().unwrap().insert(open_id.into(), id.clone());
        Ok(Some(id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(user: &str, roles: &[&str]) -> BoundIdentity {
        BoundIdentity {
            user_id: user.into(),
            username: user.into(),
            roles: roles.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[tokio::test]
    async fn mock_lookup_and_verify_flow() {
        let m = MockBindingResolver::new()
            .with_bound("ou_a", id("u1", &["admin"]))
            .with_code("888888", id("u2", &[]));

        // 已绑定 → lookup 命中
        let got = m.lookup("feishu", "ou_a").await.unwrap().unwrap();
        assert_eq!(got.user_id, "u1");

        // 未绑定 → None
        assert!(m.lookup("feishu", "ou_b").await.unwrap().is_none());

        // 验证码建绑定：ou_b 发 888888 → 绑上 u2；码核销（再用失败）
        let got = m.verify("feishu", "ou_b", "888888").await.unwrap().unwrap();
        assert_eq!(got.user_id, "u2");
        assert!(m.lookup("feishu", "ou_b").await.unwrap().is_some());
        assert!(m.verify("feishu", "ou_c", "888888").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mock_broken_fails_closed() {
        let mut m = MockBindingResolver::new();
        m.broken = true;
        assert!(m.lookup("feishu", "ou_a").await.is_err());
        assert!(m.verify("feishu", "ou_a", "123456").await.is_err());
    }
}
