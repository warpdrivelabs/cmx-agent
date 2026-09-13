//! U13 数据权限 PEP：把工具的 `requires_auth`（如 `flow:write`）解析为 cmx-data-auth 的
//! `POST /decide {subject, resource{kind,action}}` 判定，缓存结果，供 [`cmx_agent_core::AuthGuard`] 的
//! **同步**闭包读取（Guard::check 是同步的，故判定异步预热 + 同步查缓存）。
//!
//! 判定缓存 `(subject.user, perm) -> allowed`；`prewarm` 在装配/登录时一次性拉全部写权限判定。
//! **fail-closed**：`enforce=true` 时缓存未命中即拒绝（宁可挡住也不误放行）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cmx_agent_core::guard::Subject;
use serde_json::json;

use crate::client::CmxServiceClient;

/// 判定缓存 TTL：到期视为未命中重新判定——权限在门户侧被收回后，进程内
/// 旧缓存最多再放行 TTL 窗口（旧实现缓存终身有效，直到重启）。
pub const PEP_CACHE_TTL: Duration = Duration::from_secs(300);

/// 把 `scope:action` 权限串映射为 PDP 的 (kind, action)。
/// action 归一到 read/write/execute；kind 用 scope（flow/onto/report/fs/net…）。
fn perm_to_resource(perm: &str) -> (String, String) {
    let (scope, act) = match perm.split_once(':') {
        Some((s, a)) => (s, a),
        // 无冒号权限（exec 等）按「执行」判——旧实现退化为 read，"允许读"类策略
        // 即可放行 shell 执行，判定语义与策略作者意图错位。
        None => (perm, "exec"),
    };
    let action = match act {
        "read" => "read",
        "write" => "write",
        "exec" | "execute" => "execute",
        "browser" | "fetch" | "search" => "read", // 联网读类
        _ => "read",
    };
    (scope.to_string(), action.to_string())
}

/// 判定缓存：(主体, 权限) → (放行?, 写入时刻)。
type PepCache = Arc<Mutex<HashMap<(String, String), (bool, Instant)>>>;

/// 数据权限 PEP：持 dataauth 客户端 + 判定缓存（带 TTL，见 [`PEP_CACHE_TTL`]）。
#[derive(Clone)]
pub struct DataAuthPep {
    client: Arc<CmxServiceClient>,
    cache: PepCache,
    /// true=严格执行（缓存未命中拒绝）；false=宽松（未命中放行，仅缓存已知拒绝）。
    enforce: bool,
}

impl DataAuthPep {
    /// `base_url` 指 cmx-data-auth（如 http://127.0.0.1:8098）；判定打 `POST {base}/api/dataauth/v1/decide`。
    pub fn new(base_url: impl Into<String>, tenant: impl Into<String>, user: impl Into<String>, enforce: bool) -> Self {
        let client = CmxServiceClient::new(base_url).with_identity(tenant, user);
        Self {
            client: Arc::new(client),
            cache: Arc::new(Mutex::new(HashMap::new())),
            enforce,
        }
    }

    /// 调 PDP 判定一个 (subject, perm)，写缓存并返回是否放行。网络失败：enforce 下拒绝、否则放行。
    pub async fn decide(&self, subject: &Subject, perm: &str) -> bool {
        let (kind, action) = perm_to_resource(perm);
        let body = json!({
            "subject": { "userId": subject.user, "roles": subject.roles, "orgs": [] },
            "resource": { "kind": kind, "action": action }
        });
        let allowed = match self.client.post_write("/api/dataauth/v1/decide", &body).await {
            Ok(d) => {
                let eff = d.get("effect").and_then(|e| e.as_str()).unwrap_or("");
                eff.eq_ignore_ascii_case("permit") || eff.eq_ignore_ascii_case("allow")
            }
            Err(_) => !self.enforce, // 判定服务不可达：严格模式拒绝、宽松模式放行
        };
        self.cache
            .lock()
            .expect("pep cache")
            .insert((subject.user.clone(), perm.to_string()), (allowed, Instant::now()));
        allowed
    }

    /// 预热：对给定权限集逐个判定并入缓存（装配/登录后调，避免首次工具调用阻塞）。
    pub async fn prewarm(&self, subject: &Subject, perms: &[&str]) {
        for p in perms {
            let _ = self.decide(subject, p).await;
        }
    }

    /// 同步查缓存（供 AuthGuard 闭包）。**只对写/敏感权限（[`ENFORCED_PERMS`]）接地**；
    /// 其余（读类 `*:read`/`net:*` 等）默认放行——不预热、不阻断（数据权限门只管写侧）。
    /// 被接地的权限未命中缓存时：enforce 下拒绝、否则放行（fail-closed/open）。
    pub fn cached_allow(&self, subject: &Subject, perm: &str) -> bool {
        if !ENFORCED_PERMS.contains(&perm) {
            return true; // 读类/非敏感：默认放行
        }
        let fresh = |v: Option<&(bool, Instant)>| match v {
            Some((allowed, at)) if at.elapsed() < PEP_CACHE_TTL => Some(*allowed),
            _ => None, // 未命中或已过期：按未命中走 fail-closed/open
        };
        match fresh(
            self.cache
                .lock()
                .expect("pep cache")
                .get(&(subject.user.clone(), perm.to_string())),
        ) {
            Some(v) => v,
            None => !self.enforce,
        }
    }
}

/// 所有需接地的写/敏感权限（预热用）。读类默认放行，不必预热。
pub const ENFORCED_PERMS: &[&str] = &["flow:write", "onto:write", "report:write", "exec", "fs:write"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn perm_maps_to_kind_action() {
        assert_eq!(perm_to_resource("flow:write"), ("flow".into(), "write".into()));
        assert_eq!(perm_to_resource("onto:read"), ("onto".into(), "read".into()));
        assert_eq!(perm_to_resource("exec"), ("exec".into(), "execute".into())); // 无冒号按执行判
        assert_eq!(perm_to_resource("net:fetch"), ("net".into(), "read".into()));
    }

    #[test]
    fn reads_default_allow_under_enforce() {

        // 读类/非敏感权限即使 enforce 也默认放行（不阻断读）。
        let subj = Subject::new("bob");
        let strict = DataAuthPep::new("http://127.0.0.1:9", "default", "bob", true);
        assert!(strict.cached_allow(&subj, "flow:read"), "读应默认放行");
        assert!(strict.cached_allow(&subj, "net:fetch"), "net 应默认放行");
        assert!(!strict.cached_allow(&subj, "flow:write"), "写未命中严格应拒绝");
        }

    #[test]
    fn fail_closed_vs_open_on_cache_miss() {
        let subj = Subject::new("alice");
        let strict = DataAuthPep::new("http://127.0.0.1:9", "default", "alice", true);
        assert!(!strict.cached_allow(&subj, "flow:write"), "严格模式未命中应拒绝");
        let loose = DataAuthPep::new("http://127.0.0.1:9", "default", "alice", false);
        assert!(loose.cached_allow(&subj, "flow:write"), "宽松模式未命中应放行");
    }

    #[test]
    fn expired_cache_entry_is_treated_as_miss() {
        // 权限收回后旧 allow 不得终身放行：过期条目按未命中走（严格拒绝 / 宽松放行）。
        let subj = Subject::new("carol");
        let strict = DataAuthPep::new("http://127.0.0.1:9", "default", "carol", true);
        let key = (subj.user.clone(), "flow:write".to_string());
        strict
            .cache
            .lock()
            .expect("pep cache")
            .insert(key.clone(), (true, Instant::now() - PEP_CACHE_TTL - Duration::from_secs(1)));
        assert!(!strict.cached_allow(&subj, "flow:write"), "过期 allow 应按未命中拒绝");
        strict
            .cache
            .lock()
            .expect("pep cache")
            .insert(key.clone(), (true, Instant::now()));
        assert!(strict.cached_allow(&subj, "flow:write"), "新鲜 allow 应放行");
    }
}
