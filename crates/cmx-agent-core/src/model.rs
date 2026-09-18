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

/// 一次请求的服务端用量（压缩方案 §4.1.1）。`input` = 该次请求的上下文规模（最能量化占用）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelUsage {
    pub input: u64,
    pub output: u64,
    /// 命中缓存的输入 token（OpenAI `prompt_tokens_details.cached_tokens` 与 DeepSeek
    /// 风格 `prompt_cache_hit_tokens` 归一）；网关未回传 = None（命中率行隐藏）。
    pub cached_input: Option<u64>,
}

/// 模型响应：自由文本 + 零或多个工具调用。无工具调用 = 回合可完成。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ModelResponse {
    #[serde(default)]
    pub text: Option<String>,
    /// 推理模型可返回的思考摘要/原文；不进入下一轮模型上下文。
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<ToolCall>,
    /// 服务端用量（网关未回传 = None；流式经 `stream_options.include_usage` 换取）。
    #[serde(default)]
    pub usage: Option<ModelUsage>,
}

impl ModelResponse {
    /// 纯文本响应（无工具调用）。
    pub fn text(t: impl Into<String>) -> Self {
        Self {
            text: Some(t.into()),
            ..Default::default()
        }
    }

    /// 请求一批工具调用（可附文本）。
    pub fn calls(tool_calls: Vec<ToolCall>) -> Self {
        Self {
            tool_calls,
            ..Default::default()
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

/// 回合观察者：接收**瞬时**的文字增量（token 流），用于打字机效果。**不进日志**（区别于 EventSink）——
/// 最终完整文本仍以 `ModelMessage` 事件落库，deltas 仅用于实时 UI。
pub trait TurnObserver: Send + Sync {
    /// 模型产出一段文字增量（可能是几个字/一句）。
    fn on_text_delta(&self, delta: &str);

    /// 流式中断后决定重试时调用：前端应**丢弃**此前收到的所有 `on_text_delta` 半截文字，
    /// 等重试成功后再重新接收增量（避免重试时文字重复/错位）。默认空实现——未重试的模型/调用方无需改动。
    fn on_stream_reset(&self) {}

    /// 推理模型的思考过程增量；deltas 不落日志，最终完整内容仍以 Reasoning 事件落库。
    fn on_reasoning_delta(&self, _delta: &str) {}

    /// 流重试时同时清空已收思考增量。默认空实现。
    fn on_reasoning_reset(&self) {}

    /// 外部中断探测：模型流读取器据此尽快停止请求与重试。
    fn is_cancelled(&self) -> bool {
        false
    }
}

/// 模型缝 trait。`complete` = 给定上下文，产出下一步响应。
#[async_trait]
pub trait ModelSeam: Send + Sync {
    async fn complete(&self, ctx: &ModelContext) -> Result<ModelResponse, ModelError>;

    /// 有输出上限的一次补全（上下文压缩的摘要请求专用，压缩方案 §4.2.4.3；正常回合走
    /// `complete`/`complete_streaming`）。默认实现忽略上限——Mock/Demo 无需感知；
    /// OpenAI 兼容实现覆写为请求体带 `max_tokens`。
    async fn complete_bounded(
        &self,
        ctx: &ModelContext,
        _max_tokens: u64,
    ) -> Result<ModelResponse, ModelError> {
        self.complete(ctx).await
    }

    /// 流式版：产出文字增量经 `observer.on_text_delta` 实时回调，最终返回完整 [`ModelResponse`]。
    /// 默认实现回退到非流式 `complete`（把全文当作一个 delta 回调），保证 Mock/Demo 等无需改动。
    async fn complete_streaming(
        &self,
        ctx: &ModelContext,
        observer: &dyn TurnObserver,
    ) -> Result<ModelResponse, ModelError> {
        let resp = self.complete(ctx).await?;
        if let Some(t) = &resp.text
            && !t.is_empty() {
                observer.on_text_delta(t);
            }
        Ok(resp)
    }
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
