//! 会话 [`Session`]：持有 append-only 日志，并**只从日志派生**给模型的上下文——这就是不变量
//! "Model-visible means logged" 的结构性保证：模型看到的每条消息都能在日志里找到出处。

use crate::event::{EventKind, SessionLog};
use crate::model::{ModelContext, ModelMessage};
use crate::tool::ToolSpec;

/// 一个会话（M0：单会话、内存态。后续可持久化 / 多会话 / 子会话隔离）。
#[derive(Debug)]
pub struct Session {
    pub id: String,
    pub log: SessionLog,
    system: Option<String>,
}

impl Session {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            log: SessionLog::new(),
            system: None,
        }
    }

    /// 设置系统指令（AGENTS.md 式项目指令的 M0 落点）。
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn system(&self) -> Option<&str> {
        self.system.as_deref()
    }

    /// 下一个回合号：历史 `TurnEnded` 事件计数 + 1。从日志派生 → 持久化恢复后能续上正确回合号。
    pub fn next_turn_no(&self) -> u64 {
        let ended = self.log.count(|k| matches!(k, EventKind::TurnEnded { .. }));
        ended as u64 + 1
    }

    /// 用一串已有事件重建会话（持久化恢复）。事件按 `seq` 排序后灌入日志，seq/时间戳保持原值。
    pub fn from_events(
        id: impl Into<String>,
        system: Option<String>,
        events: impl IntoIterator<Item = crate::event::SessionEvent>,
    ) -> Self {
        let mut log = SessionLog::new();
        let mut evs: Vec<_> = events.into_iter().collect();
        evs.sort_by_key(|e| e.seq);
        for e in evs {
            log.push_restored(e);
        }
        Self {
            id: id.into(),
            log,
            system,
        }
    }

    /// 从 append-only 日志重建给模型的上下文（+ 当前工具清单）。
    ///
    /// 遍历事件，把"模型可见"的三类事件投影为消息：
    /// - `UserMessage` → User
    /// - `ModelMessage` → Assistant（文本 + 工具调用）
    /// - `ToolResult`   → Tool（回灌）
    ///
    /// 其余事件（守卫裁决、审批、注记、turn 边界）是审计/控制流元数据，不进模型上下文。
    pub fn model_context(&self, tools: Vec<ToolSpec>) -> ModelContext {
        let mut messages = Vec::new();
        for ev in self.log.iter() {
            match &ev.kind {
                EventKind::UserMessage { text } => {
                    messages.push(ModelMessage::User { text: text.clone() });
                }
                EventKind::ModelMessage { text, tool_calls } => {
                    messages.push(ModelMessage::Assistant {
                        text: text.clone(),
                        tool_calls: tool_calls.clone(),
                    });
                }
                EventKind::ToolResult {
                    call_id, output, ..
                } => {
                    messages.push(ModelMessage::Tool {
                        call_id: call_id.clone(),
                        output: output.clone(),
                    });
                }
                _ => {}
            }
        }
        ModelContext {
            system: self.system.clone(),
            messages,
            tools,
        }
    }
}
