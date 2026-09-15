//! 前门命令协议（同核多壳的接缝）。定义一组 **JSON 请求/响应**，由 [`dispatch`] 派发到 [`AgentApp`]。
//!
//! 这正是 Tauri `invoke("cmd", args)` 的边界：桌面壳前端发一个 `AppRequest` JSON，Rust 侧
//! `dispatch` 后回一个 `AppResponse` JSON。**同一协议**也服务 CLI 与后续 Headless HTTP——
//! 换壳不换核。所有变体 `tag = "cmd"`，snake_case（对齐 codex/规则引擎 rename_all 约定）。

use crate::app::{AgentApp, SendOutcome};

use std::path::PathBuf;
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
    /// 打开「助理」：确保 IM 统一会话存在（default 空间、固定标题）后返回其 id。
    /// 桌面侧栏「助理」入口点击时先调本命令再进会话（双壳共用，见侧栏 nav）。
    OpenAssistantSession,
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
    /// 调起操作系统原生文件夹选择器（Web 壳由本机后端进程弹出对话框）；用户取消返回 `picked: null`。
    /// Tauri 桌面壳有自己的同名 invoke 命令实现，不会走到这里。
    PickLocalDirectory,
    /// 把空间移出列表（不删磁盘目录）；`id` 不允许为内置 default。
    RemoveWorkspace { id: String },
    /// 用系统文件浏览器打开空间文件夹（侧栏空间菜单「打开文件夹」）。
    OpenWorkspaceFolder { id: String },
    /// 选择工作空间；`id=None` 表示“不使用工作空间”（普通任务）。
    SelectWorkspace {
        #[serde(default)]
        id: Option<String>,
    },
    /// 检索文件供 @ 悬浮菜单选择；带 `session_id` 时按该会话所属空间检索（跨空间任务
    /// 的 @ 提示不能看错根），缺省按当前空间。
    SearchWorkspaceFiles {
        #[serde(default)]
        query: String,
        #[serde(default)]
        limit: Option<usize>,
        #[serde(default)]
        session_id: Option<String>,
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
    /// 沙箱应用级配置（S0，方案 §6.3）：读取 SandboxSettings 快照（持久化落 data_dir/settings.json）。
    GetSandboxSettings,
    /// 沙箱应用级配置：整体替换并持久化（与 set_policy 的内存旋钮分层——重启不丢）。
    SetSandboxSettings {
        net: String,
        #[serde(default)]
        require_os: Option<bool>,
        #[serde(default)]
        cmd_risk_screen: Option<bool>,
        #[serde(default)]
        win_cache_policy: Option<String>,
        #[serde(default)]
        extra_write_roots: Vec<String>,
        #[serde(default)]
        hardened_read: bool,
    },
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
    /// `note` 为用户附言（ZCode 式「告诉模型接下来应该怎么做」），拒绝时回灌给模型帮其自愈。
    Approve {
        call_id: String,
        approved: bool,
        #[serde(default)]
        all: bool,
        #[serde(default)]
        session_id: String,
        #[serde(default)]
        note: String,
    },
    /// 人在环提问答复：前端在答题卡点「提交」后发来，唤醒挂起的回合。
    /// `answers` 按问题顺序，每问一个字符串数组（多选 = 多个 label；自由输入 = 一条
    /// `"user_note: …"`）。未命中待决提问时响应 `resolved:false`（前端降级为已失效态）。
    AnswerQuestion {
        request_id: String,
        answers: Vec<Vec<String>>,
        #[serde(default)]
        session_id: String,
    },
    /// 人在环提问忽略：前端在答题卡点「忽略」后发来，回合以 dismissed 继续不中止。
    DismissQuestion {
        request_id: String,
        #[serde(default)]
        session_id: String,
    },
    /// 列待决提问（前端在途恢复主路径：刷新/重开窗口后挂起中的提问不在落库事件里，
    /// 只能查进程内 pending）。`session_id` 缺省列全部会话的待决提问。
    ListPendingQuestions {
        #[serde(default)]
        session_id: Option<String>,
    },
    /// 列某会话待决审批（前端在途恢复：刷新后重画审批卡）。`session_id` 为空时返回空列表。
    ListPendingApprovals {
        #[serde(default)]
        session_id: String,
    },
    /// IM 绑定：为当前登录用户生成一次性验证码（前端引导用户把码发到 IM 机器人完成绑定）。
    ImBindGenCode,
    /// IM 绑定：列出当前登录用户已绑定的 IM 身份（provider/open_id/created_at）。
    ImListBindings,
    /// IM 绑定：解绑一个 IM 身份。
    ImUnbind { provider: String, open_id: String },
    /// 阶段一：列子智能体（内置两条带覆盖项 + 自定义）——设置中心「子智能体」分区。
    ListAgents,
    /// 阶段一：保存子智能体。内置只允许改 model/enabled（协议层校验，其余字段忽略）；
    /// 自定义按 name upsert（snake_case 校验、撞内置名拒绝）。保存即热生效。
    SaveAgent { spec: cmx_agent_core::agents::AgentSpec },
    /// 阶段一：删除一个自定义子智能体（内置/未知 → 拒绝）。
    DeleteAgent { name: String },
    /// 阶段二：切换会话计划模式（**仅用户可调**——UI chip；模型无任何切换工具）。
    /// im-assistant / im-* 会话拒绝；与回合生命周期串行（session_locks permit）。
    SetPlanMode {
        session_id: String,
        enabled: bool,
    },
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
    // 前门硬登录门：配置了门户认证的双壳里，除登录/查当前用户外一律要求已认证。此前登录门
    // 只存在于前端路由——任何能到达前门的执行体（Web 壳的跨站请求、注入脚本）都能 set_policy
    // 拆沙箱、add_local_workspace 挂任意目录、install_plugin 装插件。
    // 未配置认证（CLI serve / 单元测试的本地单机模式）不强制。
    if app.auth_configured()
        && !app.is_authenticated()
        && !matches!(req, AppRequest::Login { .. } | AppRequest::CurrentUser)
    {
        return Err(AppError::Auth("未登录：请先登录门户账号".into()));
    }
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
        AppRequest::OpenAssistantSession => {
            let id = app.open_assistant_session()?;
            Ok(AppResponse::ok(serde_json::json!({ "session_id": id })))
        }
        AppRequest::ListWorkspaces => Ok(AppResponse::ok(app.list_workspaces()?)),
        AppRequest::CreateWorkspace { name } => Ok(AppResponse::ok(app.create_workspace(&name)?)),
        AppRequest::AddLocalWorkspace { path, name } => {
            Ok(AppResponse::ok(app.add_local_workspace(&path, name.as_deref())?))
        }
        AppRequest::PickLocalDirectory => {
            // 系统选择器是阻塞对话框，放阻塞线程池跑，避免占住异步运行时 worker。
            let picked =
                tokio::task::spawn_blocking(crate::workspace::native_pick_local_directory)
                    .await
                    .map_err(|e| AppError::BadRequest(format!("文件夹选择任务失败：{e}")))?
                    .map_err(AppError::BadRequest)?;
            Ok(AppResponse::ok(serde_json::json!({ "picked": picked })))
        }
        AppRequest::RemoveWorkspace { id } => {
            Ok(AppResponse::ok(app.remove_workspace(&id)?))
        }
        AppRequest::OpenWorkspaceFolder { id } => {
            Ok(AppResponse::ok(app.open_workspace_folder(&id)?))
        }
        AppRequest::SelectWorkspace { id } => Ok(AppResponse::ok(
            app.select_workspace(id.as_deref())?,
        )),
        AppRequest::SearchWorkspaceFiles { query, limit, session_id } => {
            let files = app.search_workspace_files(&query, limit, session_id.as_deref()).await?;
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
        AppRequest::GetSandboxSettings => {
            let s = cmx_agent_sandbox::settings::get();
            Ok(AppResponse::ok(serde_json::json!({
                "net": s.net.as_str(),
                "require_os": s.require_os,
                "cmd_risk_screen": s.cmd_risk_screen,
                "win_cache_policy": match s.win_cache_policy {
                    cmx_agent_sandbox::WinCachePolicy::Redirect => "redirect",
                    cmx_agent_sandbox::WinCachePolicy::Allow => "allow",
                    cmx_agent_sandbox::WinCachePolicy::Deny => "deny",
                },
                "extra_write_roots": s.extra_write_roots.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "hardened_read": s.hardened_read,
            })))
        }
        AppRequest::SetSandboxSettings { net, require_os, cmd_risk_screen, win_cache_policy, extra_write_roots, hardened_read } => {
            let mut s = cmx_agent_sandbox::settings::get();
            s.net = match net.to_lowercase().as_str() {
                "open" => cmx_agent_sandbox::NetMode::Open,
                "poison" => cmx_agent_sandbox::NetMode::Poison,
                "enforce" => cmx_agent_sandbox::NetMode::Enforce,
                other => return Err(AppError::BadRequest(format!("sandbox.net 非法值 '{other}'（open|poison|enforce）"))),
            };
            if let Some(v) = require_os { s.require_os = v; }
            if let Some(v) = cmd_risk_screen { s.cmd_risk_screen = v; }
            if let Some(p) = win_cache_policy {
                s.win_cache_policy = match p.to_lowercase().as_str() {
                    "redirect" => cmx_agent_sandbox::WinCachePolicy::Redirect,
                    "allow" => cmx_agent_sandbox::WinCachePolicy::Allow,
                    "deny" => cmx_agent_sandbox::WinCachePolicy::Deny,
                    other => return Err(AppError::BadRequest(format!("win_cache_policy 非法值 '{other}'"))),
                };
            }
            // extra_write_roots 防线校验（红队3 P1-2）：`..` 穿越/根路径/超量直接拒——
            // 该字段等价于扩写围栏，不能静默单请求拆沙箱。
            if extra_write_roots.len() > 16 {
                return Err(AppError::BadRequest("extra_write_roots 超过 16 条上限".into()));
            }
            for e in &extra_write_roots {
                let bad = e.is_empty()
                    || e.contains("..")
                    || e == "/"
                    || e == "\\"
                    || (e.len() >= 2 && e.as_bytes()[1] == b':');
                if bad {
                    return Err(AppError::BadRequest(format!(
                        "extra_write_roots 非法条目 '{e}'（空串/../盘符根/卷根不允许）"
                    )));
                }
            }
            s.extra_write_roots = extra_write_roots.into_iter().map(PathBuf::from).collect();
            s.hardened_read = hardened_read;
            let persisted = cmx_agent_sandbox::settings::set(s.clone()).map_err(AppError::BadRequest)?;
            Ok(AppResponse::ok(serde_json::json!({ "saved": true, "persisted": persisted, "net": s.net.as_str() })))
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
        AppRequest::Approve { call_id, approved, all, session_id, note } => {
            let hit = app.resolve_approval_decision(&call_id, approved, all, &session_id, &note);
            Ok(AppResponse::ok(
                serde_json::json!({ "resolved": hit, "call_id": call_id, "approved": approved, "all": all }),
            ))
        }
        AppRequest::AnswerQuestion { request_id, answers, session_id } => {
            let hit = app.answer_question(&request_id, answers, &session_id);
            Ok(AppResponse::ok(
                serde_json::json!({ "resolved": hit, "request_id": request_id }),
            ))
        }
        AppRequest::DismissQuestion { request_id, session_id } => {
            let hit = app.dismiss_question(&request_id, &session_id);
            Ok(AppResponse::ok(
                serde_json::json!({ "resolved": hit, "request_id": request_id }),
            ))
        }
        AppRequest::ListPendingQuestions { session_id } => {
            let pending = app.list_pending_questions(session_id.as_deref());
            Ok(AppResponse::ok(serde_json::json!({ "pending": pending })))
        }
        AppRequest::ListPendingApprovals { session_id } => {
            let pending = app.list_pending_approvals(&session_id);
            Ok(AppResponse::ok(serde_json::json!({ "pending": pending })))
        }
        AppRequest::ImBindGenCode => Ok(AppResponse::ok(app.im_bind_gen_code().await?)),
        AppRequest::ImListBindings => Ok(AppResponse::ok(app.im_list_bindings().await?)),
        AppRequest::ImUnbind { provider, open_id } => {
            Ok(AppResponse::ok(app.im_unbind(&provider, &open_id).await?))
        }
        AppRequest::ListAgents => Ok(AppResponse::ok(app.list_agents()?)),
        AppRequest::SaveAgent { spec } => Ok(AppResponse::ok(app.save_agent(spec)?)),
        AppRequest::DeleteAgent { name } => Ok(AppResponse::ok(app.delete_agent(&name)?)),
        AppRequest::SetPlanMode { session_id, enabled } => {
            Ok(AppResponse::ok(app.set_plan_mode(&session_id, enabled).await?))
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
