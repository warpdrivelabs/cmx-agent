//! 流式事件接收器（E办公助手：对齐 cmx-ai 的 SSE 事件流思路）。
//!
//! 内核 [`cmx_agent_core::event::SessionLog`] 每 append 一个事件就通知所有 [`EventSink`]。
//! 本模块提供一个把事件序列化后投递到 tokio 无界通道的 sink —— 壳（web SSE / Tauri emit）据此
//! 把回合里的每个事件（模型消息 / 工具调用 / 工具结果 / 收尾）**边产生边推给前端**。

use cmx_agent_core::event::{EventSink, SessionEvent};
use cmx_agent_core::TurnObserver;
use tokio::sync::mpsc::UnboundedSender;

/// 把每个会话事件序列化为 JSON 并发到通道的 sink。发送失败（接收端已断）静默忽略。
pub struct ChannelSink {
    tx: UnboundedSender<serde_json::Value>,
}

impl ChannelSink {
    pub fn new(tx: UnboundedSender<serde_json::Value>) -> Self {
        Self { tx }
    }
}

impl EventSink for ChannelSink {
    fn on_event(&self, ev: &SessionEvent) {
        if let Ok(v) = serde_json::to_value(ev) {
            let _ = self.tx.send(v);
        }
    }
}

/// 同一个 sink 也承接**文字增量**（token 流）：发一个瞬时 `{"kind":"text_delta","text":..}`。
/// 前端据此把增量追加到当前助手气泡，形成打字机效果；deltas 不落库（区别于 ModelMessage 全文）。
impl TurnObserver for ChannelSink {
    fn on_text_delta(&self, delta: &str) {
        let _ = self
            .tx
            .send(serde_json::json!({ "kind": "text_delta", "text": delta }));
    }
}
