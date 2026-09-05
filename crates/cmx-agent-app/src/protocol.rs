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
    /// 读取会话事件。`limit`/`before` 用于大会话分页尾加载：`limit=None` 全量（向后兼容）；
    /// `Some(n)` 取最近 n 条，`before=Some(k)` 向前翻页。响应含 events/total/start/title。
    GetEvents {
        session_id: String,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        before: Option<usize>,
    },
    /// 列出所有会话。
    ListSessions,
    /// 删除会话。
    DeleteSession { session_id: String },
    /// 列出连接器（描述 + live 健康）——侧栏「专家·技能·连接器」面板用。
    ListConnectors,
    /// 登录（对接门户 /api/auth/login）。成功后前门持有当前用户。
    Login { username: String, password: String },
    /// 取当前登录用户（前端启动时填充用户菜单；未登录 data.user=null）。
    CurrentUser,
    /// 登出（清当前用户）。
    Logout,
    /// 人在环审批决定（X4）：前端在审批卡片点「允许/拒绝」后发来，唤醒挂起的回合。
    /// `all=true`（「本对话全部允许」）时，把 `session_id` 会话标记为全部允许，后续不再弹卡。
    Approve {
        call_id: String,
        approved: bool,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        session_id: String,
    },
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
        AppError::Auth(_) => "auth_error",
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
        AppRequest::GetEvents { session_id, limit, before } => {
            // 分页 / 尾加载。limit=None 时仍返回全量（向后兼容），但前端现在会带 limit 只取一屏。
            let w = app.get_events_window(&session_id, limit, before)?;
            Ok(AppResponse::ok(serde_json::json!({
                "events": w.events, "total": w.total, "start": w.start, "title": w.title
            })))
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
        AppRequest::Login { username, password } => {
            let user = app.login(&username, &password).await?;
            Ok(AppResponse::ok(serde_json::json!({ "user": user })))
        }
        AppRequest::CurrentUser => Ok(AppResponse::ok(
            serde_json::json!({ "user": app.current_user() }),
        )),
        AppRequest::Logout => {
            app.logout();
            Ok(AppResponse::ok(serde_json::json!({ "ok": true })))
        }
        AppRequest::Approve { call_id, approved, all, session_id } => {
            let hit = app.resolve_approval_decision(&call_id, approved, all, &session_id);
            Ok(AppResponse::ok(
                serde_json::json!({ "resolved": hit, "call_id": call_id, "approved": approved, "all": all }),
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
