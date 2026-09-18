//! cmx-agent E0：真实模型缝。
//!
//! 「同核多壳」的模型侧接缝：把内核 [`cmx_agent_core::model::ModelSeam`] 用真实大模型实现——
//! 一个 **OpenAI 兼容** provider（`{base}/chat/completions`，支持工具调用），可对接
//! DeepSeek / OpenAI / Qwen(DashScope 兼容模式) / 本地 Ollama·vLLM。
//!
//! 网络依赖（reqwest）隔离在本 crate；`cmx-agent-core` 仍零网络、纯离线可测。
//! 未配置（无 API Key 且无显式 base_url）时，壳回退到离线 [`cmx_agent_app::DemoModel`]。

pub mod config;
pub mod error_friendly;
pub mod openai;
pub mod providers;

pub use config::ModelProviderConfig;
pub use error_friendly::{classify, friendly_model_error, friendly_model_error_brief, is_context_overflow};
pub use openai::{
    OpenAiCompatModel, TestConnectError, build_request_body, parse_response,
};
pub use providers::{
    ModelEntry, NamedProvider, PROVIDER_PRESETS, ProviderFile, ProviderPreset, MODEL_CAPABILITIES,
    MODEL_INPUT_TYPES, MODEL_REASONING_ORDER, find_preset, new_id, provider_presets_json,
    resolve_active,
};
