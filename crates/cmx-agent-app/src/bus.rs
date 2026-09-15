//! 会话事件总线（U16）：把任意来源（本地发消息 / IM 桥 / 后续 webhook）追加的会话事件
//! **实时广播**给进程内订阅者（桌面壳的 `/api/subscribe` SSE），实现「IM 消息实时展示到对话界面」。
//!
//! 设计：`AgentApp` 持有一个 `SessionEventBus`（包 `tokio::broadcast`）。每次回合 `send_inner` 给
//! `session.log` 挂一个 [`BusSink`]——`SessionLog::append` 通知 sink 时，事件被封装成
//! [`EventEnvelope`]（带 `session_id`）广播出去。订阅者拿到后自行分流到对应会话 tab。
//!
//! 与 [`crate::stream::ChannelSink`] 的区别：ChannelSink 是 per-turn 瞬态通道（本地回合的 token 流 +
//! 事件，回合结束即弃）；本总线是进程内常驻 pub/sub（任何来源、任何会话的事件都广播给所有订阅者）。
//! 两者并存：本地回合同时挂两者——ChannelSink 给打字机 + 文本增量，BusSink 给全局实时订阅。

use cmx_agent_core::event::{EventSink, SessionEvent};
use tokio::sync::broadcast;

/// 广播容量：足够缓存一轮多事件回合 + 多会话并发，避免无订阅者时丢事件（无订阅者 send 失败静默，
/// 这本就无意义；有订阅者但 lagging 才会丢，1024 足够宽裕）。
const BUS_CAPACITY: usize = 1024;

/// 事件信封：把事件与其所属会话 id 绑在一起广播（订阅者据此分流到对应 tab）。
/// `parent`（方案 20260915 子智能体可视化 B1）：子智能体会话事件填所属**父会话 id**，
/// 前端据此把子事件渲染进父视图的子任务卡（而不是当成新会话开 tab）；父会话事件为 `None`。
/// `#[serde(default)]` 兼容旧信封 JSON（无该字段 = None）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EventEnvelope {
    pub session_id: String,
    #[serde(default)]
    pub parent: Option<String>,
    pub event: SessionEvent,
}

/// 进程内会话事件总线（broadcast）。`AgentApp` 持其 `Arc`，订阅者拿 receiver。
#[derive(Clone)]
pub struct SessionEventBus {
    tx: broadcast::Sender<EventEnvelope>,
}

impl SessionEventBus {
    pub fn new() -> Self {
        let (tx, _rx) = broadcast::channel(BUS_CAPACITY);
        Self { tx }
    }

    /// 订阅：返回一个 receiver，逐条收到 `(session_id, event)` 信封。
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.tx.subscribe()
    }

    /// 广播一条事件（供 [`BusSink`] 调用）。无订阅者 → send 失败，静默忽略。
    pub fn publish(&self, env: EventEnvelope) {
        let _ = self.tx.send(env);
    }

    /// 内部 sender（供 [`BusSink`] 持有，避免持有整个 bus 的强引用循环）。
    pub(crate) fn sender(&self) -> broadcast::Sender<EventEnvelope> {
        self.tx.clone()
    }
}

impl Default for SessionEventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// 事件旁路 sink：挂到 `SessionLog::add_sink`，把该会话每个 append 的事件广播到总线。
/// 一个会话一个 sink（持有 session_id + bus sender）。
pub struct BusSink {
    session_id: String,
    tx: broadcast::Sender<EventEnvelope>,
}

impl BusSink {
    pub fn new(session_id: impl Into<String>, tx: broadcast::Sender<EventEnvelope>) -> Self {
        Self { session_id: session_id.into(), tx }
    }
}

impl EventSink for BusSink {
    fn on_event(&self, ev: &SessionEvent) {
        // send 失败 = 无订阅者或 lagging；静默忽略（事件仍正常落库，订阅者拉取历史兜底）。
        let _ = self.tx.send(EventEnvelope {
            session_id: self.session_id.clone(),
            parent: None, // 父会话事件无 parent；子会话走 SubagentHandle 的 event_sink 通路
            event: ev.clone(),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::event::EventKind;

    #[tokio::test]
    async fn bus_delivers_envelope_to_subscriber() {
        let bus = SessionEventBus::new();
        let mut rx = bus.subscribe();
        let sink = BusSink::new("s1", bus.sender());
        sink.on_event(&SessionEvent {
            seq: 1,
            ts: chrono::Utc::now(),
            kind: EventKind::UserMessage { text: "hi".into() },
        });
        let env = rx.recv().await.expect("应收到信封");
        assert_eq!(env.session_id, "s1");
        assert_eq!(env.event.seq, 1);
    }

    #[tokio::test]
    async fn bus_no_subscriber_is_silent() {
        let bus = SessionEventBus::new();
        let sink = BusSink::new("s1", bus.sender());
        // 无订阅者：on_event 不应 panic。
        sink.on_event(&SessionEvent {
            seq: 1,
            ts: chrono::Utc::now(),
            kind: EventKind::UserMessage { text: "x".into() },
        });
        // 订阅后只收后续事件（不收历史的）。
        let mut rx = bus.subscribe();
        sink.on_event(&SessionEvent {
            seq: 2,
            ts: chrono::Utc::now(),
            kind: EventKind::UserMessage { text: "y".into() },
        });
        let env = rx.recv().await.unwrap();
        assert_eq!(env.event.seq, 2);
    }

    #[tokio::test]
    async fn bus_fans_out_to_multiple_subscribers() {
        let bus = SessionEventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let sink = BusSink::new("s1", bus.sender());
        sink.on_event(&SessionEvent {
            seq: 1,
            ts: chrono::Utc::now(),
            kind: EventKind::UserMessage { text: "z".into() },
        });
        assert_eq!(rx1.recv().await.unwrap().session_id, "s1");
        assert_eq!(rx2.recv().await.unwrap().session_id, "s1");
    }

    #[test]
    fn envelope_parent_defaults_to_none_for_old_json() {
        // B1 兼容：旧信封 JSON（无 parent 字段）反序列化 = None；父信封 parent 恒 None；
        // 子信封带 parent，序列化往返不丢。
        let old = r#"{"session_id":"s1","event":{"seq":1,"ts":"2026-09-16T00:00:00Z","kind":"user_message","text":"hi"}}"#;
        let env: EventEnvelope = serde_json::from_str(old).expect("旧信封应可解析");
        assert_eq!(env.session_id, "s1");
        assert!(env.parent.is_none(), "缺省 parent 应为 None");
        let sub = EventEnvelope {
            session_id: "subtask-1-2".into(),
            parent: Some("s1".into()),
            event: env.event.clone(),
        };
        let round: EventEnvelope =
            serde_json::from_str(&serde_json::to_string(&sub).unwrap()).unwrap();
        assert_eq!(round.parent.as_deref(), Some("s1"));
    }
}
