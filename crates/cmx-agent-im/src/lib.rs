//! cmx-agent-im（U9 IM 遥控）：把 IM 会话桥接到 [`cmx_agent_app::AgentApp`]——
//! 从 IM 收到消息 → 映射到一个 agent 会话 → 鉴权 → 跑一个回合 → 把回复分段发回 IM。
//!
//! **transport 无关**：核心是 [`ImProvider`] trait（拉取 + 发送）+ [`ImBridge`] 编排。
//! 参考实现 [`TelegramProvider`]（长轮询）；企业微信/飞书/钉钉/自建网关按同一 trait 追加即可。
//!
//! **安全**：IM 可驱动 agent 跑工具（bash/写文件…）→ 生产务必配**白名单**（`allow`）。
//! `allow=None` 表示开放，仅供测试/纯内网；CLI `im` 模式强制要求白名单。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use cmx_agent_app::AgentApp;

mod telegram;
mod feishu;
mod config;
pub use telegram::TelegramProvider;
pub use feishu::FeishuProvider;
pub use config::{ImConfig, ImKind, parse_allow};

/// 一条入站 IM 消息。
#[derive(Debug, Clone)]
pub struct InboundMsg {
    pub chat_id: String,
    pub text: String,
    /// 传输层游标（如 Telegram update_id）——用于长轮询增量拉取。
    pub update_id: i64,
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
}

/// IM 桥：每个 IM 会话映射到一个稳定的 agent 会话，鉴权 → 跑回合 → 分段回复。
pub struct ImBridge {
    app: Arc<AgentApp>,
    provider: Arc<dyn ImProvider>,
    /// provider 类型标签，用于会话命名前缀 `im-<kind>-<chat>`（多 provider 不撞名）。
    kind: String,
    /// None=开放（仅测试/内网）；Some=chat_id 白名单。
    allow: Option<HashSet<String>>,
    offset: AtomicI64,
    sessions: Mutex<HashMap<String, String>>,
}

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
            offset: AtomicI64::new(0),
            sessions: Mutex::new(HashMap::new()),
        }
    }

    fn allowed(&self, chat: &str) -> bool {
        self.allow.as_ref().map(|s| s.contains(chat)).unwrap_or(true)
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
            let sid = self.session_for(&m.chat_id);
            let _ = self.app.create_session(&sid);
            let reply = match self.app.send(&sid, &m.text).await {
                Ok(o) => o.final_text.unwrap_or_else(|| "（本回合无文字回复）".into()),
                Err(e) => format!("⚠ 处理出错：{e}"),
            };
            for chunk in chunk_text(&reply, 3800) {
                let _ = self.provider.send(&m.chat_id, &chunk).await;
            }
        }
        Ok(n)
    }

    /// 长轮询/收件主循环（出错退避重试；无消息时节流，避免空转）。
    pub async fn run(&self) {
        tracing::info!("cmx-agent IM 桥启动，开始轮询…");
        loop {
            match self.tick().await {
                Ok(0) => {
                    // 无消息：短歇，避免空转吃 CPU（长轮询 provider 自带阻塞；Stream provider 靠此节流）。
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
                Ok(_) => { /* 有消息，立即进入下一轮 */ }
                Err(e) => {
                    tracing::warn!("im tick 出错：{e}");
                    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
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
