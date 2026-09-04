//! 模型缝 ModelSeam（方案图 4 步①）——把"具体大模型"与内核解耦。
//!
//! 真实实现（OpenAI/Anthropic/DeepSeek…）在上层 crate 以 HTTP 客户端提供；本 crate 只定义协议 +
//! 一个确定性的 [`MockModel`]（脚本化响应），让整个回合循环**无需网络即可完整测试**。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;

use crate::tool::{ToolCall, ToolSpec};

/// 喂给模型的上下文（由会话日志派生——见 [`crate::session::Session::model_context`]）。
/// 不变量的落地面：模型能看见的一切都在 `messages` 里，而 `messages` 只来自 append-only 日志。
#[derive(Debug, Clone, Default)]
pub struct ModelContext {
    /// 系统指令（AGENTS.md 式项目指令的落点，M0 简化为一段文本）。
    pub system: Option<String>,
    /// 对话消息（用户/助手/工具结果，按时间序）。
    pub messages: Vec<ModelMessage>,
    /// 可用工具清单（供模型选择）。
    pub tools: Vec<ToolSpec>,
}

/// 一条模型可见的消息。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum ModelMessage {
    User {
        text: String,
    },
    /// 助手消息：文本 + 工具调用意图。
    Assistant {
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        tool_calls: Vec<ToolCall>,
    },
    /// 工具结果回灌（关联 call_id）。
    Tool {
        call_id: String,
        output: serde_json::Value,
    },
}

/// 模型响应：自由文本 + 零或多个工具调用。无工具调用 = 回合可完成。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelResponse {
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
}

impl ModelResponse {
    /// 纯文本响应（无工具调用）。
    pub fn text(t: impl Into<String>) -> Self {
        Self {
            text: Some(t.into()),
            tool_calls: vec![],
        }
    }

    /// 请求一批工具调用（可附文本）。
    pub fn calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            text: None,
            tool_calls,
        }
    }

    pub fn with_text(mut self, t: impl Into<String>) -> Self {
        self.text = Some(t.into());
        self
    }

    pub fn wants_tools(&self) -> bool {
        !self.tool_calls.is_empty()
    }
}

#[derive(Debug, thiserror::Error)]
#[error("model seam error: {0}")]
pub struct ModelError(pub String);

/// 模型缝 trait。`complete` = 给定上下文，产出下一步响应。
#[async_trait]
pub trait ModelSeam: Send + Sync {
    async fn complete(&self, ctx: &ModelContext) -> Result<ModelResponse, ModelError>;
}

/// 确定性 Mock 模型：按注入的脚本逐次弹出响应；脚本耗尽后返回一句收尾文本（无工具调用 → Completed）。
/// 记录每次收到的上下文快照，供测试断言"模型看见了什么"。
pub struct MockModel {
    scripted: Mutex<std::collections::VecDeque<ModelResponse>>,
    seen: Mutex<Vec<ModelContext>>,
    fallback: ModelResponse,
}

impl MockModel {
    /// 用一串脚本响应构造。
    pub fn new(script: impl IntoIterator<Item = ModelResponse>) -> Self {
        Self {
            scripted: Mutex::new(script.into_iter().collect()),
            seen: Mutex::new(vec![]),
            fallback: ModelResponse::text("done"),
        }
    }

    /// 单步纯文本模型（不调工具，一回合即完成）。
    pub fn saying(text: impl Into<String>) -> Self {
        Self::new([ModelResponse::text(text)])
    }

    /// 自定义脚本耗尽后的兜底响应。
    pub fn fallback(mut self, resp: ModelResponse) -> Self {
        self.fallback = resp;
        self
    }

    /// 已消费的上下文快照数（= 被调用次数）。
    pub fn calls_seen(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// 第 n 次调用时模型看到的上下文（测试断言用）。
    pub fn nth_context(&self, n: usize) -> Option<ModelContext> {
        self.seen.lock().unwrap().get(n).cloned()
    }

    /// 最近一次看到的上下文。
    pub fn last_context(&self) -> Option<ModelContext> {
        self.seen.lock().unwrap().last().cloned()
    }
}

#[async_trait]
impl ModelSeam for MockModel {
    async fn complete(&self, ctx: &ModelContext) -> Result<ModelResponse, ModelError> {
        self.seen.lock().unwrap().push(ctx.clone());
        let next = self.scripted.lock().unwrap().pop_front();
        Ok(next.unwrap_or_else(|| self.fallback.clone()))
    }
}
