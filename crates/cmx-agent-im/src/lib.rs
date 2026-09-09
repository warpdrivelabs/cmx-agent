//! cmx-agent-im（U9 IM 遥控）：把 IM 会话桥接到 [`cmx_agent_app::AgentApp`]——
//! 从 IM 收到消息 → 映射到一个 agent 会话 → 鉴权 → 跑一个回合 → 把回复分段发回 IM。
//!
//! **transport 无关**：核心是 [`ImProvider`] trait（拉取 + 发送）+ [`ImBridge`] 编排。
//! 参考实现 [`TelegramProvider`]（长轮询）；企业微信/飞书/钉钉/自建网关按同一 trait 追加即可。
//!
//! **用户绑定**：配了 [`ImBindingResolver`]（`ImBridge::with_bindings`）时按 **发送者**（飞书 open_id）
//! 鉴权——已绑定 → 以绑定用户身份跑回合（`AgentApp::send_as`，数据权限按此人）；未绑定 → 回提示；
//! 消息文本恰为有效验证码 → 完成绑定。未配 resolver = 白名单模式（`allow`，按 chat_id）。
//!
//! **安全**：IM 可驱动 agent 跑工具（bash/写文件…）→ 白名单模式务必配 `allow`（不配即拒，
//! 显式放开须 `CMX_AGENT_IM_NO_ALLOW=1`，仅测试/纯内网）；绑定模式的门是「已绑定身份」
//! （fail-closed：门户不可达即拒）。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cmx_agent_app::AgentApp;
use tokio::sync::watch;
pub use cmx_agent_connectors::im_binding::BoundIdentity;

mod telegram;
mod feishu;
mod config;
mod remocon;
pub mod binding;
pub use telegram::TelegramProvider;
pub use feishu::FeishuProvider;
pub use config::{ImConfig, ImKind, parse_allow};
pub use binding::{ImBindingResolver, MockBindingResolver, PortalBindingResolver};
pub use remocon::{
    FeishuCreds, ImRemoconConfig, ResolvedIm, TelegramCreds, env_active, im_config_path,
    load_im_config, resolve, save_im_config, test_feishu,
};

/// 一条入站 IM 消息。
#[derive(Debug, Clone)]
pub struct InboundMsg {
    pub chat_id: String,
    pub text: String,
    /// 传输层游标（如 Telegram update_id）——用于长轮询增量拉取。
    pub update_id: i64,
    /// 发送者在该 IM 平台的稳定标识（飞书 open_id；Telegram 一期填空串）。
    /// 用于 IM 用户 ↔ 门户用户绑定（按 sender 查绑定，以绑定用户身份跑回合）。
    pub sender: String,
}

/// IM 传输 provider：拉新消息 + 发消息。可换 Telegram/企业微信/飞书/自建网关。
#[async_trait]
pub trait ImProvider: Send + Sync {
    /// 拉新消息。`offset`=已处理到的游标；返回 (新消息列表, 下一次的游标)。
    async fn poll(&self, offset: i64) -> Result<(Vec<InboundMsg>, i64), String>;
    /// 发一条消息给某会话。
    async fn send(&self, chat_id: &str, text: &str) -> Result<(), String>;
    /// 启动 provider 的常驻接收链（如飞书 Stream 长连接后台 task）。
    /// 长轮询型 provider（Telegram）无需启动，默认 no-op。
    async fn start(&self) -> Result<(), String> {
        Ok(())
    }
    /// 热重载：请求停止常驻接收链（断开长连接、退出后台 task）。
    /// 有常驻 task 的 provider 覆盖实现；轮询型默认 no-op。
    fn stop(&self) {}
}

/// 绑定模式下一条入站消息的发送者解析结果（决定 tick 对它的处理方式）。
enum SenderResolution {
    /// 已绑定 → 以此身份跑回合。
    Bound(BoundIdentity),
    /// 本条消息就是有效验证码，刚完成绑定 → 回「绑定成功」，不跑回合。
    JustBound(BoundIdentity),
    /// 未绑定 / 验证码无效 → 回绑定提示。
    Unbound,
    /// provider 未提供 sender → 绑定模式无法鉴别（fail-closed）。
    NoSender,
    /// 绑定服务不可达等 → 回服务不可用（不跑回合）。
    ServiceError(String),
}

/// IM 桥：每个 IM 会话映射到一个稳定的 agent 会话，鉴权 → 跑回合 → 分段回复。
pub struct ImBridge {
    app: Arc<AgentApp>,
    provider: Arc<dyn ImProvider>,
    /// provider 类型标签，用于会话命名前缀 `im-<kind>-<chat>`（多 provider 不撞名）。
    kind: String,
    /// None=开放（仅测试/内网）；Some=chat_id 白名单。
    allow: Option<HashSet<String>>,
    /// 用户绑定解析（Some=绑定模式：按 sender 鉴权 + 以绑定用户身份跑回合；None=白名单模式）。
    bindings: Option<Arc<dyn ImBindingResolver>>,
    /// 个人模式（personal）：**忽略 bindings/白名单 sender 鉴权**，所有消息直接以桌面壳
    /// 当前登录用户的身份跑回合（`current_subject()`）。场景 = 自己私聊自己的机器人遥控
    /// 自己的桌面，App ID/Secret 配好 + 登录即用，无需验证码绑定。⚠ 任何能发消息给机器人
    /// 的人都会被当作登录人——安全靠 chat_id 白名单或私聊兜底。
    personal: bool,
    /// open_id → (身份, 缓存时刻) 正缓存（TTL 见 `BINDING_TTL`；解绑后最多 TTL 内失效）。
    binding_cache: Mutex<HashMap<String, (BoundIdentity, Instant)>>,
    offset: AtomicI64,
    sessions: Mutex<HashMap<String, String>>,
}

/// 绑定身份缓存 TTL：平衡「每条消息都查门户」的开销与解绑生效延迟。
const BINDING_TTL: Duration = Duration::from_secs(60);

impl ImBridge {
    /// `kind` = provider 标签（如 `"feishu"`/`"telegram"`），用于会话命名前缀。
    pub fn new(
        app: Arc<AgentApp>,
        provider: Arc<dyn ImProvider>,
        kind: impl Into<String>,
        allow: Option<HashSet<String>>,
    ) -> Self {
        Self {
            app,
            provider,
            kind: kind.into(),
            allow,
            bindings: None,
            personal: false,
            binding_cache: Mutex::new(HashMap::new()),
            offset: AtomicI64::new(0),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 启用用户绑定模式：按发送者（open_id）鉴权——已绑定以绑定用户身份跑回合（`send_as`），
    /// 未绑定回绑定提示，消息恰为有效验证码则完成绑定。白名单退化为可选的会话级二次过滤。
    pub fn with_bindings(mut self, resolver: Arc<dyn ImBindingResolver>) -> Self {
        self.bindings = Some(resolver);
        self
    }

    /// 个人模式：所有 IM 消息直接以桌面壳当前登录用户身份跑回合，跳过绑定/验证码
    /// （`with_bindings` 的绑定解析被短路）。未登录时回提示引导先登录桌面端。
    pub fn with_personal(mut self, on: bool) -> Self {
        self.personal = on;
        self
    }

    fn allowed(&self, chat: &str) -> bool {
        self.allow.as_ref().map(|s| s.contains(chat)).unwrap_or(true)
    }

    /// 查缓存中的绑定身份（未过期才命中）。
    fn cached_identity(&self, open_id: &str) -> Option<BoundIdentity> {
        let mut cache = self.binding_cache.lock().expect("binding cache lock");
        match cache.get(open_id) {
            Some((id, at)) if at.elapsed() < BINDING_TTL => Some(id.clone()),
            Some(_) => {
                cache.remove(open_id); // 过期：清掉
                None
            }
            None => None,
        }
    }

    fn cache_identity(&self, open_id: &str, id: BoundIdentity) {
        self.binding_cache
            .lock()
            .expect("binding cache lock")
            .insert(open_id.to_string(), (id, Instant::now()));
    }

    /// 解析发送者身份（绑定模式）。返回本回合的处理方式。
    async fn resolve_sender(
        &self,
        resolver: &Arc<dyn ImBindingResolver>,
        m: &InboundMsg,
    ) -> SenderResolution {
        if m.sender.is_empty() {
            // provider 没给 sender（如 Telegram 一期）→ 绑定模式无法鉴别 → 拒（fail-closed）。
            return SenderResolution::NoSender;
        }
        if let Some(id) = self.cached_identity(&m.sender) {
            return SenderResolution::Bound(id);
        }
        match resolver.lookup(&self.kind, &m.sender).await {
            Ok(Some(id)) => {
                self.cache_identity(&m.sender, id.clone());
                SenderResolution::Bound(id)
            }
            Ok(None) => {
                // 未绑定：把消息文本当验证码试一次（用户在 IM 里发码完成绑定）。
                let code = m.text.trim();
                match resolver.verify(&self.kind, &m.sender, code).await {
                    Ok(Some(id)) => {
                        tracing::info!(
                            "IM 绑定成功 provider={} open_id={}→user={}",
                            self.kind, m.sender, id.user_id
                        );
                        self.cache_identity(&m.sender, id.clone());
                        SenderResolution::JustBound(id)
                    }
                    Ok(None) => SenderResolution::Unbound,
                    Err(e) => SenderResolution::ServiceError(e),
                }
            }
            Err(e) => SenderResolution::ServiceError(e),
        }
    }

    /// 每个 IM 会话 → 一个稳定 agent 会话 id（`im-<kind>-<净化chat_id>`），跨消息续上下文。
    /// kind 前缀让多 provider（飞书/微信/钉钉）的会话互不撞名。
    fn session_for(&self, chat: &str) -> String {
        let clean: String = chat
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        let sid = format!("im-{}-{clean}", self.kind);
        self.sessions
            .lock()
            .expect("sessions lock")
            .entry(chat.to_string())
            .or_insert(sid)
            .clone()
    }

    /// 处理一轮：拉取 → 逐条（鉴权 → 跑回合 → 分段回复）。返回处理条数。
    pub async fn tick(&self) -> Result<usize, String> {
        let off = self.offset.load(Ordering::SeqCst);
        let (msgs, next) = self.provider.poll(off).await?;
        self.offset.store(next, Ordering::SeqCst);
        let mut n = 0;
        for m in msgs {
            n += 1;
            if !self.allowed(&m.chat_id) {
                tracing::info!("IM 未授权 chat_id={}（不在白名单）", m.chat_id);
                let _ = self
                    .provider
                    .send(&m.chat_id, "⛔ 未授权：你的会话不在允许列表内。")
                    .await;
                continue;
            }
            // 个人模式：所有消息直接以桌面壳当前登录用户身份跑回合（跳过绑定/验证码）。
            // 未登录 → 提示先登录（fail-closed：没有身份就不跑）。
            let identity = if self.personal {
                match self.app.current_subject() {
                    Some(s) => Some(BoundIdentity {
                        user_id: s.user.clone(),
                        username: s.user.clone(),
                        roles: s.roles.clone(),
                    }),
                    None => {
                        tracing::warn!("IM 个人模式未登录 provider={}", self.kind);
                        let _ = self
                            .provider
                            .send(
                                &m.chat_id,
                                "🔒 桌面端尚未登录：请先在桌面应用登录门户账号，IM 消息将以该账号身份对话。",
                            )
                            .await;
                        continue;
                    }
                }
            } else {
                // 绑定模式：先解析发送者身份（未配 resolver = 白名单模式，直接跑）。
                match &self.bindings {
                Some(resolver) => match self.resolve_sender(resolver, &m).await {
                    SenderResolution::Bound(id) => Some(id),
                    SenderResolution::JustBound(id) => {
                        let _ = self
                            .provider
                            .send(
                                &m.chat_id,
                                &format!(
                                    "✅ 绑定成功：{}（{}）。现在直接发消息即可对话。",
                                    id.username, id.user_id
                                ),
                            )
                            .await;
                        continue;
                    }
                    SenderResolution::Unbound | SenderResolution::NoSender => {
                        tracing::info!("IM 未绑定 provider={} sender={}", self.kind, m.sender);
                        let _ = self
                            .provider
                            .send(
                                &m.chat_id,
                                "🔒 请先绑定账号：在桌面端「设置 → IM 绑定」获取验证码，把验证码发给我完成绑定。",
                            )
                            .await;
                        continue;
                    }
                    SenderResolution::ServiceError(e) => {
                        tracing::warn!("IM 绑定服务不可用：{e}");
                        let _ = self
                            .provider
                            .send(&m.chat_id, "⚠ 绑定服务暂不可用，稍后再试。")
                            .await;
                        continue;
                    }
                },
                None => None,
                }
            };
            let sid = self.session_for(&m.chat_id);
            let _ = self.app.create_session(&sid);
            let reply = match identity {
                // 已绑定：以绑定用户身份跑回合（守卫/数据权限按此人判定，与桌面登录身份互不干扰）。
                Some(id) => {
                    let mut subj = cmx_agent_core::Subject::new(&id.user_id);
                    subj.roles = id.roles.clone();
                    match self.app.send_as(&sid, &m.text, &subj).await {
                        Ok(o) => o.final_text.unwrap_or_else(|| "（本回合无文字回复）".into()),
                        Err(e) => format!("⚠ 处理出错：{e}"),
                    }
                }
                None => match self.app.send(&sid, &m.text).await {
                    Ok(o) => o.final_text.unwrap_or_else(|| "（本回合无文字回复）".into()),
                    Err(e) => format!("⚠ 处理出错：{e}"),
                },
            };
            for chunk in chunk_text(&reply, 3800) {
                let _ = self.provider.send(&m.chat_id, &chunk).await;
            }
        }
        Ok(n)
    }

    /// 长轮询/收件主循环（出错退避重试；无消息时节流，避免空转）。
    /// 热重载：`stop` watch 置 true → 主循环干净退出（桥任务结束，壳侧起新桥）。
    pub async fn run(&self, stop: watch::Receiver<bool>) {
        tracing::info!("cmx-agent IM 桥启动，开始轮询…");
        let mut stop = stop.clone();
        loop {
            // stop 已置位（或本轮 tick 后置位）→ 退出。
            if *stop.borrow() {
                tracing::info!("IM 桥停止（热重载）");
                return;
            }
            tokio::select! {
                _ = stop.changed() => {
                    if *stop.borrow() {
                        tracing::info!("IM 桥停止（热重载）");
                        return;
                    }
                }
                r = self.tick() => match r {
                    Ok(0) => {
                        // 无消息：短歇，避免空转吃 CPU（长轮询 provider 自带阻塞；Stream provider 靠此节流）。
                        // 睡眠期间也响应 stop。
                        tokio::select! {
                            _ = stop.changed() => { if *stop.borrow() { tracing::info!("IM 桥停止（热重载）"); return; } }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(500)) => {}
                        }
                    }
                    Ok(_) => { /* 有消息，立即进入下一轮 */ }
                    Err(e) => {
                        tracing::warn!("im tick 出错：{e}");
                        tokio::select! {
                            _ = stop.changed() => { if *stop.borrow() { tracing::info!("IM 桥停止（热重载）"); return; } }
                            _ = tokio::time::sleep(std::time::Duration::from_secs(3)) => {}
                        }
                    }
                }
            }
        }
    }
}

/// 按字符数分段（IM 单条有长度上限，如 Telegram 4096）。优先在换行处断，单行超长则硬切。
pub fn chunk_text(s: &str, max: usize) -> Vec<String> {
    if s.chars().count() <= max {
        return vec![s.to_string()];
    }
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cnt = 0usize;
    for line in s.split_inclusive('\n') {
        let lc = line.chars().count();
        if cnt + lc > max && !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
            cnt = 0;
        }
        if lc > max {
            for ch in line.chars() {
                cur.push(ch);
                cnt += 1;
                if cnt >= max {
                    out.push(std::mem::take(&mut cur));
                    cnt = 0;
                }
            }
        } else {
            cur.push_str(line);
            cnt += lc;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::chunk_text;

    #[test]
    fn chunk_splits_on_limit() {
        let one = chunk_text("短", 100);
        assert_eq!(one.len(), 1);
        let s = "行一\n".repeat(50); // 150 字符
        let parts = chunk_text(&s, 40);
        assert!(parts.len() > 1);
        assert!(parts.iter().all(|p| p.chars().count() <= 40));
        assert_eq!(parts.concat(), s); // 无损
    }
}
