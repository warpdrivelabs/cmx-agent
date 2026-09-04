//! 顶层错误。设计取向：**工具失败不致命**（转 [`crate::tool::ToolResult`] 回灌给模型自愈），
//! 只有模型缝失败等才升级为致命 `AgentError`。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AgentError {
    /// 模型缝调用失败（网络 / 协议 / 供应商错误）。
    #[error("model error: {0}")]
    Model(String),

    /// 内核装配缺失（如未注入模型缝）。
    #[error("agent not configured: {0}")]
    NotConfigured(String),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub type AgentResult<T> = Result<T, AgentError>;
