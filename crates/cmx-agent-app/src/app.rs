//! 应用 façade [`AgentApp`]：桌面壳后端的单一入口。持有一个共享 `Agent` + 一个 `SessionStore`，
//! 对外提供"新建会话 / 发一条消息 / 列会话 / 读会话 / 删会话"等**用例级**操作，并在每个回合后
//! **增量落库**（只 append 新事件，不重写历史）。
//!
//! 这一层与前门无关（同核多壳）：CLI、Tauri invoke、Headless HTTP 都调它。见 [`crate::protocol`]。

use std::sync::Arc;
use std::sync::Mutex;

use cmx_agent_connectors::{AuthProvider, ConnectorCard, ConnectorRegistry, LoggedInUser};
use cmx_agent_core::event::StopReason;
use cmx_agent_core::{Agent, Session};

use crate::error::{AppError, AppResult};
use crate::store::{SessionMeta, SessionStore};

/// 一个回合的对外结果（含新产生的事件条数，便于前门增量渲染）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SendOutcome {
    pub session_id: String,
    pub turn: u64,
    pub reason: StopReason,
    pub steps: usize,
    pub final_text: Option<String>,
    /// 本回合新增事件（供前门直接展示/审计，无需重读全量日志）。
    pub new_events: Vec<cmx_agent_core::event::SessionEvent>,
}

/// 应用 façade。`Agent` 无内部可变状态（回合号从会话派生），故可 `Arc` 共享给多会话/多前门。
pub struct AgentApp {
    agent: Arc<Agent>,
    store: Arc<dyn SessionStore>,
    default_system: Option<String>,
    /// 连接器注册表（面板查询用）。None = 未启用连接器。
    connectors: Option<Arc<ConnectorRegistry>>,
    /// 认证提供者（对接门户 /api/auth/*）。None = 未启用登录门。
    auth: Option<Arc<AuthProvider>>,
    /// 交互式审批者（X4）。Some = 桌面壳交互审批；None = 非交互（CLI 自动审批）。
    approver: Option<Arc<crate::approval::InteractiveApprover>>,
    /// 当前登录用户（登录后置入；登出清空）。跨命令共享，故用 Mutex。
    current_user: Arc<Mutex<Option<LoggedInUser>>>,
    /// 共享令牌槽：登录成功写入 access_token，连接器调用时读出带 Bearer（登出清空）。
    /// 与 ConnectorRegistry 的 client 共享同一 Arc，故登录即对所有连接器生效。
    token_store: Option<cmx_agent_connectors::TokenStore>,
    /// U15 已装插件摘要（加载时快照；plugins_dir 未设时的兜底）。
    plugins: Vec<serde_json::Value>,
    /// U15 plugins 目录（设了则 list_plugins 实时扫描、install/uninstall 落到此）。
    plugins_dir: Option<std::path::PathBuf>,
    /// U15 远程市场 URL（env CMX_AGENT_PLUGIN_MARKET；list_plugins 有则拉取市场目录）。
    plugin_market: Option<String>,
    /// U13 数据权限 PEP（启用数据权限时 Some）：登录后按真实用户重热 PDP 判定。
    data_auth_pep: Option<Arc<cmx_agent_connectors::DataAuthPep>>,
    /// U13 共享授权主体：与 AuthGuard 闭包共享同一 Arc；登录写真实用户(userId+roles)、登出复位。
    auth_identity: Option<Arc<std::sync::RwLock<cmx_agent_core::Subject>>>,
    /// B2 可热换模型槽（与 Agent 共享）：模型选择器切换模型即时生效。
    model_slot: Option<crate::ModelSlot>,
    /// B2 模型配置目录（含 providers.json / model.json）：切换模型后持久化。
    model_config_dir: Option<std::path::PathBuf>,
    /// Web 壳启用 per-user 配置后的基目录：`<base>/<username>/providers.json`。
    /// Tauri 壳不设此字段，始终用 model_config_dir（单机单用户）。
    user_config_base: Option<std::path::PathBuf>,
    /// 多 provider 配置读写锁：providers.json 的 load→改→save 序列串行化，防并发写坏。
    providers_lock: Mutex<()>,
    /// U16 会话事件总线：任意来源（本地 / IM 桥）追加事件时广播给订阅者（`/api/subscribe` SSE）。
    event_bus: Arc<crate::bus::SessionEventBus>,
    /// IM 绑定客户端（Some=启用绑定面板：gen_code/list/unbind 三命令）。
    im_binding: Option<cmx_agent_connectors::ImBindingClient>,
}

impl AgentApp {
    pub fn new(agent: Arc<Agent>, store: Arc<dyn SessionStore>) -> Self {
        Self {
            agent,
            store,
            default_system: None,
            connectors: None,
            auth: None,
            approver: None,
            current_user: Arc::new(Mutex::new(None)),
            token_store: None,
            plugins: Vec::new(),
            plugins_dir: None,
            plugin_market: None,
            data_auth_pep: None,
            auth_identity: None,
            model_slot: None,
            model_config_dir: None,
            user_config_base: None,
            providers_lock: Mutex::new(()),
            event_bus: Arc::new(crate::bus::SessionEventBus::new()),
            im_binding: None,
        }
    }

    pub fn with_default_system(mut self, system: impl Into<String>) -> Self {
        self.default_system = Some(system.into());
        self
    }

    /// 注入连接器注册表（由 DesktopAppBuilder 调用）。
    pub fn with_connectors(mut self, connectors: Arc<ConnectorRegistry>) -> Self {
        self.connectors = Some(connectors);
        self
    }

    /// 注入认证提供者（由 DesktopAppBuilder 调用）。
    pub fn with_auth(mut self, auth: Arc<AuthProvider>) -> Self {
        self.auth = auth.into();
        self
    }

    /// 注入共享令牌槽（由 DesktopAppBuilder 调用，与连接器 client 共享）。登录写、登出清。
    pub fn with_token_store(mut self, store: cmx_agent_connectors::TokenStore) -> Self {
        self.token_store = Some(store);
        self
    }

    /// 注入已装插件摘要（U15；由 DesktopAppBuilder 从 plugins 目录清单派生）。
    pub fn with_plugins(mut self, plugins: Vec<serde_json::Value>) -> Self {
        self.plugins = plugins;
        self
    }

    /// 注入 plugins 目录（U15；设了则 list_plugins 实时扫描、install/uninstall 落到此）。
    pub fn with_plugins_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.plugins_dir = Some(dir.into());
        self
    }

    /// 注入远程市场 URL（U15；None/空 = 无市场，list_plugins 返回空市场）。
    pub fn with_plugin_market(mut self, url: Option<String>) -> Self {
        self.plugin_market = url.filter(|s| !s.is_empty());
        self
    }

    /// U13：注入数据权限 PEP + 共享授权主体（由 DesktopAppBuilder 在启用数据权限时调用）。
    /// 之后 `login` 会把主体换成真实登录用户(userId+roles)并重热 PDP；`logout` 复位为无角色（fail-closed）。
    pub fn with_data_auth_identity(
        mut self,
        pep: Arc<cmx_agent_connectors::DataAuthPep>,
        identity: Arc<std::sync::RwLock<cmx_agent_core::Subject>>,
    ) -> Self {
        self.data_auth_pep = Some(pep);
        self.auth_identity = Some(identity);
        self
    }

    /// B2：注入可热换模型槽 + 配置目录（由 DesktopAppBuilder 调用）。模型选择器据此列出/切换模型。
    pub fn with_model(mut self, slot: crate::ModelSlot, config_dir: impl Into<std::path::PathBuf>) -> Self {
        self.model_slot = Some(slot);
        self.model_config_dir = Some(config_dir.into());
        self
    }

    /// 启用 per-user 模型配置基目录（Web 壳由 DesktopAppBuilder::user_config_base 注入）。
    pub fn with_user_config_base(mut self, base: impl Into<std::path::PathBuf>) -> Self {
        self.user_config_base = Some(base.into());
        self
    }

    /// 注入交互式审批者（由 DesktopAppBuilder 调用，桌面壳交互审批用）。
    pub fn with_approver(mut self, approver: Arc<crate::approval::InteractiveApprover>) -> Self {
        self.approver = Some(approver);
        self
    }

    /// 注入 IM 绑定客户端（桌面壳「设置 → IM 绑定」面板用；由壳装配，配了才有绑定命令）。
    pub fn with_im_binding(mut self, client: cmx_agent_connectors::ImBindingClient) -> Self {
        self.im_binding = Some(client);
        self
    }

    /// 当前登录用户的 access_token（绑定命令用；从共享令牌槽读）。
    fn current_token(&self) -> AppResult<String> {
        self.token_store
            .as_ref()
            .and_then(|ts| ts.read().ok().and_then(|g| g.clone()))
            .ok_or_else(|| AppError::Auth("请先登录".into()))
    }

    /// IM 绑定：为当前登录用户生成一次性验证码（前端引导用户把码发到 IM 机器人完成绑定）。
    pub async fn im_bind_gen_code(&self) -> AppResult<serde_json::Value> {
        let client = self
            .im_binding
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("未启用 IM 绑定".into()))?;
        let token = self.current_token()?;
        let (code, expires_in) = client
            .gen_code(&token)
            .await
            .map_err(|e| AppError::Auth(format!("生成绑定验证码失败：{e}")))?;
        Ok(serde_json::json!({ "code": code, "expires_in": expires_in }))
    }

    /// IM 绑定：列出当前登录用户已绑定的 IM 身份。
    pub async fn im_list_bindings(&self) -> AppResult<serde_json::Value> {
        let client = self
            .im_binding
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("未启用 IM 绑定".into()))?;
        let token = self.current_token()?;
        let items = client
            .list(&token)
            .await
            .map_err(|e| AppError::Auth(format!("查询绑定失败：{e}")))?;
        Ok(serde_json::json!({ "items": items }))
    }

    /// IM 绑定：解绑一个 IM 身份。
    pub async fn im_unbind(&self, provider: &str, open_id: &str) -> AppResult<serde_json::Value> {
        let client = self
            .im_binding
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("未启用 IM 绑定".into()))?;
        let token = self.current_token()?;
        client
            .unbind(&token, provider, open_id)
            .await
            .map_err(|e| AppError::Auth(format!("解绑失败：{e}")))?;
        Ok(serde_json::json!({ "unbound": true }))
    }

    /// 前端送回审批决定（`approve` 命令）：唤醒挂起的回合。返回是否命中一个待决审批。
    /// 前端送回审批决定。`all=true` 表示「本对话全部允许」：先把该会话标记为全部允许
    /// （此后该会话需审批的工具全部自动放行、不再弹卡），再放行当前这次调用。
    pub fn resolve_approval_decision(
        &self,
        call_id: &str,
        approved: bool,
        all: bool,
        session_id: &str,
    ) -> bool {
        match &self.approver {
            Some(a) => {
                if all && approved && !session_id.is_empty() {
                    a.allow_session(session_id);
                }
                a.decide(call_id, approved)
            }
            None => false,
        }
    }

    /// 登录：校验凭据（对接门户 /api/auth/login）→ 成功后置入当前用户 → 返回前端可见用户信息（不含令牌）。
    pub async fn login(&self, username: &str, password: &str) -> AppResult<serde_json::Value> {
        let auth = self
            .auth
            .as_ref()
            .ok_or_else(|| AppError::Auth("未配置认证服务".into()))?;
        if username.trim().is_empty() || password.is_empty() {
            return Err(AppError::Auth("请输入用户名和密码".into()));
        }
        let user = auth
            .login(username.trim(), password)
            .await
            .map_err(|e| AppError::Auth(friendly_auth_error(e, auth.base_url())))?;
        let public = user.public_json();
        // 把 access_token 写入共享令牌槽 → 之后连接器读写自动带 Bearer（auth=on 服务如 cmx-flow 必需）。
        if let Some(ts) = &self.token_store
            && let Ok(mut g) = ts.write() {
                *g = Some(user.access_token.clone());
            }
        // U13：把授权主体换成真实登录用户(userId+roles)并重热 PDP → 数据权限门按此人判定。
        if let (Some(pep), Some(identity)) = (&self.data_auth_pep, &self.auth_identity) {
            let subj = {
                let mut s = cmx_agent_core::Subject::new(user.user_id.clone());
                s.roles = user.roles.clone();
                s
            };
            if let Ok(mut g) = identity.write() {
                *g = subj.clone();
            }
            pep.prewarm(&subj, cmx_agent_connectors::ENFORCED_PERMS).await;
        }
        *self.current_user.lock().expect("current_user lock") = Some(user);
        Ok(public)
    }

    /// 当前登录用户（前端可见信息，不含令牌）。未登录返回 None。
    pub fn current_user(&self) -> Option<serde_json::Value> {
        self.current_user
            .lock()
            .expect("current_user lock")
            .as_ref()
            .map(|u| u.public_json())
    }

    /// 当前登录用户的授权主体（userId+roles）。IM 个人模式用：桥直接以此身份
    /// `send_as` 跑回合（数据权限按桌面登录人判定），不经绑定验证码。未登录 None。
    pub fn current_subject(&self) -> Option<cmx_agent_core::Subject> {
        let u = self
            .current_user
            .lock()
            .expect("current_user lock")
            .clone()?;
        let mut s = cmx_agent_core::Subject::new(u.user_id);
        s.roles = u.roles;
        Some(s)
    }

    /// 是否已登录。
    pub fn is_authenticated(&self) -> bool {
        self.current_user
            .lock()
            .expect("current_user lock")
            .is_some()
    }

    /// 登出：清当前用户（本地态；令牌失效由服务端会话过期兜底）。
    pub fn logout(&self) {
        *self.current_user.lock().expect("current_user lock") = None;
        // 清共享令牌槽 → 连接器回落到未认证（X-Tenant）态。
        if let Some(ts) = &self.token_store
            && let Ok(mut g) = ts.write() {
                *g = None;
            }
        // U13：授权主体复位为无角色匿名 → 数据权限门 fail-closed（登出后写操作被 PDP 拒）。
        if let Some(identity) = &self.auth_identity
            && let Ok(mut g) = identity.write() {
                *g = cmx_agent_core::Subject::new("anon");
            }
    }

    /// 列出连接器卡片（描述 + live 健康）。未启用连接器时返回空表。
    /// 返回当前有效的模型配置目录。
    /// - Web 壳且已登录：`<user_config_base>/<username>/`（自动创建）
    /// - 其他（Tauri 或未登录）：`model_config_dir`（原有全局路径）
    fn effective_model_config_dir(&self) -> Option<std::path::PathBuf> {
        if let Some(dir) = self
            .user_config_base
            .as_ref()
            .zip(self
                .current_user
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|u| u.username.clone())))
            .map(|(base, username)| base.join(&username))
        {
            std::fs::create_dir_all(&dir).ok();
            return Some(dir);
        }
        self.model_config_dir.clone()
    }

    /// 读当前 providers.json（多 provider 配置）。必须在 `providers_lock` 持锁下调用。
    /// None = 无配置目录（理论不可达，调用方兜底 BadRequest）。
    fn load_providers(&self) -> Option<(std::path::PathBuf, cmx_agent_model::ProviderFile)> {
        let dir = self.effective_model_config_dir()?;
        Some((dir.clone(), cmx_agent_model::ProviderFile::load(&dir)))
    }

    /// 取一个命名 provider 的面板回填 JSON（掩码 key + 候选模型）。None = id 不存在。
    fn provider_config_json(pf: &cmx_agent_model::ProviderFile, p: &cmx_agent_model::NamedProvider) -> serde_json::Value {
        let candidates: Vec<String> = p.config.candidate_models().iter().map(|s| s.to_string()).collect();
        serde_json::json!({
            "configured": !p.config.api_key.is_empty() || !p.config.base_url.is_empty(),
            "id": p.id,
            "name": p.name,
            "builtin": p.builtin,
            "active": pf.active_id() == Some(p.id.as_str()),
            "base_url": p.config.base_url,
            "api_key_masked": p.config.masked_api_key(),
            "model": p.config.model,
            "temperature": p.config.temperature,
            "timeout_ms": p.config.timeout_ms,
            "candidates": candidates,
        })
    }

    /// B2：读取完整模型配置（api_key 脱敏），供前端配置面板填充表单。
    /// `id` 为空取当前激活 provider（旧行为兼容）；有 id 取指定条目（多 provider 面板）。
    pub fn get_model_config(&self, id: Option<&str>) -> serde_json::Value {
        let _g = self.providers_lock.lock().expect("providers lock");
        if let Some((_, pf)) = self.load_providers() {
            let target = id
                .and_then(|i| pf.get(i))
                .or_else(|| pf.active());
            if let Some(p) = target {
                return Self::provider_config_json(&pf, p);
            }
            if id.is_some() {
                return serde_json::json!({ "configured": false, "error": "provider 不存在" });
            }
        }
        // 无配置目录 / 无激活条目：空表单（demo 兜底，与旧未配置行为一致）。
        serde_json::json!({
            "configured": false,
            "base_url": "",
            "api_key_masked": "",
            "model": "",
            "temperature": 0.2_f32,
            "timeout_ms": 60000_u64,
            "candidates": [],
        })
    }

    /// B2：保存模型配置（前端配置面板「保存」按钮调用）——**按 id upsert** 多 provider 条目。
    ///
    /// payload 字段：
    /// - `id`：可选。缺省/空 = 新建（自动生成 `p-<nanos>`）；有值 = 更新该条目（不存在则报错）
    /// - `name`：用户可见名称。新建必填非空且不得与其他条目重名；更新时空缺沿用旧名
    /// - `base_url` / `model`：必填
    /// - `temperature`：浮点（可选，缺省 0.2）；`timeout_ms`：整数（可选，缺省 60000）
    /// - `api_key_action`："keep"（沿用该条目已存 key）| "set"（使用 `api_key_value`）
    ///
    /// 新建不自动激活；更新激活条目时热换模型槽立即生效。内置条目可编辑、不可由此删除。
    pub fn set_model_config(&self, payload: serde_json::Value) -> AppResult<serde_json::Value> {
        let get_str = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
        let base_url = get_str("base_url")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::BadRequest("base_url 不能为空".into()))?;
        let model = get_str("model")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::BadRequest("model 不能为空".into()))?;
        let temperature = payload.get("temperature").and_then(|v| v.as_f64()).map(|f| f as f32).unwrap_or(0.2);
        let timeout_ms = payload.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(60_000);
        let action = get_str("api_key_action").unwrap_or_else(|| "keep".into());
        let target_id = get_str("id").filter(|s| !s.is_empty());
        let name_in = get_str("name").filter(|s| !s.is_empty());

        let dir = self
            .effective_model_config_dir()
            .ok_or_else(|| AppError::BadRequest("模型配置目录不可用".into()))?;
        let _g = self.providers_lock.lock().expect("providers lock");
        let mut pf = cmx_agent_model::ProviderFile::load(&dir);

        // 更新已有条目：沿用旧 name/key；新建：name 必填 + 重名校验、key 可空（keyless 端点）。
        let (name, api_key, builtin, is_active) = match &target_id {
            Some(id) => {
                let p = pf
                    .get(id)
                    .ok_or_else(|| AppError::BadRequest("Provider 不存在（可能已被删除）".into()))?;
                (
                    name_in.unwrap_or_else(|| p.name.clone()),
                    if action == "set" { get_str("api_key_value").unwrap_or_default() } else { p.config.api_key.clone() },
                    p.builtin,
                    pf.active_id() == Some(id.as_str()),
                )
            }
            None => {
                let name = name_in
                    .ok_or_else(|| AppError::BadRequest("名称不能为空".into()))?;
                if pf.name_taken(&name, None) {
                    return Err(AppError::BadRequest(format!("名称「{name}」已存在，换一个")));
                }
                (name, get_str("api_key_value").unwrap_or_default(), false, false)
            }
        };

        let id = target_id.unwrap_or_else(cmx_agent_model::new_id);
        let cfg = cmx_agent_model::ModelProviderConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.clone(),
            temperature,
            timeout_ms,
        };
        pf.upsert(cmx_agent_model::NamedProvider { id: id.clone(), name: name.clone(), builtin, config: cfg.clone() });
        let persisted = pf.save(&dir).is_ok();
        // 仅激活条目热换（新建/非激活条目只落盘，切换由 set_active_provider 负责）。
        if is_active && let Some(slot) = &self.model_slot {
            slot.swap(std::sync::Arc::new(cmx_agent_model::OpenAiCompatModel::new(cfg.clone())));
        }
        let note = if !persisted {
            "已应用（本次会话；持久化失败）"
        } else if is_active {
            "已保存并立即生效"
        } else {
            "已保存（切换到该 Provider 后生效）"
        };
        Ok(serde_json::json!({
            "service": "cmx-model",
            "id": id,
            "current": model,
            "provider": name,
            "persisted": persisted,
            "note": note,
        }))
    }

    /// 多 provider：列出全部条目（模型菜单分组 + 配置面板左列共用）。
    /// `active` 标出当前激活条目；`candidates` 为该 provider 的候选模型（自定义 URL 可能为空，仅显示当前模型）。
    pub fn list_providers(&self) -> serde_json::Value {
        let _g = self.providers_lock.lock().expect("providers lock");
        let (dir, pf) = match self.load_providers() {
            Some(x) => x,
            None => return serde_json::json!({ "service": "cmx-model", "providers": [], "current": "demo" }),
        };
        let _ = dir;
        let providers: Vec<serde_json::Value> = pf
            .providers
            .iter()
            .map(|p| {
                let candidates: Vec<String> =
                    p.config.candidate_models().iter().map(|s| s.to_string()).collect();
                let mut cands: Vec<serde_json::Value> = candidates
                    .iter()
                    .map(|m| serde_json::json!({ "model": m, "label": m }))
                    .collect();
                // 当前模型保证在候选里（自定义模型名/未知 provider 时）。
                if !p.config.model.is_empty()
                    && !candidates.iter().any(|m| m == &p.config.model)
                {
                    cands.insert(0, serde_json::json!({ "model": p.config.model, "label": p.config.model }));
                }
                serde_json::json!({
                    "id": p.id,
                    "name": p.name,
                    "builtin": p.builtin,
                    "active": pf.active_id() == Some(p.id.as_str()),
                    "base_url": p.config.base_url,
                    "api_key_masked": p.config.masked_api_key(),
                    "configured_key": !p.config.api_key.is_empty(),
                    "model": p.config.model,
                    "candidates": cands,
                })
            })
            .collect();
        let current = pf
            .active()
            .map(|p| p.config.model.clone())
            .unwrap_or_else(|| "demo".to_string());
        serde_json::json!({ "service": "cmx-model", "current": current, "providers": providers })
    }

    /// 多 provider：删除一个自定义条目。内置条目不可删；删除激活条目时激活回落到剩余第一条
    /// （无剩余则回 demo），并热换模型槽。
    pub fn delete_provider(&self, id: &str) -> AppResult<serde_json::Value> {
        let dir = self
            .effective_model_config_dir()
            .ok_or_else(|| AppError::BadRequest("模型配置目录不可用".into()))?;
        let _g = self.providers_lock.lock().expect("providers lock");
        let mut pf = cmx_agent_model::ProviderFile::load(&dir);
        let p = pf
            .get(id)
            .ok_or_else(|| AppError::BadRequest("Provider 不存在".into()))?;
        if p.builtin {
            return Err(AppError::BadRequest("内置 Provider 不可删除".into()));
        }
        let was_active = pf.active_id() == Some(id);
        pf.remove(id);
        if was_active {
            pf.active = pf.providers.first().map(|p| p.id.clone());
        }
        pf.save(&dir).map_err(|e| AppError::BadRequest(format!("持久化失败：{e}")))?;
        // 热换：删除激活条目 → 换成新激活条目（或回 demo）。
        if was_active && let Some(slot) = &self.model_slot {
            let model = match pf.active().filter(|p| !p.config.base_url.is_empty()) {
                Some(p) => std::sync::Arc::new(cmx_agent_model::OpenAiCompatModel::new(p.config.clone()))
                    as std::sync::Arc<dyn cmx_agent_core::ModelSeam>,
                None => std::sync::Arc::new(crate::DemoModel),
            };
            slot.swap(model);
        }
        let note = if was_active {
            "已删除，激活已切换到剩余 Provider"
        } else {
            "已删除"
        };
        Ok(serde_json::json!({ "service": "cmx-model", "deleted": id, "active": pf.active, "note": note }))
    }

    /// 多 provider：整体切换激活条目（模型菜单按 provider 分组后的点击行为）。持久化 + 热换。
    pub fn set_active_provider(&self, id: &str) -> AppResult<serde_json::Value> {
        let dir = self
            .effective_model_config_dir()
            .ok_or_else(|| AppError::BadRequest("模型配置目录不可用".into()))?;
        let _g = self.providers_lock.lock().expect("providers lock");
        let mut pf = cmx_agent_model::ProviderFile::load(&dir);
        let p = pf
            .get(id)
            .ok_or_else(|| AppError::BadRequest("Provider 不存在".into()))?;
        let cfg = p.config.clone();
        let name = p.name.clone();
        pf.active = Some(id.to_string());
        pf.save(&dir).map_err(|e| AppError::BadRequest(format!("持久化失败：{e}")))?;
        if let Some(slot) = &self.model_slot {
            if cfg.base_url.is_empty() {
                slot.swap(std::sync::Arc::new(crate::DemoModel));
            } else {
                slot.swap(std::sync::Arc::new(cmx_agent_model::OpenAiCompatModel::new(cfg.clone())));
            }
        }
        Ok(serde_json::json!({
            "service": "cmx-model",
            "active": id,
            "current": cfg.model,
            "provider": name,
            "note": format!("已切换到「{name}」· {}", cfg.model),
        }))
    }

    pub async fn list_connectors(&self) -> Vec<ConnectorCard> {
        match &self.connectors {
            Some(cr) => cr.probe_all().await,
            None => vec![],
        }
    }

    /// B2：列出可选模型（前门 list_models → 模型选择器）。
    /// `current` = 当前生效模型（真实 provider 的 model，或 demo）；`candidates` = 同 provider 候选 + demo。
    pub fn list_models(&self) -> serde_json::Value {
        let cfg = cmx_agent_model::resolve_active(self.effective_model_config_dir().as_deref());
        let (current, provider, base_url) = match &cfg {
            Some(c) => (c.model.clone(), provider_label(&c.base_url), c.base_url.clone()),
            None => ("demo".to_string(), "离线演示".to_string(), String::new()),
        };
        let mut candidates: Vec<serde_json::Value> = Vec::new();
        if let Some(c) = &cfg {
            for m in c.candidate_models() {
                candidates.push(serde_json::json!({ "model": m, "label": m }));
            }
            // 保证当前模型在列表里（provider 未识别或自定义模型名时）
            if !candidates.iter().any(|x| x["model"] == current) {
                candidates.insert(0, serde_json::json!({ "model": current, "label": current }));
            }
        }
        candidates.push(serde_json::json!({ "model": "demo", "label": "离线演示（DemoModel）" }));
        serde_json::json!({
            "service": "cmx-model",
            "current": current,
            "provider": provider,
            "baseUrl": base_url,
            "configurable": cfg.is_some(),
            "candidates": candidates,
        })
    }

    /// B2：切换模型（前门 set_model）。`model=="demo"` → 换 DemoModel（仅本进程，不持久化）；
    /// 否则在指定（或当前激活）provider 条目上换模型名 → 热换 + 持久化 providers.json（下次启动沿用）。
    pub fn set_model(&self, model: &str, provider_id: Option<&str>) -> AppResult<serde_json::Value> {
        let slot = self
            .model_slot
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("未启用模型槽".into()))?;
        if model.eq_ignore_ascii_case("demo") {
            slot.swap(std::sync::Arc::new(crate::DemoModel));
            return Ok(serde_json::json!({ "service": "cmx-model", "current": "demo", "persisted": false,
                "note": "已切到离线演示模型（本次会话；重启按 providers.json）" }));
        }
        let dir = self
            .effective_model_config_dir()
            .ok_or_else(|| AppError::BadRequest("模型配置目录不可用".into()))?;
        let _g = self.providers_lock.lock().expect("providers lock");
        let mut pf = cmx_agent_model::ProviderFile::load(&dir);
        // 目标条目：显式 provider_id 优先；否则激活条目（多 provider 菜单在非激活 provider 下换模型时带 id）。
        let id = provider_id
            .map(|s| s.to_string())
            .or_else(|| pf.active.clone())
            .ok_or_else(|| AppError::BadRequest("未配置真实模型（无 providers.json/model.json/env），只能用 demo".into()))?;
        let p = pf
            .get_mut(&id)
            .ok_or_else(|| AppError::BadRequest("Provider 不存在（可能已被删除）".into()))?;
        p.config.model = model.to_string();
        let cfg = p.config.clone();
        let name = p.name.clone();
        let is_active = pf.active_id() == Some(id.as_str());
        let persisted = pf.save(&dir).is_ok();
        if is_active {
            slot.swap(std::sync::Arc::new(cmx_agent_model::OpenAiCompatModel::new(cfg.clone())));
        }
        let note = if persisted { "已切换并持久化，立即生效" } else { "已切换（本次会话；持久化失败）" };
        Ok(serde_json::json!({ "service": "cmx-model", "current": model, "provider": name,
            "persisted": persisted, "note": note }))
    }

    /// 运行时切换两旋钮（前门 set_policy）：沙箱能力 × 审批许可。
    /// 其余 Policy 项（max_steps / allowed_roots / subject）照抄当前值；仅本进程生效，不持久化。
    pub fn set_policy(
        &self,
        sandbox: cmx_agent_core::SandboxMode,
        approval: cmx_agent_core::ApprovalPolicy,
    ) -> AppResult<serde_json::Value> {
        let mut p = self.agent.policy();
        p.sandbox = sandbox;
        p.approval = approval;
        self.agent.set_policy(p);
        tracing::info!("cmx-agent 策略切换：sandbox={sandbox:?} · approval={approval:?}");
        Ok(serde_json::json!({ "sandbox": sandbox, "approval": approval }))
    }

    /// U15：列出插件（前门 list_plugins → master-detail 视图）。
    /// `installed` = plugins_dir 实时扫描（反映安装/卸载，无需重启）；无 dir 时用构建时快照。
    /// `market` = 配了 URL 则拉远程目录，否则空（前端可回退演示目录）。
    pub async fn list_plugins(&self) -> serde_json::Value {
        let installed = match &self.plugins_dir {
            Some(dir) => cmx_agent_plugin::scan_plugin_summaries(dir),
            None => self.plugins.clone(),
        };
        let (market, market_error) = match &self.plugin_market {
            Some(url) => match cmx_agent_plugin::fetch_market_catalog(url).await {
                Ok(items) => (items, None),
                Err(e) => (Vec::new(), Some(e)),
            },
            None => (Vec::new(), None),
        };
        serde_json::json!({
            "installed": installed,
            "market": market,
            "marketUrl": self.plugin_market,
            "marketError": market_error,
        })
    }

    /// U15：安装插件（前门 install_plugin）。用户在详情面板点「安装」直接触发（点击即人工授权）。
    /// 写入 plugins 目录 + **热注册**：http/command/wasm 建同步工具；**mcp 异步连上 server 代理其工具**——
    /// 均立即可用、无需重启。
    pub async fn install_plugin(&self, manifest: &serde_json::Value) -> AppResult<serde_json::Value> {
        let dir = self
            .plugins_dir
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("未配置 plugins 目录".into()))?
            .clone();
        let mut info = cmx_agent_plugin::install_manifest(&dir, manifest).map_err(AppError::BadRequest)?;
        let kind = manifest.get("kind").and_then(|v| v.as_str()).unwrap_or("");
        let name = manifest.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let hot = if kind == "mcp" {
            // mcp：异步连上 server，代理其工具热注册；记录工具名供卸载。
            match cmx_agent_plugin::connect_mcp_manifest(manifest).await {
                Ok(tools) => {
                    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
                    for t in tools {
                        self.agent.tools().register_dyn(t);
                    }
                    cmx_agent_plugin::record_mcp_tools(&dir, name, &names);
                    !names.is_empty()
                }
                Err(e) => {
                    tracing::warn!("mcp 插件 {name} 热连失败：{e}");
                    false
                }
            }
        } else {
            match cmx_agent_plugin::tool_for_installed(&dir, manifest) {
                Some(tool) => {
                    self.agent.tools().register_dyn(tool);
                    true
                }
                None => false,
            }
        };
        if let Some(o) = info.as_object_mut() {
            o.insert("hotLoaded".into(), serde_json::json!(hot));
            o.insert(
                "note".into(),
                serde_json::json!(if hot { "已热加载，立即可用（无需重启）" } else { "未知载体或连接失败：重启后再试" }),
            );
        }
        Ok(info)
    }

    /// U15：卸载插件（前门 uninstall_plugin）。删 `<plugins>/<name>/` + **热卸载**其工具（mcp 卸其全部代理工具）。
    pub fn uninstall_plugin(&self, name: &str) -> AppResult<serde_json::Value> {
        let dir = self
            .plugins_dir
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("未配置 plugins 目录".into()))?;
        // 删目录前先读 mcp 代理工具名。
        let mcp_tools = cmx_agent_plugin::read_mcp_tools(dir, name);
        let mut info = cmx_agent_plugin::uninstall_plugin(dir, name).map_err(AppError::BadRequest)?;
        let hot = if !mcp_tools.is_empty() {
            let mut any = false;
            for tn in &mcp_tools {
                any |= self.agent.tools().unregister(tn);
            }
            any
        } else {
            self.agent.tools().unregister(name) // 工具名 = 清单 name
        };
        if let Some(o) = info.as_object_mut() {
            o.insert("hotUnloaded".into(), serde_json::json!(hot));
            o.insert(
                "note".into(),
                serde_json::json!(if hot { "已热卸载（立即移除）" } else { "重启后生效" }),
            );
        }
        Ok(info)
    }

    /// U15：启用/禁用插件（前门 toggle_plugin）。禁用=写 `.disabled` 标记 + 热卸载工具（保留清单文件）；
    /// 启用=删标记 + 重建工具热注册。http/command/wasm 即时生效；mcp 需重启（异步连接）。
    pub fn toggle_plugin(&self, name: &str, enabled: bool) -> AppResult<serde_json::Value> {
        let dir = self
            .plugins_dir
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("未配置 plugins 目录".into()))?;
        cmx_agent_plugin::set_plugin_disabled(dir, name, !enabled).map_err(AppError::BadRequest)?;
        let hot = if enabled {
            // 启用：读回清单重建工具热注册（mcp/未知→None，需重启）。
            match cmx_agent_plugin::read_plugin_manifest(dir, name)
                .and_then(|mf| cmx_agent_plugin::tool_for_installed(dir, &mf))
            {
                Some(tool) => {
                    self.agent.tools().register_dyn(tool);
                    true
                }
                None => false,
            }
        } else {
            // 禁用：从注册表热卸载。
            self.agent.tools().unregister(name)
        };
        let note = if !hot {
            "mcp/未知载体：重启后生效"
        } else if enabled {
            "已启用，立即可用"
        } else {
            "已禁用，立即移除"
        };
        Ok(serde_json::json!({
            "service": "cmx-plugin", "name": name, "enabled": enabled, "hot": hot, "note": note,
        }))
    }

    /// 新建会话并落元数据。返回其 id。
    pub fn create_session(&self, id: impl Into<String>) -> AppResult<String> {
        let id = id.into();
        if id.is_empty() {
            return Err(AppError::BadRequest("empty session id".into()));
        }
        let now = chrono::Utc::now();
        let meta = SessionMeta {
            id: id.clone(),
            title: None,
            system: self.default_system.clone(),
            created_at: now,
            updated_at: now,
            event_count: 0,
        };
        self.store.put_meta(&meta)?;
        Ok(id)
    }

    /// 向某会话发一条用户消息，跑一个回合，**增量落库**新事件，返回结果。
    /// 若会话已有持久化日志，先加载恢复（回合号、历史上下文都续上）。
    pub async fn send(&self, session_id: &str, user_input: &str) -> AppResult<SendOutcome> {
        self.send_inner(session_id, user_input, None, None).await
    }

    /// 以指定主体跑一个回合（IM 绑定场景）：守卫/数据权限按 `subject`（绑定用户的 user_id+roles）
    /// 判定，与桌面登录身份（auth_identity）互不干扰、并发无竞态。其余语义同 [`Self::send`]。
    pub async fn send_as(
        &self,
        session_id: &str,
        user_input: &str,
        subject: &cmx_agent_core::Subject,
    ) -> AppResult<SendOutcome> {
        self.send_inner(session_id, user_input, None, Some(subject.clone())).await
    }

    /// 流式版：同 [`Self::send`]，但在回合开始前给会话日志挂上 `sink`——回合中每产生一个事件
    /// （模型消息 / 工具调用 / 工具结果 / 收尾）就实时通知 sink；同时模型**文字增量**（token 流）
    /// 也经同一 sink 实时回调（打字机效果）。返回值同 `send`（含全部新事件，供落库与兜底）。
    pub async fn send_streaming(
        &self,
        session_id: &str,
        user_input: &str,
        sink: std::sync::Arc<crate::stream::ChannelSink>,
    ) -> AppResult<SendOutcome> {
        self.send_inner(session_id, user_input, Some(sink), None).await
    }

    async fn send_inner(
        &self,
        session_id: &str,
        user_input: &str,
        sink: Option<std::sync::Arc<crate::stream::ChannelSink>>,
        subject: Option<cmx_agent_core::Subject>,
    ) -> AppResult<SendOutcome> {
        // 加载已有会话；不存在则以默认 system 新建一个内存会话（并补落元数据）。
        let mut session = match self.store.load(session_id) {
            Ok(s) => s,
            Err(AppError::NotFound(_)) => {
                self.create_session(session_id)?;
                let mut s = Session::new(session_id);
                if let Some(sys) = &self.default_system {
                    s = s.with_system(sys.clone());
                }
                s
            }
            Err(e) => return Err(e),
        };

        // 流式：加载完历史后挂 sink（历史用 push_restored 不触发 sink，故只流式本回合新事件）。
        // 同一个 sink 既是事件 EventSink（全量事件）又是 TurnObserver（文字增量）。
        let before = session.log.len();
        // U16：始终挂事件总线 sink——任何来源（本地 / IM 桥）的本回合事件都广播给 `/api/subscribe` 订阅者。
        session.log.add_sink(Arc::new(crate::bus::BusSink::new(
            session_id.to_string(),
            self.event_bus.sender(),
        )));
        let outcome = match (&sink, &subject) {
            (Some(s), Some(subj)) => {
                session.log.add_sink(s.clone());
                self.agent
                    .run_turn_observed_as(&mut session, user_input, Some(s.as_ref()), Some(subj))
                    .await
            }
            (Some(s), None) => {
                session.log.add_sink(s.clone());
                self.agent
                    .run_turn_observed(&mut session, user_input, Some(s.as_ref()))
                    .await
            }
            (None, Some(subj)) => self.agent.run_turn_as(&mut session, user_input, subj).await,
            (None, None) => self.agent.run_turn(&mut session, user_input).await,
        };
        // 出错一致性（飞书 ↔ 界面）：回合中途模型失败等会让 run_turn 提前返回 Err。若直接 `?` 抛出，
        // 已 append 的 TurnStarted/UserMessage 不会落库、不广播；错误文案只被 ImBridge 发给飞书，
        // 界面什么都看不到 → 两端不一致。故捕获错误：把错误文案作为 Note 事件追加（同步广播给界面
        // + 落库），补一条 TurnEnded(Error)，把错误文案塞进 final_text——ImBridge 据此回发飞书，
        // 界面经事件总线拿到同一份内容，两端一致。
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                let msg = format!("⚠ 处理出错：{e}");
                session.log.append(cmx_agent_core::event::EventKind::Note {
                    text: msg.clone(),
                });
                let turn = session.next_turn_no().saturating_sub(1);
                session.log.append(cmx_agent_core::event::EventKind::TurnEnded {
                    turn,
                    reason: cmx_agent_core::event::StopReason::Error,
                    steps: 0,
                });
                cmx_agent_core::TurnOutcome {
                    turn,
                    reason: cmx_agent_core::event::StopReason::Error,
                    steps: 0,
                    final_text: Some(msg),
                }
            }
        };

        // 只取本回合新增的事件，append 落库（不重写历史行）。
        let new_events: Vec<_> = session.log.events()[before..].to_vec();
        self.store.append_events(session_id, &new_events)?;

        // 刷新元数据（更新时间 + 事件总数；标题首条用户消息自动填充，保留原 created_at/title）。
        let now = chrono::Utc::now();
        let prev = self.store.list()?.into_iter().find(|m| m.id == session_id);
        let title = prev
            .as_ref()
            .and_then(|m| m.title.clone())
            .or_else(|| Some(make_title(user_input)));
        let meta = SessionMeta {
            id: session_id.to_string(),
            title,
            system: session.system().map(|s| s.to_string()),
            created_at: prev.map(|m| m.created_at).unwrap_or(now),
            updated_at: now,
            event_count: session.log.len(),
        };
        self.store.put_meta(&meta)?;

        Ok(SendOutcome {
            session_id: session_id.to_string(),
            turn: outcome.turn,
            reason: outcome.reason,
            steps: outcome.steps,
            final_text: outcome.final_text,
            new_events,
        })
    }

    /// 读取一个会话的全部事件（回放/展示）。
    pub fn get_events(
        &self,
        session_id: &str,
    ) -> AppResult<Vec<cmx_agent_core::event::SessionEvent>> {
        let session = self.store.load(session_id)?;
        Ok(session.log.events().to_vec())
    }

    /// 分页 / 尾加载事件（大会话切换提速）：只取最近一屏，需要时再「加载更早」。
    /// 见 [`crate::store::SessionStore::load_events_window`]。
    pub fn get_events_window(
        &self,
        session_id: &str,
        limit: Option<usize>,
        before: Option<usize>,
    ) -> AppResult<crate::store::EventWindow> {
        self.store.load_events_window(session_id, limit, before)
    }

    /// 列出所有会话元数据。
    pub fn list_sessions(&self) -> AppResult<Vec<SessionMeta>> {
        self.store.list()
    }

    /// 删除一个会话。
    pub fn delete_session(&self, session_id: &str) -> AppResult<()> {
        // 会话删除时一并撤销其「本对话全部允许」授权，避免同名会话复用旧授权。
        if let Some(a) = &self.approver {
            a.revoke_session(session_id);
        }
        self.store.delete(session_id)
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// U16：会话事件总线引用（壳的 `/api/subscribe` SSE 订阅它，实现 IM 事件实时推前端）。
    pub fn event_bus(&self) -> &Arc<crate::bus::SessionEventBus> {
        &self.event_bus
    }
}

/// B2：按 base_url 推断 provider 展示名（模型选择器用）。
fn provider_label(base_url: &str) -> String {
    let b = base_url.to_ascii_lowercase();
    if b.contains("mlamp") {
        "MLamp 网关".into()
    } else if b.contains("deepseek") {
        "DeepSeek".into()
    } else if b.contains("openai") {
        "OpenAI".into()
    } else if b.contains("dashscope") || b.contains("qwen") || b.contains("aliyun") {
        "通义千问".into()
    } else if b.is_empty() {
        "离线演示".into()
    } else {
        "OpenAI 兼容".into()
    }
}

/// 把连接器 [`ClientError`] 映射为面向用户的干净登录错误文案。
fn friendly_auth_error(e: cmx_agent_connectors::ClientError, base: &str) -> String {
    use cmx_agent_connectors::ClientError;
    match e {
        // 服务端业务错误（如 401 用户名/密码错误）——直接用其 msg（已是中文提示）。
        ClientError::Envelope { msg, .. } => msg,
        // 连不上认证服务（门户未启动等）。
        ClientError::Transport(_) => {
            format!("无法连接认证服务（{base}），请确认 cmx 门户服务已启动")
        }
        ClientError::Http(code) => format!("认证服务返回 HTTP {code}"),
        ClientError::Decode(m) => format!("认证响应解析失败：{m}"),
    }
}

/// 从首条用户消息生成会话标题：取首行、截断到 ~24 字符。
fn make_title(user_input: &str) -> String {
    let first_line = user_input.trim().lines().next().unwrap_or("").trim();
    let title: String = first_line.chars().take(24).collect();
    if title.is_empty() {
        "新会话".to_string()
    } else if first_line.chars().count() > 24 {
        format!("{title}…")
    } else {
        title
    }
}
