//! 应用层错误。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    /// 认证失败（用户名/密码错误、服务不可达等）。message 已是面向用户的干净文案（错误类型由 code 表达）。
    #[error("{0}")]
    Auth(String),

    /// 持久化数据损坏（无法解析的日志/元数据行）。
    #[error("corrupt store: {0}")]
    Corrupt(String),

    #[error("agent error: {0}")]
    Agent(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl From<cmx_agent_core::AgentError> for AppError {
    fn from(e: cmx_agent_core::AgentError) -> Self {
        AppError::Agent(e.to_string())
    }
}

pub type AppResult<T> = Result<T, AppError>;
