//! 前门命令协议（同核多壳的接缝）。定义一组 **JSON 请求/响应**，由 [`dispatch`] 派发到 [`AgentApp`]。
//!
//! 这正是 Tauri `invoke("cmd", args)` 的边界：桌面壳前端发一个 `AppRequest` JSON，Rust 侧
//! `dispatch` 后回一个 `AppResponse` JSON。**同一协议**也服务 CLI 与后续 Headless HTTP——
//! 换壳不换核。所有变体 `tag = "cmd"`，snake_case（对齐 codex/规则引擎 rename_all 约定）。

use crate::app::{AgentApp, SendOutcome};
use crate::error::AppError;
use crate::store::SessionMeta;

/// 前门请求。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum AppRequest {
    /// 新建会话。
    CreateSession { id: String },
    /// 发一条用户消息，跑一个回合。
    Send { session_id: String, text: String },
    /// 读取会话全部事件。
    GetEvents { session_id: String },
    /// 列出所有会话。
    ListSessions,
    /// 删除会话。
    DeleteSession { session_id: String },
    /// 列出连接器（描述 + live 健康）——侧栏「专家·技能·连接器」面板用。
    ListConnectors,
}

/// 前门响应（`ok=false` 时 `error` 有值；成功时 `data` 按命令而异）。统一信封，便于前端一致处理。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<AppErrorBody>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppErrorBody {
    /// 稳定错误码（前端可据此分支，不依赖 message 文案）。
    pub code: String,
    pub message: String,
}

impl AppResponse {
    fn ok(data: serde_json::Value) -> Self {
        Self {
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    fn err(code: &str, message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(AppErrorBody {
                code: code.into(),
                message: message.into(),
            }),
        }
    }
}

/// 把 [`AppError`] 映射为稳定错误码（前门契约的一部分）。
fn error_code(e: &AppError) -> &'static str {
    match e {
        AppError::NotFound(_) => "not_found",
        AppError::BadRequest(_) => "bad_request",
        AppError::Corrupt(_) => "corrupt_store",
        AppError::Agent(_) => "agent_error",
        AppError::Io(_) => "io_error",
        AppError::Json(_) => "json_error",
    }
}

/// 派发一个请求到 app，产出统一信封响应。**永不 panic / 永不 Err**——错误进信封，前门始终拿到 JSON。
pub async fn dispatch(app: &AgentApp, req: AppRequest) -> AppResponse {
    let result = dispatch_inner(app, req).await;
    match result {
        Ok(resp) => resp,
        Err(e) => AppResponse::err(error_code(&e), e.to_string()),
    }
}

async fn dispatch_inner(app: &AgentApp, req: AppRequest) -> Result<AppResponse, AppError> {
    match req {
        AppRequest::CreateSession { id } => {
            let id = app.create_session(id)?;
            Ok(AppResponse::ok(serde_json::json!({ "session_id": id })))
        }
        AppRequest::Send { session_id, text } => {
            let outcome: SendOutcome = app.send(&session_id, &text).await?;
            Ok(AppResponse::ok(serde_json::to_value(outcome)?))
        }
        AppRequest::GetEvents { session_id } => {
            let events = app.get_events(&session_id)?;
            Ok(AppResponse::ok(serde_json::json!({ "events": events })))
        }
        AppRequest::ListSessions => {
            let sessions: Vec<SessionMeta> = app.list_sessions()?;
            Ok(AppResponse::ok(serde_json::json!({ "sessions": sessions })))
        }
        AppRequest::DeleteSession { session_id } => {
            app.delete_session(&session_id)?;
            Ok(AppResponse::ok(
                serde_json::json!({ "deleted": session_id }),
            ))
        }
        AppRequest::ListConnectors => {
            let connectors = app.list_connectors().await;
            Ok(AppResponse::ok(
                serde_json::json!({ "connectors": connectors }),
            ))
        }
    }
}

/// 便捷：从 JSON 字符串派发到 JSON 字符串（stdin/stdout 前门 / Tauri invoke 的最薄封装）。
pub async fn dispatch_json(app: &AgentApp, raw: &str) -> String {
    let resp = match serde_json::from_str::<AppRequest>(raw) {
        Ok(req) => dispatch(app, req).await,
        Err(e) => AppResponse::err("bad_request", format!("invalid request json: {e}")),
    };
    serde_json::to_string(&resp).unwrap_or_else(|e| {
        format!(r#"{{"ok":false,"error":{{"code":"json_error","message":"{e}"}}}}"#)
    })
}
