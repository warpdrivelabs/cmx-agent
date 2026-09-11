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
    /// 列出工作空间与当前选择；前端输入区工作空间悬浮菜单用。
    ListWorkspaces,
    /// 新建托管工作空间（数据目录 `<data>/workspaces/<id>`）。
    CreateWorkspace { name: String },
    /// 添加一个用户显式给出的本地目录作为工作空间。
    AddLocalWorkspace {
        path: String,
        #[serde(default)]
        name: Option<String>,
    },
    /// 选择工作空间；`id=None` 表示“不使用工作空间”（普通任务）。
    SelectWorkspace {
        #[serde(default)]
        id: Option<String>,
    },
    /// 在当前工作空间内检索文件；@ 悬浮菜单用。
    SearchWorkspaceFiles {
        #[serde(default)]
        query: String,
        #[serde(default)]
        limit: Option<usize>,
    },
    /// 列出可用技能（当前工具契约）；/ 悬浮菜单用。
    ListSkills,
    /// 中断当前会话正在执行的回合。
    CancelSession { session_id: String },
    /// 删除会话。
    DeleteSession { session_id: String },
    /// 列出连接器（描述 + live 健康）——侧栏「专家·技能·连接器」面板用。
    ListConnectors,
    /// U15 列出插件（已装真实清单 + 市场）——插件管理 master-detail 视图用。
    ListPlugins,
    /// U15 安装插件（用户在详情面板点「安装」；内嵌 manifest 直接写入。点击即人工授权）。
    InstallPlugin { manifest: serde_json::Value },
    /// U15 卸载插件（用户在详情面板确认「卸载」；删 `<plugins>/<name>/`）。
    UninstallPlugin { name: String },
    /// U15 启用/禁用插件（详情面板 toggle；禁用保留清单文件，仅从注册表热卸载）。
    TogglePlugin { name: String, enabled: bool },
    /// B2 列出可选模型（模型选择器：当前 + 同 provider 候选 + demo）。
    ListModels,
    /// B2 切换模型（热换 + 持久化 providers.json；`model=="demo"` 换离线演示）。
    /// `provider_id` 缺省 = 激活条目；指定 = 在该 provider 条目上换模型（多 provider 菜单用）。
    SetModel {
        model: String,
        #[serde(default)]
        provider_id: Option<String>,
    },
    /// 多 provider：列出全部命名 provider 条目（模型菜单分组 + 配置面板左列共用）。
    ListProviders,
    /// 多 provider：删除一个自定义条目（内置不可删；删激活条目时激活回落）。
    DeleteProvider { id: String },
    /// 多 provider：整体切换激活条目（持久化 + 热换模型槽）。
    SetActiveProvider { id: String },
    /// 运行时切换两旋钮（沙箱能力 × 审批许可；其余 Policy 项不动）。
    /// `sandbox`: `read-only|workspace-write|danger-full-access`；`approval`: `never|on-request|unless-trusted`。
    SetPolicy { sandbox: String, approval: String },
    /// B2 读取完整模型配置（api_key 脱敏），供配置面板填充表单。
    /// `id` 缺省 = 激活 provider（旧行为兼容）；有值 = 指定条目（多 provider 面板）。
    GetModelConfig {
        #[serde(default)]
        id: Option<String>,
    },
    /// B2 保存模型配置（配置面板「保存」调用）——按 id upsert 多 provider 条目。
    /// `id` 缺省/空 = 新建（name 必填）；`name` 更新时空缺沿用旧名。
    SetModelConfig {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        name: Option<String>,
        base_url: String,
        model: String,
        #[serde(default)]
        temperature: Option<f32>,
        #[serde(default)]
        timeout_ms: Option<u64>,
        /// "keep" 保留该条目现有 key；"set" 使用 api_key_value。
        #[serde(default = "default_keep")]
        api_key_action: String,
        #[serde(default)]
        api_key_value: Option<String>,
    },
    /// 登录（对接门户 /api/auth/login）。成功后前门持有当前用户。
    Login { username: String, password: String },
    /// 取当前登录用户（前端启动时填充用户菜单；未登录 data.user=null）。
    /// `user.must_change_password=true` 时前端应弹框提醒修改密码。
    CurrentUser,
    /// 修改密码（对接门户 /api/auth/change-password）。门户改密成功即吊销全部 token，
    /// 后端同步本地登出——前端收到 ok 后引导重新登录（`data.relogin=true`）。
    ChangePassword {
        old_password: String,
        new_password: String,
    },
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
    /// IM 绑定：为当前登录用户生成一次性验证码（前端引导用户把码发到 IM 机器人完成绑定）。
    ImBindGenCode,
    /// IM 绑定：列出当前登录用户已绑定的 IM 身份（provider/open_id/created_at）。
    ImListBindings,
    /// IM 绑定：解绑一个 IM 身份。
    ImUnbind { provider: String, open_id: String },
}

fn default_keep() -> String {
    "keep".to_string()
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

/// 解析 kebab-case 枚举字符串（set_policy 的 sandbox/approval 参数），带合法值提示。
fn parse_enum<T: serde::de::DeserializeOwned>(field: &str, raw: &str) -> Result<T, String> {
    let valid = match field {
        "sandbox" => "read-only|workspace-write|danger-full-access",
        "approval" => "never|on-request|unless-trusted",
        _ => "unknown",
    };
    serde_json::from_value(serde_json::json!(raw))
        .map_err(|e| format!("{field} 无效（合法值：{valid}）：{e}"))
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
        AppRequest::ListWorkspaces => Ok(AppResponse::ok(app.list_workspaces()?)),
        AppRequest::CreateWorkspace { name } => Ok(AppResponse::ok(app.create_workspace(&name)?)),
        AppRequest::AddLocalWorkspace { path, name } => {
            Ok(AppResponse::ok(app.add_local_workspace(&path, name.as_deref())?))
        }
        AppRequest::SelectWorkspace { id } => Ok(AppResponse::ok(
            app.select_workspace(id.as_deref())?,
        )),
        AppRequest::SearchWorkspaceFiles { query, limit } => {
            let files = app.search_workspace_files(&query, limit).await?;
            Ok(AppResponse::ok(serde_json::json!({ "files": files })))
        }
        AppRequest::ListSkills => Ok(AppResponse::ok(app.list_skills())),
        AppRequest::CancelSession { session_id } => Ok(AppResponse::ok(
            serde_json::json!({ "cancelled": app.cancel_session_turn(&session_id) }),
        )),
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
        AppRequest::ListPlugins => Ok(AppResponse::ok(app.list_plugins().await)),
        AppRequest::InstallPlugin { manifest } => Ok(AppResponse::ok(app.install_plugin(&manifest).await?)),
        AppRequest::UninstallPlugin { name } => Ok(AppResponse::ok(app.uninstall_plugin(&name)?)),
        AppRequest::TogglePlugin { name, enabled } => {
            Ok(AppResponse::ok(app.toggle_plugin(&name, enabled)?))
        }
        AppRequest::ListModels => Ok(AppResponse::ok(app.list_models())),
        AppRequest::SetModel { model, provider_id } => {
            Ok(AppResponse::ok(app.set_model(&model, provider_id.as_deref())?))
        }
        AppRequest::ListProviders => Ok(AppResponse::ok(app.list_providers())),
        AppRequest::DeleteProvider { id } => Ok(AppResponse::ok(app.delete_provider(&id)?)),
        AppRequest::SetActiveProvider { id } => Ok(AppResponse::ok(app.set_active_provider(&id)?)),
        AppRequest::SetPolicy { sandbox, approval } => {
            let s: cmx_agent_core::SandboxMode = parse_enum("sandbox", &sandbox)
                .map_err(AppError::BadRequest)?;
            let a: cmx_agent_core::ApprovalPolicy = parse_enum("approval", &approval)
                .map_err(AppError::BadRequest)?;
            Ok(AppResponse::ok(app.set_policy(s, a)?))
        }
        AppRequest::GetModelConfig { id } => Ok(AppResponse::ok(app.get_model_config(id.as_deref()))),
        AppRequest::SetModelConfig { id, name, base_url, model, temperature, timeout_ms, api_key_action, api_key_value } => {
            let mut payload = serde_json::json!({
                "base_url": base_url,
                "model": model,
                "api_key_action": api_key_action,
            });
            if let Some(i) = id { payload["id"] = serde_json::json!(i); }
            if let Some(n) = name { payload["name"] = serde_json::json!(n); }
            if let Some(t) = temperature { payload["temperature"] = serde_json::json!(t); }
            if let Some(ms) = timeout_ms { payload["timeout_ms"] = serde_json::json!(ms); }
            if let Some(k) = api_key_value { payload["api_key_value"] = serde_json::json!(k); }
            Ok(AppResponse::ok(app.set_model_config(payload)?))
        }
        AppRequest::Login { username, password } => {
            let user = app.login(&username, &password).await?;
            Ok(AppResponse::ok(serde_json::json!({ "user": user })))
        }
        AppRequest::CurrentUser => Ok(AppResponse::ok(
            serde_json::json!({ "user": app.current_user() }),
        )),
        AppRequest::ChangePassword { old_password, new_password } => {
            Ok(AppResponse::ok(app.change_password(&old_password, &new_password).await?))
        }
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
        AppRequest::ImBindGenCode => Ok(AppResponse::ok(app.im_bind_gen_code().await?)),
        AppRequest::ImListBindings => Ok(AppResponse::ok(app.im_list_bindings().await?)),
        AppRequest::ImUnbind { provider, open_id } => {
            Ok(AppResponse::ok(app.im_unbind(&provider, &open_id).await?))
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
