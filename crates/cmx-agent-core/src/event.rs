//! 会话事件日志 —— 审计、回放、记忆的同一地基（方案图 9）。
//!
//! 不变量 **Model-visible means logged**：凡是模型能看见的（用户输入、模型输出、工具调用、
//! 守卫裁决、工具结果、审批），都必须先 [`SessionLog::append`] 进这条 append-only 流。
//! 由 [`crate::session::Session::model_context`] 结构性地保证——模型上下文只从本日志派生。

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::guard::{GuardDecision, GuardPhase};
use crate::question::AskQuestion;
use crate::tool::ToolCall;

/// 一条会话事件的类型化载荷（内部标签 `kind`，snake_case）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EventKind {
    /// 一个回合开始（含触发它的用户输入，便于回放定位）。
    TurnStarted { turn: u64, user_input: String },
    /// 用户消息（模型可见）。
    UserMessage { text: String },
    /// 模型输出：自由文本 + 零或多个工具调用意图（模型可见——它看得见自己上一步说了什么）。
    ModelMessage {
        text: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
    },
    /// 推理模型的思考过程（不回灌上下文，但审计/回放/界面需要）。
    Reasoning {
        #[serde(default)]
        text: String,
    },
    /// 工具被派发（进入守卫管道之前登记，保证"意图"可审计）。
    ToolInvoked { call: ToolCall },
    /// 守卫裁决（某一相 pre/execute/post 的第一个非 Allow 结果，或最终 Allow）。
    GuardDecision {
        call_id: String,
        phase: GuardPhase,
        guard: String,
        #[serde(flatten)]
        decision: GuardDecision,
    },
    /// 触发人在环审批。
    ApprovalRequested {
        call_id: String,
        tool: String,
        reason: String,
        /// 参数摘要（shell 类取命令文本，其余取紧凑 JSON，截断）——审批卡上直接展示 `$ …`。
        #[serde(default)]
        summary: String,
    },
    /// 审批结果。
    ApprovalResolved {
        call_id: String,
        approved: bool,
        by: String,
    },
    /// 工具结果（模型可见——回灌给下一步）。
    ToolResult {
        call_id: String,
        ok: bool,
        output: serde_json::Value,
    },
    /// 触发向用户提问（答题卡渲染 + 审计 + 重放；问题内容模型本可见于 ToolInvoked.input，
    /// 故本事件不进模型上下文——由 [`crate::session::Session::model_context`] 白名单投影保证）。
    QuestionAsked {
        request_id: String,
        questions: Vec<AskQuestion>,
    },
    /// 提问解决：`answered=false` 时 `by` 说明放弃方（user=忽略 / canceled=清理 / timeout=超时）。
    QuestionResolved {
        request_id: String,
        answered: bool,
        by: String,
        /// 答案（键=question id，值=选中 label 数组；忽略/超时为空表）——UI 轨迹行
        /// 「已询问 N 个问题」展开还原 Q/A 用（ZCode 式）。`#[serde(default)]` 兼容旧落库事件。
        #[serde(default)]
        answers: serde_json::Map<String, serde_json::Value>,
    },
    /// 回合结束。
    TurnEnded {
        turn: u64,
        reason: StopReason,
        steps: usize,
    },
    /// 内部注记（不必模型可见）。
    Note { text: String },
}

/// 回合停机原因（对齐 dsh `agent/turn-stopping` 语义）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// 模型不再请求工具，回合自然完成。
    Completed,
    /// 达到 `max_steps` 上限（防失控）。
    MaxSteps,
    /// 被停机钩子/外部显式停止。
    Stopped,
    /// 致命错误中止。
    Error,
}

/// 一条带序号与时间戳的会话事件。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionEvent {
    pub seq: u64,
    pub ts: DateTime<Utc>,
    #[serde(flatten)]
    pub kind: EventKind,
}

/// 事件旁路（审计中心 / 持久化 / SSE 推送等在此挂钩，不影响主流程）。
pub trait EventSink: Send + Sync {
    fn on_event(&self, ev: &SessionEvent);
}

/// append-only 会话日志。**没有**任何删除/改写方法——不可变性即审计可信度。
#[derive(Default)]
pub struct SessionLog {
    events: Vec<SessionEvent>,
    seq: u64,
    sinks: Vec<Arc<dyn EventSink>>,
}

impl std::fmt::Debug for SessionLog {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionLog")
            .field("events", &self.events.len())
            .field("seq", &self.seq)
            .field("sinks", &self.sinks.len())
            .finish()
    }
}

impl SessionLog {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册一个事件旁路（审计/持久化）。返回 self 便于链式。
    pub fn add_sink(&mut self, sink: Arc<dyn EventSink>) -> &mut Self {
        self.sinks.push(sink);
        self
    }

    /// 追加一条事件，分配单调递增的 seq 与时间戳，并通知所有旁路。返回该事件的只读引用。
    pub fn append(&mut self, kind: EventKind) -> &SessionEvent {
        self.seq += 1;
        let ev = SessionEvent {
            seq: self.seq,
            ts: Utc::now(),
            kind,
        };
        for s in &self.sinks {
            s.on_event(&ev);
        }
        self.events.push(ev);
        self.events.last().expect("just pushed")
    }

    pub fn events(&self) -> &[SessionEvent] {
        &self.events
    }

    /// 恢复一条已有事件（持久化回放）：保留其原 `seq`/`ts`，不重新分配。
    /// 与 [`SessionLog::append`] 区别：append 是"新发生的事件"分配新序号；push_restored 是"重放旧事件"。
    /// 不触发 sink（避免回放时把历史事件误当新事件外发）。内部维护 `seq` 水位为已见最大值。
    pub fn push_restored(&mut self, ev: SessionEvent) {
        self.seq = self.seq.max(ev.seq);
        self.events.push(ev);
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &SessionEvent> + ExactSizeIterator {
        self.events.iter()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// 统计满足谓词的事件数（测试/监控便捷）。
    pub fn count<F: Fn(&EventKind) -> bool>(&self, f: F) -> usize {
        self.events.iter().filter(|e| f(&e.kind)).count()
    }
}
