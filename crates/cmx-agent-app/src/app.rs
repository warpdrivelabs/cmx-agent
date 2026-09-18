//! 应用 façade [`AgentApp`]：桌面壳后端的单一入口。持有一个共享 `Agent` + 一个 `SessionStore`，
//! 对外提供"新建会话 / 发一条消息 / 列会话 / 读会话 / 删会话"等**用例级**操作，并在每个回合后
//! **增量落库**（只 append 新事件，不重写历史）。
//!
//! 这一层与前门无关（同核多壳）：CLI、Tauri invoke、Headless HTTP 都调它。见 [`crate::protocol`]。

use std::sync::Arc;
use std::sync::Mutex;

use cmx_agent_connectors::{AuthProvider, ConnectorCard, ConnectorRegistry, LoggedInUser, user_from_me};
use cmx_agent_core::event::{CompactionReason, EventKind, EventSink, SessionEvent, StopReason};
use cmx_agent_core::{Agent, Approver, ModelContext, ModelMessage, Session, TurnCancel};
use cmx_agent_model::is_context_overflow;

use crate::error::{AppError, AppResult};
use crate::store::{SessionMeta, SessionStore};

/// IM 遥控统一会话 id：飞书/QQ/微信三通道所有消息落这同一会话（cmx-agent-im 引用同值，
/// 前端 session.js 以 `^im-` 前缀过滤并作「助理」入口目标）。
pub const ASSISTANT_SESSION_ID: &str = "im-assistant";
/// 统一会话的固定展示标题（创建时写入，此后由 meta 保留机制维持）。
pub const ASSISTANT_SESSION_TITLE: &str = "IM 助理";

/// 计划模式回合追加的系统提示词章节（§7.3）。
const PLAN_MODE_PROMPT_SECTION: &str = "\n\n【计划模式】当前处于计划模式（只读档）：只做调研分析，\
不得修改任何文件或系统状态（写操作会被守卫拒绝，也不要用变通手段绕过）。\
先充分调研再收敛；最终产出结构化计划（目标 / 步骤 / 涉及文件 / 风险 / 验证方式），\
完成后调用 exit_plan 工具提交计划请求用户批准。不得宣称已修改任何文件；\
被拒绝的工具不要重试，改用白名单内的只读工具继续调研，或直接调用 exit_plan。";

/// 后台子任务回执回合（<task_result> 注入）追加的系统提示词章节：完成状态与完整原文已由
/// 界面系统卡承载，约束父模型转述措辞——不复述状态语、不整段引用原文（09-15「卡片+转述+
/// 引用」三连冗余收敛）。同计划模式章节模式：只改本回合模型上下文，不落日志。
const TASK_RESULT_PROMPT_SECTION: &str = "\n\n【后台子任务回执】刚收到的 <task_result> 回执已作为\
系统卡片向用户展示（含完成状态与完整输出）。请勿复述「子任务已完成」等状态语，也不要整段引用回执\
原文；直接给出基于结果实质内容的回答、整理或后续动作；若无实质内容可说，简短说明即可。";

// ── 上下文压缩常量（压缩方案 §4.2.3，2026-09-18 拍板）──
/// `context_window` 未配置时的默认窗口（用户拍板 1M；小真窗口模型靠溢出救援兜底）。
pub(crate) const DEFAULT_CONTEXT_WINDOW: u64 = 1_000_000;
/// 压缩预留（对齐 opencode buffer）。
pub(crate) const COMPACTION_BUFFER: u64 = 20_000;
/// 自动触发线：估算上下文 ≥ usable×0.85（宁晚勿早——误压代价是摘要丢细节）。
const AUTO_COMPACT_RATIO: f64 = 0.85;
/// 手动压缩空表（无可压缩历史，如刚压过再压）的收尾 Note 文本：前端按它渲染灰字「无需压缩」
/// 边界线（ZCode 同款 2026-09-18 反馈），IM 以 outcome.final_text 回复同文；改一侧须同步另一侧。
pub(crate) const COMPACT_NOOP_TEXT: &str = "上下文已是最新，无需压缩";
/// 未配置 max_output_tokens 时的输出预留缺省。
const DEFAULT_MAX_OUTPUT_TOKENS: u64 = 8_192;

/// 可用上下文 = 窗口 − max(max_output 配置值缺省 8_192, 20k 预留)。
pub(crate) fn usable_window(window: u64, max_output: Option<u64>) -> u64 {
    let reserve = max_output.unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS).max(COMPACTION_BUFFER);
    window.saturating_sub(reserve)
}

/// 斜杠分发路由结果（姊妹方案 §2.2）：Pass=继续正常发送（可能已改写文本），Handled=已消费输入。
pub enum SlashRoute {
    Pass(String),
    Handled(SendOutcome),
}

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
    /// 真技能目录 `<data_dir>/skills`（姊妹方案 §2.1；builder 从 data_dir 装配）。
    skills_dir: Option<std::path::PathBuf>,
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
    /// 登录会话落盘路径（`<data_dir>/auth.json`，由 builder 在启用登录门时自动装配）。
    /// None = 不持久化（CLI / 测试）。
    auth_session_path: Option<std::path::PathBuf>,
    /// 登录页注册入口显隐（`CMX_AGENT_REGISTER_ENABLED`，缺省开）。仅控制前端展示；
    /// 是否真能注册由门户部署的 `[auth] whitelist` 决定（服务端无开关）。
    register_enabled: bool,
    /// 工作空间注册表。None 兼容直接构造 AgentApp 的旧测试/CLI。
    workspaces: Option<Arc<crate::workspace::WorkspaceRegistry>>,
    /// 正在执行的会话回合；前端 CancelSession / 会话删除据此置位。
    active_turns: Mutex<std::collections::HashMap<String, TurnCancel>>,
    /// 同会话回合串行队列：后发消息排队，不与当前回合并发改日志。
    session_locks: tokio::sync::Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 已删除会话墓碑：删除时该会话若有在途回合，回合收尾的持久化步骤查墓碑放弃落库——
    /// 防「删除后回合结束又把会话文件写回来」的僵尸复活。新消息显式复活时清墓碑。
    pending_deleted: Mutex<std::collections::HashSet<String>>,
    /// 登出钩子：logout 时逐个调用（壳注册，如 Tauri 壳停 IM 桥——旧实现登出后 IM 桥
    /// 仍以旧身份拉消息跑回合）。
    logout_hooks: Mutex<Vec<Box<dyn Fn() + Send + Sync>>>,
    /// 阶段一：子智能体句柄（后台注入/取消/并发计数需要 app 层可达）。
    subagents: Option<Arc<cmx_agent_tools::SubagentHandle>>,
    /// 阶段一：子智能体注册表（agents.json 读写 + 热生效清单）。
    agents: Option<Arc<crate::agents::AgentRegistry>>,
    /// 阶段一：专属模型解析缝（None=父模型槽不需 app；Some(id)=需 providers 装配）。
    model_resolver: Option<Arc<crate::agents::AppModelResolver>>,
    /// 阶段二：计划模式活开关注册表：session_id → flag（回合开始插入、收尾摘除并对账回写 meta）。
    turn_plan_flags: Mutex<std::collections::HashMap<String, Arc<std::sync::atomic::AtomicBool>>>,
    /// 改造二（方案 20260914）：父会话在途回合期间到达的后台子任务回执暂存队列——
    /// 注入器见「父忙」即挂此处，由在途回合的内核收口点经 deferred 缝吸收（折尾）；
    /// 收口点未吸收完（race/中断/MaxSteps）由回合收尾持锁兜底 drain（降级独立回执回合）。
    pending_receipts: Mutex<std::collections::HashMap<String, Vec<String>>>,
}

/// 落盘的登录会话（`<data_dir>/auth.json`）：启动时经 /api/auth/me 校验回放，
/// access 失效再用 refresh 续签——双壳共用同一份数据根，一次登录双壳免登。
/// ⚠ 含明文令牌：与 model.json 同级保护（用户 profile 目录，不进仓库/不进日志）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuthSessionFile {
    pub user_id: String,
    pub username: String,
    #[serde(default)]
    pub nickname: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
    #[serde(default)]
    pub must_change_password: bool,
    pub access_token: String,
    #[serde(default)]
    pub refresh_token: String,
    #[serde(default)]
    pub access_expires_at: i64,
    #[serde(default)]
    pub refresh_expires_at: i64,
    /// 落盘时间（Unix 秒，诊断用）。
    #[serde(default)]
    pub saved_at: i64,
}

/// 读会话文件。损坏 / 缺失 → None。
pub fn read_auth_session(path: &std::path::Path) -> Option<AuthSessionFile> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 写会话文件（登录成功 / refresh 轮换后调用）。失败只打日志不阻断登录本身。
pub fn write_auth_session(path: &std::path::Path, session: &AuthSessionFile) {
    match serde_json::to_string_pretty(session) {
        Ok(json) => {
            if let Err(e) = std::fs::write(path, json) {
                eprintln!("[auth] 会话落盘失败（{}）：{e}", path.display());
            }
        }
        Err(e) => eprintln!("[auth] 会话序列化失败：{e}"),
    }
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
            skills_dir: None,
            plugin_market: None,
            data_auth_pep: None,
            auth_identity: None,
            model_slot: None,
            model_config_dir: None,
            user_config_base: None,
            providers_lock: Mutex::new(()),
            event_bus: Arc::new(crate::bus::SessionEventBus::new()),
            im_binding: None,
            auth_session_path: None,
            register_enabled: true,
            workspaces: None,
            active_turns: Mutex::new(std::collections::HashMap::new()),
            session_locks: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            pending_deleted: Mutex::new(std::collections::HashSet::new()),
            logout_hooks: Mutex::new(Vec::new()),
            subagents: None,
            agents: None,
            model_resolver: None,
            turn_plan_flags: Mutex::new(std::collections::HashMap::new()),
            pending_receipts: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// 注册登出钩子（壳层调用，如 Tauri 壳停 IM 桥长连接）。logout 时按注册序逐个调用。
    pub fn add_logout_hook(&mut self, f: Box<dyn Fn() + Send + Sync>) {
        self.logout_hooks.lock().expect("logout hooks lock").push(f);
    }

    /// 注入工作空间注册表；调用方负责加载失败时给出明确启动错误。
    pub fn with_workspace_registry(
        mut self,
        registry: crate::workspace::WorkspaceRegistry,
    ) -> Self {
        self.workspaces = Some(Arc::new(registry));
        self
    }

    /// 注入登录会话落盘路径（由 DesktopAppBuilder 在启用登录门时调用；None = 不持久化）。
    pub fn with_auth_session_path(mut self, path: std::path::PathBuf) -> Self {
        self.auth_session_path = Some(path);
        self
    }

    /// 注入登录页注册入口显隐开关（双壳构建期/启动期 env 烧定，缺省 true）。
    pub fn with_register_enabled(mut self, enabled: bool) -> Self {
        self.register_enabled = enabled;
        self
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

    /// 真技能目录（姊妹方案 §2.1）。未设 = 无技能（`/` 菜单技能组为空）。
    pub fn with_skills_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.skills_dir = Some(dir.into());
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

    /// 阶段一：注入子智能体句柄（由 DesktopAppBuilder 调用；取消/注入器挂 app 层用）。
    pub fn with_subagents(mut self, handle: Arc<cmx_agent_tools::SubagentHandle>) -> Self {
        self.subagents = Some(handle);
        self
    }

    /// 阶段一：注入子智能体注册表（ListAgents/SaveAgent/DeleteAgent 用）。
    pub fn with_agents(mut self, agents: Arc<crate::agents::AgentRegistry>) -> Self {
        self.agents = Some(agents);
        self
    }

    /// 阶段一：注入模型解析缝（`into_shared` 时 attach app 弱引用）。
    pub fn with_model_resolver(mut self, resolver: Arc<crate::agents::AppModelResolver>) -> Self {
        self.model_resolver = Some(resolver);
        self
    }

    /// 阶段三：把 `self` 变成共享 `Arc`，并装配「需要 app 弱引用」的回调缝：
    /// 后台子任务完成注入器（升级 Weak → `send` 唤醒父会话，父忙经 session_locks 排队）
    /// 与专属模型解析器（按 provider id 解析）。CLI/e2e 等直接 `Arc::new` 的装配不调本方法——
    /// 后台 task 据此 fail-closed 拒绝，专属模型解析报"需完整装配"。
    pub fn into_shared(self) -> Arc<Self> {
        let arc = Arc::new(self);
        if let (Some(handle), Some(resolver)) = (&arc.subagents, &arc.model_resolver) {
            resolver.attach(&arc);
            let weak = Arc::downgrade(&arc);
            handle.attach_injector(Arc::new(move |parent_id, text| {
                let weak = weak.clone();
                Box::pin(async move {
                    if let Some(app) = weak.upgrade() {
                        // 改造二（方案 20260914）分流：父会话在途回合 → 回执挂 pending 队列，
                        // 由在途回合的内核收口点吸收折进当前回复（图二形态）；父空闲 → 立即
                        // app.send 开独立回执回合（图三形态，原路径不变）。
                        let busy = app
                            .active_turns
                            .lock()
                            .expect("active turns lock")
                            .contains_key(&parent_id);
                        if busy {
                            app.pending_receipts
                                .lock()
                                .expect("pending receipts lock")
                                .entry(parent_id.to_string())
                                .or_default()
                                .push(text.to_string());
                            return;
                        }
                        // AgentApp::send 自动获得 session_locks 按会话排队：父会话忙则
                        // <task_result> 排在当前回合之后（不与在途回合并发写日志）。
                        if let Err(e) = app.send(&parent_id, &text).await {
                            eprintln!("[subagent] 后台结果注入会话 {parent_id} 失败：{e}");
                        }
                    }
                })
            }));
        }
        arc
    }

    /// 阶段一：解析一个命名 provider 为模型缝（子智能体专属模型用）。失败显式报错不静默降级。
    pub fn resolve_named_provider(&self, id: &str) -> Result<Arc<dyn cmx_agent_core::ModelSeam>, String> {
        let dir = self
            .effective_model_config_dir()
            .ok_or_else(|| "模型配置目录不可用".to_string())?;
        let _g = self.providers_lock.lock().expect("providers lock");
        let pf = self.provider_file_inherited(&dir);
        let p = pf
            .get(id)
            .ok_or_else(|| format!("provider '{id}' 不存在"))?;
        if p.config.base_url.is_empty() {
            return Err(format!("provider '{id}' 未配置 base_url"));
        }
        Ok(Arc::new(cmx_agent_model::OpenAiCompatModel::new(p.config.clone())))
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
    /// `note` 为用户附言（拒绝时「告诉模型接下来应该怎么做」），随决定回灌给模型。
    pub fn resolve_approval_decision(
        &self,
        call_id: &str,
        approved: bool,
        all: bool,
        session_id: &str,
        note: &str,
    ) -> bool {
        match &self.approver {
            Some(a) => {
                // 会话绑定强制（红蓝审查 P2-3）：空 session_hint 一律拒绝——与提问服务同规。
                // （审批者内部清理路径 revoke_all/cancel_session 走 InteractiveApprover::decide
                // 原语、不经本前门入口，不受此限。）
                if session_id.is_empty() {
                    return false;
                }
                let hit = a.decide(call_id, approved, session_id, note);
                // 只有确实命中一个待决审批才授予「全部允许」——空点/重复点击/跨会话误投
                // 不得静默关闭整会话的审批门。
                if all && approved && hit && !session_id.is_empty() {
                    a.allow_session(session_id);
                }
                hit
            }
            None => false,
        }
    }

    /// 某会话当前待决审批（刷新后在途恢复：前端重画审批卡）。
    pub fn list_pending_approvals(&self, session_id: &str) -> Vec<serde_json::Value> {
        match &self.approver {
            Some(a) => a
                .pending_in_session(session_id)
                .into_iter()
                .map(|i| serde_json::to_value(i).unwrap_or(serde_json::Value::Null))
                .collect(),
            None => Vec::new(),
        }
    }

    /// 某父会话当前活动子任务快照（方案 20260915 可视化 B4/F4 恢复层）：
    /// 页面刷新/重开后，前端据此重建「后台执行中」卡、续流子事件、并逐个补查子会话挂起的审批。
    /// 项在子任务收尾/级联取消时即摘除（登记与取消同源同生命周期）。
    pub fn list_active_subtasks(&self, parent_id: &str) -> Vec<serde_json::Value> {
        match &self.subagents {
            Some(h) => h
                .active_for_parent(parent_id)
                .into_iter()
                .map(|t| {
                    serde_json::json!({
                        "task_id": t.sub_id,
                        "description": t.description,
                        "subagent_type": t.subagent_type,
                        "background": t.background,
                        "started_at_ms": t.started_at_ms,
                    })
                })
                .collect(),
            None => Vec::new(),
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
        // 会话落盘（下次启动回放免登录）；失败静默——持久化是优化，不是登录门。
        self.persist_auth_session(&user);
        self.apply_session(user).await;
        Ok(public)
    }

    /// 自助注册（对接门户 /api/auth/register）。注册成功即登录：门户直接签发 token 对，
    /// 这里与登录完全同路径（persist_auth_session + apply_session）。返回前端可见用户信息。
    pub async fn register(
        &self,
        username: &str,
        password: &str,
        nickname: Option<&str>,
    ) -> AppResult<serde_json::Value> {
        let auth = self
            .auth
            .as_ref()
            .ok_or_else(|| AppError::Auth("未配置认证服务".into()))?;
        if username.trim().is_empty() || password.is_empty() {
            return Err(AppError::Auth("请输入用户名和密码".into()));
        }
        // 镜像门户 PasswordPolicy 本地快速失败（与 change_password 同规；门户仍为最终裁决）。
        check_password_policy(password).map_err(AppError::Auth)?;
        let user = auth
            .register(username.trim(), password, nickname)
            .await
            .map_err(|e| AppError::Auth(friendly_register_error(e, auth.base_url())))?;
        let public = user.public_json();
        self.persist_auth_session(&user);
        self.apply_session(user).await;
        Ok(public)
    }

    /// 登录页 UI 配置（免登录白名单命令）：注册入口显隐等纯展示开关。
    pub fn ui_config(&self) -> serde_json::Value {
        serde_json::json!({ "register_enabled": self.register_enabled })
    }

    /// 启动会话回放：读 `<data_dir>/auth.json` → `/api/auth/me` 校验 → 有效则恢复登录态；
    /// access 失效用 refresh_token 续签（轮换）后重试；确认失效 → 清落盘文件（回登录门）。
    /// ⚠ 网络/服务不可达 ≠ 会话失效：此时**保留**落盘文件，下次启动再试（不能因断网把人登出）。
    /// 返回是否恢复成功。
    pub async fn try_restore_session(&self) -> bool {
        let (Some(path), Some(auth)) = (&self.auth_session_path, &self.auth) else {
            return false;
        };
        let raw_snapshot = std::fs::read_to_string(path).ok();
        let Some(file) = read_auth_session(path) else { return false };
        eprintln!("[auth] 会话回放：检测到本地登录态（{}），校验中…", file.username);
        let mut access = file.access_token.clone();
        let mut refresh_token = file.refresh_token.clone();
        let mut access_exp = file.access_expires_at;
        let mut refresh_exp = file.refresh_expires_at;
        for attempt in 0..2 {
            let me = match auth.me(&access).await {
                Ok(me) => me,
                Err(e) => {
                    if is_auth_rejection(&e) && attempt == 0 && !refresh_token.is_empty() {
                        // access 确认失效 → refresh 续签（refresh 轮换，须回写落盘）后重验。
                        match auth.refresh(&refresh_token).await {
                            Ok(pair) if !pair.access_token.is_empty() => {
                                access = pair.access_token;
                                refresh_token = pair.refresh_token;
                                access_exp = pair.access_expires_at;
                                refresh_exp = pair.refresh_expires_at;
                                continue;
                            }
                            _ => break, // refresh 也被拒 / 响应异常：会话彻底失效
                        }
                    }
                    if !is_auth_rejection(&e) {
                        // 传输失败 / 响应解析异常：按网络问题处理，保留文件下次启动再试。
                        eprintln!("[auth] 会话校验暂不可达（{e}），保留本地会话");
                        return false;
                    }
                    break;
                }
            };
            let mut user = user_from_me(&me, &file.username, access);
            user.refresh_token = refresh_token;
            user.access_expires_at = access_exp;
            user.refresh_expires_at = refresh_exp;
            self.apply_session(user.clone()).await;
            self.persist_auth_session(&user); // refresh 轮换后回写；直接校验通过时为无害重写
            eprintln!("[auth] 会话回放成功：{}，跳过登录门", user.username);
            return true;
        }
        // 防双花误删：refresh 轮换后旧 token 一次性作废，双壳/双实例并发回放时另一方必然
        // 刷新失败走到这里。若文件内容已不等于启动时快照（= 并发方刚轮换写入新令牌），
        // 绝不能删——否则把对方的有效会话一起清掉，双壳全部被登出。
        match (std::fs::read_to_string(path), raw_snapshot) {
            (Ok(cur), Some(old)) if cur != old => {
                eprintln!("[auth] 本地会话已被其他实例更新，保留不删（防并发轮换双花误删）");
            }
            _ => {
                let _ = std::fs::remove_file(path);
            }
        }
        eprintln!("[auth] 会话已失效，清除本地会话（回登录门）");
        false
    }

    /// 把登录用户写进各共享槽（令牌槽 / PDP 主体 / current_user）。登录与回放共用。
    async fn apply_session(&self, user: LoggedInUser) {
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
    }

    /// 会话落盘（auth.json）。静默容错：落盘失败不阻断登录。
    fn persist_auth_session(&self, user: &LoggedInUser) {
        let Some(path) = &self.auth_session_path else { return };
        let file = AuthSessionFile {
            user_id: user.user_id.clone(),
            username: user.username.clone(),
            nickname: user.nickname.clone(),
            roles: user.roles.clone(),
            must_change_password: user.must_change_password,
            access_token: user.access_token.clone(),
            refresh_token: user.refresh_token.clone(),
            access_expires_at: user.access_expires_at,
            refresh_expires_at: user.refresh_expires_at,
            saved_at: now_secs(),
        };
        write_auth_session(path, &file);
    }

    /// 当前登录用户（前端可见信息，不含令牌）。未登录返回 None。
    pub fn current_user(&self) -> Option<serde_json::Value> {
        self.current_user
            .lock()
            .expect("current_user lock")
            .as_ref()
            .map(|u| u.public_json())
    }

    /// 修改密码（对接门户 /api/auth/change-password，Bearer 当前会话令牌）。
    /// 门户改密成功即吊销该用户全部 token——这里同步本地登出（清用户 + 令牌槽 + PDP 主体），
    /// 前端收到 ok 后引导重新登录。返回 `{"changed":true,"relogin":true}`。
    pub async fn change_password(&self, old_password: &str, new_password: &str) -> AppResult<serde_json::Value> {
        let auth = self
            .auth
            .as_ref()
            .ok_or_else(|| AppError::Auth("未配置认证服务".into()))?;
        let token = {
            let guard = self.current_user.lock().expect("current_user lock");
            guard
                .as_ref()
                .ok_or_else(|| AppError::Auth("尚未登录".into()))?
                .access_token
                .clone()
        };
        if old_password.is_empty() || new_password.is_empty() {
            return Err(AppError::Auth("请输入旧密码和新密码".into()));
        }
        if old_password == new_password {
            return Err(AppError::Auth("新密码不能与旧密码相同".into()));
        }
        check_password_policy(new_password).map_err(AppError::Auth)?;
        auth.change_password(&token, old_password, new_password)
            .await
            .map_err(|e| AppError::Auth(friendly_auth_error(e, auth.base_url())))?;
        // 改密成功：门户已全端吊销 token，本地立即登出（须重新登录）。
        self.logout();
        Ok(serde_json::json!({ "changed": true, "relogin": true }))
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

    /// 是否配置了门户认证（auth 客户端）。未配置 = 本地单机模式（CLI serve / 测试），
    /// 前门登录门不强制——有门户认证的桌面壳/Web 壳才 enforce。
    pub fn auth_configured(&self) -> bool {
        self.auth.is_some()
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
        // 会话文件同步删除 → 下次启动不回放（真登出，而非"重启还挂着旧会话"）。
        if let Some(path) = &self.auth_session_path {
            let _ = std::fs::remove_file(path);
        }
        // 审批态随登录身份一并失效：清「全部允许」授权 + 拒绝所有待决审批——
        // 防止换账号后沿用前任用户授予的免审批授权。
        if let Some(a) = &self.approver {
            a.revoke_all();
        }
        // 待决提问同理随登录身份失效：全量忽略（挂起的回合以 dismissed 收尾，不跨身份沿用）。
        self.agent.questions().revoke_all();
        // 壳层钩子（如 Tauri 壳停 IM 桥长连接/轮询）：登出后 IM 不再以旧身份拉消息跑回合。
        for h in self.logout_hooks.lock().expect("logout hooks lock").iter() {
            h();
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
        Some((dir.clone(), self.provider_file_inherited(&dir)))
    }

    /// 读**有效目录**的 providers.json；目录为 per-user（≠全局 `model_config_dir`）时，
    /// 从全局目录同网关条目继承 key（只补空缺）——用户在共享域填过 key、登录后 per-user
    /// 条目 key 为空，不继承则面板热切模型会拿空凭据调网关（网关报「未提供令牌」）。
    fn provider_file_inherited(
        &self,
        dir: &std::path::Path,
    ) -> cmx_agent_model::ProviderFile {
        let inherit = self
            .model_config_dir
            .as_ref()
            .filter(|base| base.as_path() != dir)
            .map(|base| base.as_path());
        cmx_agent_model::ProviderFile::load_with_inherit(dir, inherit)
    }

    /// 取一个命名 provider 的面板回填 JSON（掩码 key + 模型清单）。None = id 不存在。
    /// `reveal=true`（方案：点眼睛展示完整字符串，用户拍板 2026-09-17）时 api_key 给明文——
    /// 本机应用，providers.json 本就明文落盘；password 框掩码展示，点眼睛看全串。
    fn provider_config_json(pf: &cmx_agent_model::ProviderFile, p: &cmx_agent_model::NamedProvider, reveal: bool) -> serde_json::Value {
        let models: Vec<serde_json::Value> = p
            .models
            .iter()
            .map(|m| serde_json::json!({ "id": m.id, "enabled": m.enabled, "reasoning": m.reasoning,
                "context_window": m.context_window, "max_output_tokens": m.max_output_tokens,
                "input_types": m.input_types, "capabilities": m.capabilities,
                "reasoning_levels": m.reasoning_levels, "reasoning_params": m.reasoning_params }))
            .collect();
        // 候选（聊天选择器口径）= 仅启用项；当前模型保证在列。
        let mut cands: Vec<serde_json::Value> = p
            .models
            .iter()
            .filter(|m| m.enabled)
            .map(|m| serde_json::json!({ "model": m.id, "label": m.id, "reasoning": m.reasoning }))
            .collect();
        if !p.config.model.is_empty() && !cands.iter().any(|c| c["model"] == p.config.model.as_str()) {
            cands.insert(0, serde_json::json!({ "model": p.config.model, "label": p.config.model }));
        }
        let mut v = serde_json::json!({
            "configured": !p.config.api_key.is_empty() || !p.config.base_url.is_empty(),
            "id": p.id,
            "name": p.name,
            "builtin": p.builtin,
            "active": pf.active_id() == Some(p.id.as_str()),
            "kind": p.kind,
            "preset": p.preset,
            "api_key_url": p.api_key_url,
            "base_url": p.config.base_url,
            "api_key_masked": p.config.masked_api_key(),
            "model": p.config.model,
            "temperature": p.config.temperature,
            "timeout_ms": p.config.timeout_ms,
            "models": models,
            "candidates": cands,
        });
        if reveal {
            v["api_key"] = serde_json::json!(p.config.api_key);
        }
        v
    }

    /// B2：读取完整模型配置，供前端配置面板填充表单。
    /// `id` 为空取当前激活 provider（旧行为兼容）；有 id 取指定条目（多 provider 面板）。
    /// `reveal=true` 时 api_key 给**明文**（用户拍板「点眼睛展示完整字符串」：本机应用，
    /// providers.json 本就明文落盘、接口有登录门；password 框掩码展示、点眼睛看全串）。
    pub fn get_model_config(&self, id: Option<&str>, reveal: bool) -> serde_json::Value {
        let _g = self.providers_lock.lock().expect("providers lock");
        if let Some((_, pf)) = self.load_providers() {
            let target = id
                .and_then(|i| pf.get(i))
                .or_else(|| pf.active());
            if let Some(p) = target {
                return Self::provider_config_json(&pf, p, reveal);
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
    /// - `base_url`：必填；`kind`：P1 仅 "openai"；`preset`：来源模板 id；`api_key_url`：http(s) 外链
    /// - `models`：候选清单 `[{id, enabled, reasoning}]`（方案 20260917 §5.1），至少一条且
    ///   至少一条启用；缺省时由 `model` 播种单条（旧前端兼容）
    /// - `model`：**可选**——「当前使用中模型」语义，配置页不提供选择；沿用旧值，不在启用
    ///   清单（被删/被停用/新建）时自动回落第一个启用项
    /// - `temperature`：浮点（可选，缺省 0.2）；`timeout_ms`：整数（可选，缺省 60000，范围 5000–300000）
    /// - `api_key_action`："keep"（沿用该条目已存 key）| "set"（使用 `api_key_value`）
    ///
    /// 新建不自动激活；更新激活条目时热换模型槽立即生效。内置条目可编辑、不可由此删除。
    pub fn set_model_config(&self, payload: serde_json::Value) -> AppResult<serde_json::Value> {
        let get_str = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
        let base_url = get_str("base_url")
            .filter(|s| !s.is_empty())
            .ok_or_else(|| AppError::BadRequest("base_url 不能为空".into()))?;
        let kind = get_str("kind").unwrap_or_else(|| "openai".into());
        if kind != "openai" {
            return Err(AppError::BadRequest(format!(
                "暂不支持协议类型「{kind}」（当前仅 OpenAI 兼容端点）"
            )));
        }
        let preset = get_str("preset").unwrap_or_default();
        let api_key_url = get_str("api_key_url").unwrap_or_default();
        if !api_key_url.is_empty()
            && !api_key_url.starts_with("http://")
            && !api_key_url.starts_with("https://")
        {
            return Err(AppError::BadRequest("「获取 API Key」链接须以 http(s):// 开头".into()));
        }
        // 模型清单（方案 §5.3 清单管理）：id 非空去重；enabled 缺省 true。
        let mut models: Vec<cmx_agent_model::ModelEntry> = Vec::new();
        if let Some(arr) = payload.get("models").and_then(|v| v.as_array()) {
            for (i, m) in arr.iter().enumerate() {
                let id = m
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().to_string())
                    .unwrap_or_default();
                if id.is_empty() {
                    return Err(AppError::BadRequest(format!("第 {} 个模型 ID 不能为空", i + 1)));
                }
                if models.iter().any(|x| x.id == id) {
                    return Err(AppError::BadRequest(format!("模型「{id}」在清单中重复")));
                }
                // ZCode 式模型编辑弹窗的扩展字段（全部可选；值域白名单校验）。
                let parse_list = |key: &str, allowed: &[&str]| -> Result<Vec<String>, AppError> {
                    let mut out = Vec::new();
                    if let Some(arr) = m.get(key).and_then(|v| v.as_array()) {
                        for it in arr {
                            let s = it.as_str().unwrap_or_default().trim().to_string();
                            if s.is_empty() { continue; }
                            if !allowed.contains(&s.as_str()) {
                                return Err(AppError::BadRequest(format!("模型「{id}」的 {key} 含非法值「{s}」")));
                            }
                            if !out.contains(&s) { out.push(s); }
                        }
                    }
                    Ok(out)
                };
                let input_types = parse_list("input_types", cmx_agent_model::MODEL_INPUT_TYPES)?;
                let capabilities = parse_list("capabilities", cmx_agent_model::MODEL_CAPABILITIES)?;
                let mut reasoning_levels: Vec<String> = Vec::new();
                if let Some(arr) = m.get("reasoning_levels").and_then(|v| v.as_array()) {
                    for it in arr {
                        let s = it.as_str().unwrap_or_default().trim().to_string();
                        if !s.is_empty() && !reasoning_levels.contains(&s) { reasoning_levels.push(s); }
                    }
                }
                // 「从低到高」归一：预置档（low<high|max 常见序）按白名单顺序排，自定义档保持追加在后。
                reasoning_levels.sort_by_key(|l| {
                    cmx_agent_model::MODEL_REASONING_ORDER
                        .iter()
                        .position(|p| p == l)
                        .unwrap_or(cmx_agent_model::MODEL_REASONING_ORDER.len())
                });
                let reasoning_params = m.get("reasoning_params").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).unwrap_or_default();
                if !reasoning_params.is_empty()
                    && serde_json::from_str::<serde_json::Value>(&reasoning_params).is_err()
                {
                    return Err(AppError::BadRequest(format!("模型「{id}」的推理参数映射不是合法 JSON")));
                }
                models.push(cmx_agent_model::ModelEntry {
                    id,
                    enabled: m.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
                    // 推理等级非空 ⇔ 思考标记（reasoning 由弹窗推导，旧布尔字段保留兼容）。
                    reasoning: !reasoning_levels.is_empty(),
                    context_window: m.get("context_window").and_then(|v| v.as_u64()),
                    max_output_tokens: m.get("max_output_tokens").and_then(|v| v.as_u64()),
                    input_types,
                    capabilities,
                    reasoning_levels,
                    reasoning_params,
                });
            }
        }
        let model_in = get_str("model").unwrap_or_default();
        if models.is_empty() && !model_in.is_empty() {
            // 旧前端兼容：无 models 清单时由 model 字段播种单条启用项。
            models.push(cmx_agent_model::ModelEntry { id: model_in, enabled: true, ..Default::default() });
        }
        if models.is_empty() {
            return Err(AppError::BadRequest("至少添加一个模型".into()));
        }
        let action = get_str("api_key_action").unwrap_or_else(|| "keep".into());
        let target_id = get_str("id").filter(|s| !s.is_empty());
        let name_in = get_str("name").filter(|s| !s.is_empty());

        let dir = self
            .effective_model_config_dir()
            .ok_or_else(|| AppError::BadRequest("模型配置目录不可用".into()))?;
        let _g = self.providers_lock.lock().expect("providers lock");
        let mut pf = self.provider_file_inherited(&dir);

        // 更新已有条目：沿用旧 name/key；新建：name 必填 + 重名校验、key 可空（keyless 端点）。
        // 温度/超时：主表单已去掉「高级」区（ZCode 化），更新时 payload 缺省即**沿用旧值**，
        // 仅新建用默认——避免 UI 不传字段悄悄重置用户已有配置。
        let temperature_in = payload.get("temperature").and_then(|v| v.as_f64()).map(|f| f as f32);
        let timeout_in = payload.get("timeout_ms").and_then(|v| v.as_u64());
        let (name, api_key, builtin, is_active, old_model, old_temp, old_timeout) = match &target_id {
            Some(id) => {
                let p = pf
                    .get(id)
                    .ok_or_else(|| AppError::BadRequest("Provider 不存在（可能已被删除）".into()))?;
                (
                    name_in.unwrap_or_else(|| p.name.clone()),
                    if action == "set" { get_str("api_key_value").unwrap_or_default() } else { p.config.api_key.clone() },
                    p.builtin,
                    pf.active_id() == Some(id.as_str()),
                    p.config.model.clone(),
                    p.config.temperature,
                    p.config.timeout_ms,
                )
            }
            None => {
                let name = name_in
                    .ok_or_else(|| AppError::BadRequest("名称不能为空".into()))?;
                if pf.name_taken(&name, None) {
                    return Err(AppError::BadRequest(format!("名称「{name}」已存在，换一个")));
                }
                (name, get_str("api_key_value").unwrap_or_default(), false, false, String::new(), 0.2_f32, 60_000_u64)
            }
        };
        let temperature = temperature_in.unwrap_or(old_temp);
        let timeout_ms = timeout_in.unwrap_or(old_timeout);
        if !(5000..=300_000).contains(&timeout_ms) {
            return Err(AppError::BadRequest("超时时间需在 5000 – 300000 毫秒之间".into()));
        }

        // 「当前使用中模型」自动回落（方案 §5.3）：仍在启用清单 → 沿用；被删/被停用/新建
        // → 回落第一个启用项；全部停用 → 拒存（选择器会变空，等于废配置）。
        let model = if !old_model.is_empty() && models.iter().any(|m| m.enabled && m.id == old_model) {
            old_model
        } else if let Some(first) = models.iter().find(|m| m.enabled) {
            first.id.clone()
        } else {
            return Err(AppError::BadRequest("至少保留一个「启用」的模型（当前清单全部停用）".into()));
        };

        let id = target_id.unwrap_or_else(cmx_agent_model::new_id);
        let cfg = cmx_agent_model::ModelProviderConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model: model.clone(),
            temperature,
            timeout_ms,
        };
        pf.upsert(cmx_agent_model::NamedProvider {
            id: id.clone(),
            name: name.clone(),
            builtin,
            kind,
            preset,
            models,
            api_key_url,
            config: cfg.clone(),
        });
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
    /// `active` 标出当前激活条目；`candidates` = 启用中的候选模型（聊天选择器）；
    /// `models` = 完整清单（配置页清单管理，含停用项）。
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
                let models: Vec<serde_json::Value> = p
                    .models
                    .iter()
                    .map(|m| serde_json::json!({ "id": m.id, "enabled": m.enabled, "reasoning": m.reasoning }))
                    .collect();
                let mut cands: Vec<serde_json::Value> = p
                    .models
                    .iter()
                    .filter(|m| m.enabled)
                    .map(|m| serde_json::json!({ "model": m.id, "label": m.id, "reasoning": m.reasoning,
                        "input_types": m.input_types }))
                    .collect();
                // 当前模型保证在候选里（清单未含时——如停用了当前模型被回落前的旧快照）。
                if !p.config.model.is_empty()
                    && !cands.iter().any(|c| c["model"] == p.config.model.as_str())
                {
                    cands.insert(0, serde_json::json!({ "model": p.config.model, "label": p.config.model }));
                }
                serde_json::json!({
                    "id": p.id,
                    "name": p.name,
                    "builtin": p.builtin,
                    "active": pf.active_id() == Some(p.id.as_str()),
                    "kind": p.kind,
                    "preset": p.preset,
                    "api_key_url": p.api_key_url,
                    "base_url": p.config.base_url,
                    "api_key_masked": p.config.masked_api_key(),
                    "configured_key": !p.config.api_key.is_empty(),
                    "model": p.config.model,
                    "models": models,
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

    /// P1（方案 20260917 §5.2）：内置供应商模板目录——「＋ 新增 Provider」目录卡数据源。
    /// 真源在后端 `PROVIDER_PRESETS` 常量（前端两份硬编码已下线，随应用版本分发）。
    pub fn list_provider_presets(&self) -> serde_json::Value {
        serde_json::json!({ "service": "cmx-model", "presets": cmx_agent_model::provider_presets_json() })
    }

    /// P1（方案 §5.4）：测试连接——真实发 `max_tokens=1` 探测，同时验证 key、URL、模型名。
    /// 入参 = 表单暂存（明文 key **只进内存不落盘**）或 `id` + keep（沿用已存 key）。
    /// 错误按七类分类返回（不打 AppError，前端直接展示友好话术）。
    pub async fn test_model_config(&self, payload: serde_json::Value) -> AppResult<serde_json::Value> {
        let get_str = |k: &str| payload.get(k).and_then(|v| v.as_str()).map(|s| s.trim().to_string());
        let target_id = get_str("id").filter(|s| !s.is_empty());
        let (base_url, api_key, model, timeout_ms) = match &target_id {
            Some(id) => {
                let p = {
                    let _g = self.providers_lock.lock().expect("providers lock");
                    self.load_providers().and_then(|(_, pf)| pf.get(id).cloned())
                }
                .ok_or_else(|| AppError::BadRequest("Provider 不存在（可能已被删除）".into()))?;
                let key = if get_str("api_key_action").as_deref() == Some("set") {
                    get_str("api_key_value").unwrap_or_default()
                } else {
                    p.config.api_key
                };
                (
                    get_str("base_url").filter(|s| !s.is_empty()).unwrap_or(p.config.base_url),
                    key,
                    get_str("model").filter(|s| !s.is_empty()).unwrap_or(p.config.model),
                    payload.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(p.config.timeout_ms),
                )
            }
            None => (
                get_str("base_url")
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AppError::BadRequest("base_url 不能为空".into()))?,
                get_str("api_key_value").unwrap_or_default(),
                get_str("model")
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| AppError::BadRequest("model 不能为空".into()))?,
                payload.get("timeout_ms").and_then(|v| v.as_u64()).unwrap_or(60_000),
            ),
        };
        let cfg = cmx_agent_model::ModelProviderConfig {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            model,
            temperature: 0.2,
            // 探测超时上限 30s：表单可填 300s，但「测试」不该让用户等 5 分钟。
            timeout_ms: timeout_ms.clamp(3_000, 30_000),
        };
        match cmx_agent_model::OpenAiCompatModel::new(cfg).ping().await {
            Ok(ms) => Ok(serde_json::json!({ "service": "cmx-model", "ok": true, "latency_ms": ms })),
            Err(e) => Ok(serde_json::json!({
                "service": "cmx-model", "ok": false, "err_kind": e.kind, "message": e.message,
            })),
        }
    }

    /// 多 provider：删除一个自定义条目。内置条目不可删；删除激活条目时激活回落到剩余第一条
    /// （无剩余则回 demo），并热换模型槽。
    pub fn delete_provider(&self, id: &str) -> AppResult<serde_json::Value> {
        let dir = self
            .effective_model_config_dir()
            .ok_or_else(|| AppError::BadRequest("模型配置目录不可用".into()))?;
        let _g = self.providers_lock.lock().expect("providers lock");
        let mut pf = self.provider_file_inherited(&dir);
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
        // 子智能体专属模型引用联动清理（2026-09-15 设置页审查 P2）：被删 provider 若被
        // agents.json 引用，置回继承默认——否则该类型每次派发都报「provider 不存在」。
        if let Some(reg) = &self.agents {
            reg.clear_model_ref(id)?;
        }
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
        let mut pf = self.provider_file_inherited(&dir);
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
    /// `current` = 当前生效模型（真实 provider 的 model，或 demo）；
    /// `candidates` = 激活 provider 的启用模型清单（providers.json models，方案 20260917
    /// 关键词硬编码已下线）+ demo；展示名优先激活条目的用户命名。
    pub fn list_models(&self) -> serde_json::Value {
        let cfg = cmx_agent_model::resolve_active(self.effective_model_config_dir().as_deref());
        // 展示名：providers.json 激活条目的用户命名优先（env / model.json 路径回落 provider_label）。
        let named = self.load_providers().and_then(|(_, pf)| {
            pf.active().map(|p| {
                if p.name.is_empty() { provider_label(&p.config.base_url) } else { p.name.clone() }
            })
        });
        let enabled = self.load_providers().and_then(|(_, pf)| pf.active().map(|p| p.enabled_models()));
        let (current, provider, base_url) = match &cfg {
            Some(c) => (c.model.clone(), named.unwrap_or_else(|| provider_label(&c.base_url)), c.base_url.clone()),
            None => ("demo".to_string(), "离线演示".to_string(), String::new()),
        };
        let mut candidates: Vec<serde_json::Value> = Vec::new();
        if cfg.is_some() {
            for m in enabled.unwrap_or_default() {
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
        let mut pf = self.provider_file_inherited(&dir);
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
        // 选模型即隐式激活该 provider（2026-09-17 修复：未配置态下拉直选模型时目标还不是
        // 激活条目，旧逻辑 is_active=false 跳过热换 → 配置存了但回合仍走 DemoModel）。
        if pf.active_id() != Some(id.as_str()) {
            pf.active = Some(id.to_string());
        }
        let persisted = pf.save(&dir).is_ok();
        if cfg.base_url.is_empty() {
            slot.swap(std::sync::Arc::new(crate::DemoModel));
        } else {
            slot.swap(std::sync::Arc::new(cmx_agent_model::OpenAiCompatModel::new(cfg.clone())));
        }
        let note = if persisted { "已切换并持久化，立即生效" } else { "已切换（本次会话；持久化失败）" };
        Ok(serde_json::json!({ "service": "cmx-model", "current": model, "provider": name,
            "active": id, "persisted": persisted, "note": note }))
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
            // 同名遮蔽拒绝（N3）：任一代理工具与既有工具重名 → 整体安装失败。
            match cmx_agent_plugin::connect_mcp_manifest(manifest).await {
                Ok(tools) => {
                    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
                    let mut conflict: Option<String> = None;
                    for t in &tools {
                        if let Err(e) = self.agent.tools().register_dyn(t.clone()) {
                            conflict = Some(e);
                            break;
                        }
                    }
                    if let Some(e) = conflict {
                        return Err(AppError::BadRequest(e));
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
                Some(tool) => match self.agent.tools().register_dyn(tool) {
                    Ok(()) => true,
                    // 同名遮蔽拒绝（N3）：重装/改名冲突 → 上抛为安装失败（先卸旧再装）。
                    Err(e) => return Err(AppError::BadRequest(e)),
                },
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
                Some(tool) => match self.agent.tools().register_dyn(tool) {
                    Ok(()) => true,
                    Err(e) => return Err(AppError::BadRequest(e)),
                },
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
    ///
    /// get-or-create 语义：id 已存在时**原样返回、不覆盖 meta**。IM 桥每条消息都会调本方法，
    /// 若无条件覆盖，会把会话的空间/标题按"最后一条消息那一刻"反复重写。
    pub fn create_session(&self, id: impl Into<String>) -> AppResult<String> {
        let id = id.into();
        if id.is_empty() {
            return Err(AppError::BadRequest("empty session id".into()));
        }
        // 保留前缀（红蓝审查 P3-2）：subtask- 是子智能体会话的内部命名空间，会话列表按前缀
        // 隐匿它们——用户显式创建同前缀 id 会得到「列表里看不见」的隐身会话。
        if id.starts_with("subtask-") {
            return Err(AppError::BadRequest(
                "会话 id 不能使用保留前缀 subtask-".into(),
            ));
        }
        if self.store.list()?.iter().any(|m| m.id == id) {
            return Ok(id);
        }
        let now = chrono::Utc::now();
        let meta = SessionMeta {
            id: id.clone(),
            title: None,
            system: self.default_system.clone(),
            created_at: now,
            updated_at: now,
            event_count: 0,
            workspace_id: self.current_workspace_id(),
            plan_mode: false,
        };
        self.store.put_meta(&meta)?;
        Ok(id)
    }

    /// get-or-create，且**仅在新建时**采用给定工作空间与标题；已存在则完全不动 meta。
    /// IM 桥用它建统一助理会话：空间恒 default、标题恒「IM 助理」，桌面怎么切空间都影响不到。
    pub fn ensure_session(
        &self,
        id: impl Into<String>,
        workspace_id: Option<String>,
        title: Option<String>,
    ) -> AppResult<String> {
        let id = id.into();
        if id.is_empty() {
            return Err(AppError::BadRequest("empty session id".into()));
        }
        if self.store.list()?.iter().any(|m| m.id == id) {
            return Ok(id);
        }
        let now = chrono::Utc::now();
        let meta = SessionMeta {
            id: id.clone(),
            title,
            system: self.default_system.clone(),
            created_at: now,
            updated_at: now,
            event_count: 0,
            workspace_id,
            plan_mode: false,
        };
        self.store.put_meta(&meta)?;
        Ok(id)
    }

    /// 桌面「助理」入口：确保 IM 统一会话存在（default 空间、固定标题）后返回其 id。
    /// 走本方法而非直接 send，保证从桌面首次进入（会话尚不存在）时空间/标题就正确。
    pub fn open_assistant_session(&self) -> AppResult<String> {
        self.ensure_session(
            ASSISTANT_SESSION_ID,
            Some("default".to_string()),
            Some(ASSISTANT_SESSION_TITLE.to_string()),
        )
    }

    /// 向某会话发一条用户消息，跑一个回合，**增量落库**新事件，返回结果。
    /// 若会话已有持久化日志，先加载恢复（回合号、历史上下文都续上）。
    pub async fn send(&self, session_id: &str, user_input: &str) -> AppResult<SendOutcome> {
        self.send_inner(session_id, user_input, None, None, None).await
    }

    /// 以指定主体跑一个回合（IM 绑定场景）：守卫/数据权限按 `subject`（绑定用户的 user_id+roles）
    /// 判定，与桌面登录身份（auth_identity）互不干扰、并发无竞态。其余语义同 [`Self::send`]。
    pub async fn send_as(
        &self,
        session_id: &str,
        user_input: &str,
        subject: &cmx_agent_core::Subject,
    ) -> AppResult<SendOutcome> {
        self.send_inner(session_id, user_input, None, Some(subject.clone()), None).await
    }

    /// [`Self::send`] 的回合级权限档覆盖版：`Some(覆盖档)` 时本回合（含子智能体）的
    /// 沙箱/审批以覆盖为准——只 scope 在本回合任务树上，**不写全局档**，桌面并发回合
    /// 不受影响。IM 无人值守「默认全权」（[`cmx_agent_core::TurnPolicyOverride::FULL_ACCESS`]）由此接入。
    pub async fn send_with_policy(
        &self,
        session_id: &str,
        user_input: &str,
        policy_override: Option<cmx_agent_core::TurnPolicyOverride>,
    ) -> AppResult<SendOutcome> {
        self.send_inner(session_id, user_input, None, None, policy_override).await
    }

    /// [`Self::send_as`] 的回合级权限档覆盖版：以指定主体身份 + 覆盖档跑回合。语义见两者。
    pub async fn send_as_with_policy(
        &self,
        session_id: &str,
        user_input: &str,
        subject: &cmx_agent_core::Subject,
        policy_override: Option<cmx_agent_core::TurnPolicyOverride>,
    ) -> AppResult<SendOutcome> {
        self.send_inner(session_id, user_input, None, Some(subject.clone()), policy_override).await
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
        self.send_inner(session_id, user_input, Some(sink), None, None).await
    }

    async fn send_inner(
        &self,
        session_id: &str,
        user_input: &str,
        sink: Option<std::sync::Arc<crate::stream::ChannelSink>>,
        subject: Option<cmx_agent_core::Subject>,
        policy_override: Option<cmx_agent_core::TurnPolicyOverride>,
    ) -> AppResult<SendOutcome> {
        // 同会话队列：后到请求等待当前回合完成，避免两条消息并发写入同一个 JSONL。
        // 显式发新消息 = 该会话若曾被删除（墓碑未消费）即视为复活意图，摘除墓碑。
        self.pending_deleted
            .lock()
            .expect("pending deleted lock")
            .remove(session_id);
        let session_lock = {
            let mut locks = self.session_locks.lock().await;
            locks
                .entry(session_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _turn_permit = session_lock.lock().await;

        let cancel = TurnCancel::new();
        self.active_turns
            .lock()
            .expect("active turns lock")
            .insert(session_id.to_string(), cancel.clone());

        // 无论成功/失败都清理活动回合标记；事件持久化在下方统一完成。
        let result = self
            .send_inner_locked(
                session_id,
                user_input,
                sink,
                subject,
                &cancel,
                policy_override,
            )
            .await;
        self.active_turns
            .lock()
            .expect("active turns lock")
            .remove(session_id);
        result
    }

    /// 已持有会话队列锁后的实际回合执行（驱动器）：执行首回合，随后把在途期间到达、
    /// 收口点未吸收完的后台子任务回执**持锁兜底 drain**——逐条以独立回执回合续跑，
    /// race 窗口 / 中断 / MaxSteps 漏吸收的回执也不丢（方案 20260914 改造二）。
    async fn send_inner_locked(
        &self,
        session_id: &str,
        user_input: &str,
        sink: Option<std::sync::Arc<crate::stream::ChannelSink>>,
        subject: Option<cmx_agent_core::Subject>,
        cancel: &TurnCancel,
        policy_override: Option<cmx_agent_core::TurnPolicyOverride>,
    ) -> AppResult<SendOutcome> {
        // 加载已有会话；不存在则以默认 system 新建一个内存会话（并补落元数据）。
        let mut session = match self.store.load(session_id) {
            Ok(s) => s,
            Err(AppError::NotFound(_)) => {
                self.create_session(session_id)?;
                Session::new(session_id)
            }
            Err(e) => return Err(e),
        };

        // U16 总线 sink 上提到本函数（压缩方案改造）：边界压缩/溢出救援/斜杠命令追加的事件
        // 也要实时广播；下方 execute_turn_locked 一律传 attach_bus=false——同一日志实例
        // 重复挂载同一广播器会让每条事件双发（原实现只挂首回合，故 sink 必须只挂这一处）。
        session.log.add_sink(Arc::new(crate::bus::BusSink::new(
            session_id.to_string(),
            self.event_bus.sender(),
        )));

        // 斜杠分发（姊妹方案 §2.2）：命令 > 技能 > 子智能体 > 原样。持锁后、落任何事件前——
        // /compact 在此消费掉输入（不落 UserMessage，不进 run_turn）；技能/子智能体改写消息文本。
        let user_input = match self.dispatch_slash(session_id, &mut session, user_input).await? {
            crate::app::SlashRoute::Handled(outcome) => {
                // 斜杠命令的收尾事件（压缩成功/无需压缩/失败审计）必须补推给流式 sink：
                // ChannelSink 要到 execute_turn_locked 内部才挂载，斜杠路径到不了那里，事件
                // 只进总线——而前端本地发送期间丢弃总线帧（防双渲染），结果收尾永远不实时
                // 上屏，只能走前端兜底误报「压缩中断」（2026-09-18 实测根因）。迟到总线帧由
                // 前端 _seqs 去重兜底。
                if let Some(sink) = &sink {
                    for ev in &outcome.new_events {
                        sink.on_event(ev);
                    }
                }
                return Ok(outcome);
            }
            crate::app::SlashRoute::Pass(text) => text,
        };

        // 回合边界维护（压缩方案 §4.2.3）：先 prune、重估，仍超线再自动压缩（auto_compact 开）。
        // prune/自动压缩事件同样发生在 ChannelSink 挂载前——不补推则自动压缩边界实时不上屏
        //（与斜杠收尾同一通路坑，2026-09-18 自动压缩实测发现），前端迟到总线帧靠 _seqs 去重。
        let extra = self.boundary_maintenance(&mut session, &user_input).await;
        self.persist_extra_events(session_id, &extra)?;
        if let Some(sink) = &sink {
            for ev in &extra {
                sink.on_event(ev);
            }
        }

        // 改造二：本回合在途期间到达的后台子任务回执轮询缝——内核收口点（模型不再要工具）
        // 每次取队首一条，作为 UserMessage 注入本回合并续跑一轮（折进当前回复末尾）。
        let deferred = || {
            let mut pending = self.pending_receipts.lock().expect("pending receipts lock");
            let q = pending.get_mut(session_id)?;
            if q.is_empty() { None } else { Some(q.remove(0)) }
        };

        let mut outcome = self
            .execute_turn_locked(
                &mut session,
                session_id,
                &user_input,
                sink.clone(),
                subject.clone(),
                cancel,
                policy_override,
                Some(&deferred),
                false,
            )
            .await?;

        // 溢出救援（压缩方案 §4.2.6，1M 默认下的实际兜底主路径）：模型报上下文超限 →
        // 压缩（up_to_seq 覆盖失败回合，含其 UserMessage——重试走正常 run_turn 落新回合，
        // 投影不重复）→ 原话重试一次。无可压缩历史（首回合即超窗）或压缩失败 → 保留原错误结局。
        if matches!(outcome.reason, StopReason::Error)
            && outcome
                .final_text
                .as_deref()
                .is_some_and(is_context_overflow)
        {
            let (mut cevents, summary) = self
                .run_compaction(&mut session, CompactionReason::Overflow, None)
                .await;
            if summary.is_some() {
                self.persist_extra_events(session_id, &cevents)?;
                // 救援压缩边界同样补推流式 sink（挂载前的产物，与边界维护同因）
                if let Some(sink) = &sink {
                    for ev in &cevents {
                        sink.on_event(ev);
                    }
                }
                outcome.new_events.append(&mut cevents);
                if let Ok(retry) = self
                    .execute_turn_locked(
                        &mut session,
                        session_id,
                        &user_input,
                        sink.clone(),
                        subject.clone(),
                        cancel,
                        policy_override,
                        Some(&deferred),
                        false,
                    )
                    .await
                {
                    outcome.new_events.extend(retry.new_events);
                    outcome.turn = retry.turn;
                    outcome.reason = retry.reason;
                    outcome.steps = retry.steps;
                    outcome.final_text = retry.final_text;
                } // 重试路径 Err（持久化失败）：保留原错误结局
            }
        }

        // 收尾兜底 drain：仅在前一回合 Completed 时链式续跑（中断/出错即停，剩余回执
        // 留在队列里，下一次回合经 deferred 缝吸收，不丢）。回执回合不再挂流式 sink
        //（BusSink 已挂内存会话，实时广播照常）。
        while matches!(outcome.reason, cmx_agent_core::event::StopReason::Completed) {
            let next = {
                let mut pending = self.pending_receipts.lock().expect("pending receipts lock");
                let q = pending.get_mut(session_id);
                match q {
                    Some(q) if !q.is_empty() => Some(q.remove(0)),
                    _ => None,
                }
            };
            let Some(receipt) = next else { break };
            let r = self
                .execute_turn_locked(
                    &mut session,
                    session_id,
                    &receipt,
                    None,
                    None,
                    cancel,
                    None,
                    Some(&deferred),
                    false,
                )
                .await?;
            // 合并进同一 SendOutcome：IM 桥按 final_text 回消息，拼接各段不丢内容。
            outcome.new_events.extend(r.new_events);
            outcome.turn = r.turn;
            outcome.reason = r.reason;
            outcome.steps = r.steps;
            if let Some(text) = r.final_text {
                outcome.final_text = match outcome.final_text.take() {
                    Some(prev) if !prev.is_empty() => Some(format!("{prev}\n\n{text}")),
                    _ => Some(text),
                };
            }
        }
        Ok(outcome)
    }

    /// 执行一个回合并完成全部收尾（错误一致性 / 计划对账 / 墓碑 / 落库 / meta）。
    /// send_inner_locked 的主体：首条用户输入与兜底 drain 的后台回执回合共用；
    /// `attach_bus` 只在首回合传 true——内存会话跨回合复用，BusSink 重复挂载会广播翻倍。
    #[allow(clippy::too_many_arguments)]
    async fn execute_turn_locked(
        &self,
        session: &mut Session,
        session_id: &str,
        user_input: &str,
        sink: Option<std::sync::Arc<crate::stream::ChannelSink>>,
        subject: Option<cmx_agent_core::Subject>,
        cancel: &TurnCancel,
        policy_override: Option<cmx_agent_core::TurnPolicyOverride>,
        deferred: Option<&(dyn Fn() -> Option<String> + Send + Sync)>,
        attach_bus: bool,
    ) -> AppResult<SendOutcome> {
        if let Some(sys) = self.current_system_prompt() {
            *session = std::mem::replace(session, Session::new(session_id)).with_system(sys);
        }

        // 会话可能属于非当前空间（切走当前空间后回到旧任务继续）：按会话所属空间取根，
        // 作为**回合级快照**随调用传入内核——保证本回合 fs 工具与 @ 提示解析到同一个工作空间，
        // 且不再写共享 policy（旧实现两会话并发回合互相覆盖对方的工作空间根）。
        let turn_roots = self.session_workspace_roots(session_id)?;

        // 系统提示词与回合根同源（红蓝审查 P2-4）：提示词里的空间段落按**会话所属空间**生成
        //（无绑定/空间已移除时回合根回落共享 policy 的当前空间，提示词同规回落），
        // 旧实现恒用全局当前空间，切空间后回旧会话发消息会「提示词说 A、工具落 B」。
        if let Some(sys) = self.session_system_prompt(session_id) {
            *session = std::mem::replace(session, Session::new(session_id)).with_system(sys);
        }

        // 计划模式（阶段二）：回合开始插活开关（值 = meta.plan_mode），整个回合任务树在
        // TURN_PLAN_MODE 作用域内（PlanModeGuard 每批现读）；收尾摘除并对账回写 meta——
        // 覆盖 exit_plan 批准后同回合翻旗标的落盘（§7.1）。
        let plan_initial = self
            .store
            .list()?
            .iter()
            .find(|m| m.id == session_id)
            .map(|m| m.plan_mode)
            .unwrap_or(false);
        let plan_flag = Arc::new(std::sync::atomic::AtomicBool::new(plan_initial));
        self.turn_plan_flags
            .lock()
            .expect("plan flags lock")
            .insert(session_id.to_string(), plan_flag.clone());
        if plan_initial {
            // 计划模式回合追加固定章节（§7.3）：session.system 每回合都由 current_system_prompt
            // 重置，此处追加只影响本回合的模型上下文。
            let sys = self.current_system_prompt().unwrap_or_default() + PLAN_MODE_PROMPT_SECTION;
            *session = std::mem::replace(session, Session::new(session_id)).with_system(sys);
        }
        // 后台子任务回执回合（阶段三注入器经 app.send 送达，桌面/IM/CLI 三通道全在 send_inner
        // 汇聚）：检测 <task_result 前缀，本回合追加转述措辞章节。放在计划模式块之后，
        // session.system() 已含该有的一切，直接续加不会互相覆盖。
        if user_input.starts_with("<task_result") {
            let sys = session.system().unwrap_or_default().to_string() + TASK_RESULT_PROMPT_SECTION;
            *session = std::mem::replace(session, Session::new(session_id)).with_system(sys);
        }

        // 流式：加载完历史后挂 sink（历史用 push_restored 不触发 sink，故只流式本回合新事件）。
        // 同一个 sink 既是事件 EventSink（全量事件）又是 TurnObserver（文字增量）。
        let before = session.log.len();
        // U16：始终挂事件总线 sink——任何来源（本地 / IM 桥）的本回合事件都广播给 `/api/subscribe` 订阅者。
        // 仅首回合挂：内存会话跨兜底 drain 回合复用，重复挂载同一广播器会让每条事件双发。
        if attach_bus {
            session.log.add_sink(Arc::new(crate::bus::BusSink::new(
                session_id.to_string(),
                self.event_bus.sender(),
            )));
        }
        let outcome = match (&sink, &subject) {
            (Some(s), Some(subj)) => {
                let observed = Arc::new(s.with_cancel(cancel.clone()));
                session.log.add_sink(observed.clone());
                self.agent
                    .run_turn_observed_as_cancellable_with_policy_deferred(
                        &mut *session,
                        user_input,
                        Some(observed.as_ref()),
                        Some(subj),
                        Some(cancel),
                        turn_roots.as_ref().map(std::slice::from_ref),
                        policy_override,
                        Some(plan_flag.clone()),
                        deferred,
                    )
                    .await
            }
            (Some(s), None) => {
                let observed = Arc::new(s.with_cancel(cancel.clone()));
                session.log.add_sink(observed.clone());
                self.agent
                    .run_turn_observed_as_cancellable_with_policy_deferred(
                        &mut *session,
                        user_input,
                        Some(observed.as_ref()),
                        None,
                        Some(cancel),
                        turn_roots.as_ref().map(std::slice::from_ref),
                        policy_override,
                        Some(plan_flag.clone()),
                        deferred,
                    )
                    .await
            }
            (None, Some(subj)) => self
                .agent
                .run_turn_observed_as_cancellable_with_policy_deferred(&mut *session, user_input, None, Some(subj), Some(cancel), turn_roots.as_ref().map(std::slice::from_ref), policy_override, Some(plan_flag.clone()), deferred)
                .await,
            (None, None) => self
                .agent
                .run_turn_observed_as_cancellable_with_policy_deferred(&mut *session, user_input, None, None, Some(cancel), turn_roots.as_ref().map(std::slice::from_ref), policy_override, Some(plan_flag.clone()), deferred)
                .await,
        };
        // 出错一致性（飞书 ↔ 界面）：回合中途模型失败等会让 run_turn 提前返回 Err。若直接 `?` 抛出，
        // 已 append 的 TurnStarted/UserMessage 不会落库、不广播；错误文案只被 ImBridge 发给飞书，
        // 界面什么都看不到 → 两端不一致。故捕获错误：把错误文案作为 Note 事件追加（同步广播给界面
        // + 落库），补一条 TurnEnded(Error)，把错误文案塞进 final_text——ImBridge 据此回发飞书，
        // 界面经事件总线拿到同一份内容，两端一致。
        let outcome = match outcome {
            Ok(o) => o,
            Err(e) => {
                let msg = format!("{TURN_ERROR_NOTE_PREFIX}{e}");
                session.log.append(cmx_agent_core::event::EventKind::Note {
                    text: msg.clone(),
                });
                // turn 号 = 本回合应有编号（内核 TurnStarted 也取 next_turn_no()，红蓝审查 P3-6：
                // 旧实现的 -1 会把错误收尾记到上一个已完成回合头上）。
                let turn = session.next_turn_no();
                session.log.append(cmx_agent_core::event::EventKind::TurnEnded {
                    turn,
                    reason: cmx_agent_core::event::StopReason::Error,
                    steps: 0,
                    usage: None,
                });
                cmx_agent_core::TurnOutcome {
                    turn,
                    reason: cmx_agent_core::event::StopReason::Error,
                    steps: 0,
                    final_text: Some(msg),
                    usage: None,
                }
            }
        };

        // 计划模式收尾对账（§7.1）：摘活开关；flag 与回合初值不一致（exit_plan 批准后内核
        // 同回合翻 false 是唯一变化路径）→ 落 Note 事件（随本回合事件一起持久化 + 广播），
        // 并以 flag 为准持久化 meta。
        let flag_now = {
            let removed = self
                .turn_plan_flags
                .lock()
                .expect("plan flags lock")
                .remove(session_id);
            removed.map(|f| f.load(std::sync::atomic::Ordering::Relaxed))
        };
        let plan_final = flag_now.unwrap_or(plan_initial);
        if plan_initial && !plan_final {
            session.log.append(cmx_agent_core::event::EventKind::Note {
                text: "计划模式已退出（计划已批准），进入实现阶段".to_string(),
            });
        } else if !plan_initial && plan_final {
            session.log.append(cmx_agent_core::event::EventKind::Note {
                text: "计划模式已开启".to_string(),
            });
        }

        // 回合期间会话被删除（墓碑在）→ 放弃落库：本回合事件随删除一起蒸发，
        // 不写 append/put_meta 把已删会话复活。墓碑消费掉（同 id 再发新消息视为新建复活）。
        if self
            .pending_deleted
            .lock()
            .expect("pending deleted lock")
            .remove(session_id)
        {
            eprintln!("[session] {session_id} 回合期间被删除，放弃落库（防僵尸复活）");
            return Ok(SendOutcome {
                session_id: session_id.to_string(),
                turn: outcome.turn,
                reason: outcome.reason,
                steps: outcome.steps,
                final_text: outcome.final_text,
                new_events: Vec::new(),
            });
        }

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
            created_at: prev.as_ref().map(|m| m.created_at).unwrap_or(now),
            updated_at: now,
            event_count: session.log.len(),
            workspace_id: prev.as_ref().and_then(|m| m.workspace_id.clone()).or_else(|| self.current_workspace_id()),
            // 计划模式：以回合收尾对账后的 flag 值为准（覆盖 exit_plan 批准的落盘；
            // 无活开关变化时保住 prev 值——否则每回合 meta 重建会把它抹成 false）。
            plan_mode: plan_final,
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
        // 阶段三：清计划模式旗标残项（红队 N6 内存泄漏点；子任务级联在 cancel_session_turn 内）。
        self.turn_plan_flags
            .lock()
            .expect("plan flags lock")
            .remove(session_id);
        // 有在途回合先取消（含打断审批等待 + 级联取消子任务），并落墓碑：回合收尾的持久化
        // 步骤查到墓碑即放弃落库——否则回合结束 append_events/put_meta 会把刚删的会话「僵尸复活」。
        self.cancel_session_turn(session_id);
        self.pending_deleted
            .lock()
            .expect("pending deleted lock")
            .insert(session_id.to_string());
        let r = self.store.delete(session_id);
        if r.is_ok() {
            // 删除成功且无在途回合 → 墓碑即无用，摘除（有回合时墓碑由回合收尾消费）。
            let mut t = self.pending_deleted.lock().expect("pending deleted lock");
            if self.active_turns.lock().expect("active turns lock").get(session_id).is_none() {
                t.remove(session_id);
            }
        }
        r
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// U16：会话事件总线引用（壳的 `/api/subscribe` SSE 订阅它，实现 IM 事件实时推前端）。
    pub fn event_bus(&self) -> &Arc<crate::bus::SessionEventBus> {
        &self.event_bus
    }

    /// 列出工作空间与当前选择。
    pub fn list_workspaces(&self) -> AppResult<serde_json::Value> {
        self.workspace_registry()?.list()
    }

    pub fn select_workspace(&self, id: Option<&str>) -> AppResult<serde_json::Value> {
        let registry = self.workspace_registry()?;
        let value = registry.select(id)?;
        registry.set_allowed_roots(&self.agent)?;
        Ok(value)
    }

    pub fn create_workspace(&self, name: &str) -> AppResult<serde_json::Value> {
        let registry = self.workspace_registry()?;
        let value = registry.create_managed(name)?;
        registry.set_allowed_roots(&self.agent)?;
        Ok(value)
    }

    pub fn add_local_workspace(
        &self,
        path: &str,
        name: Option<&str>,
    ) -> AppResult<serde_json::Value> {
        let registry = self.workspace_registry()?;
        let value = registry.add_local(path, name)?;
        registry.set_allowed_roots(&self.agent)?;
        Ok(value)
    }

    /// 把空间移出列表（不删磁盘目录）；移除当前空间时回落任务模式（default）。
    pub fn remove_workspace(&self, id: &str) -> AppResult<serde_json::Value> {
        let registry = self.workspace_registry()?;
        let value = registry.remove(id)?;
        registry.set_allowed_roots(&self.agent)?;
        Ok(value)
    }

    /// 用系统文件浏览器打开空间文件夹（侧栏空间菜单「打开文件夹」）。
    pub fn open_workspace_folder(&self, id: &str) -> AppResult<serde_json::Value> {
        self.workspace_registry()?.open_folder(id)
    }

    /// 当前工作空间内检索文件，供输入框 @ 悬浮选择。
    pub async fn search_workspace_files(
        &self,
        query: &str,
        limit: Option<usize>,
        session_id: Option<&str>,
    ) -> AppResult<Vec<crate::workspace::WorkspaceFile>> {
        let registry = self.workspace_registry()?;
        let query = query.to_string();
        let limit = limit.unwrap_or(80).clamp(1, 200);
        // @ 提示按「会话创建时所属空间」检索；无会话上下文（首页新建任务）时用当前空间——
        // 新任务正是要在当前空间里创建，两者一致。
        let workspace_id = match session_id {
            Some(sid) => self
                .store
                .list()?
                .into_iter()
                .find(|m| m.id == sid)
                .and_then(|m| m.workspace_id),
            None => None,
        };
        // 文件索引可能访问大目录；放在阻塞线程里，避免拖慢前端协议派发。
        tokio::task::spawn_blocking(move || match workspace_id {
            Some(id) => registry.search_files_in(&id, &query, limit),
            None => registry.search_files(&query, limit),
        })
        .await
        .map_err(|e| AppError::Agent(format!("文件检索任务失败：{e}")))?
    }

    /// 会话所属空间的文件根（**回合级快照**，随调用传给内核 ToolCtx）。
    /// 会话无记录 / 空间已移除 → `None`：回落共享 policy 的当前空间根（任务模式语义，
    /// 当前空间切换时由 select_workspace 即时刷新 policy）。
    fn session_workspace_roots(&self, session_id: &str) -> AppResult<Option<std::path::PathBuf>> {
        let registry = self.workspace_registry()?;
        let workspace_id = self
            .store
            .list()?
            .into_iter()
            .find(|m| m.id == session_id)
            .and_then(|m| m.workspace_id);
        match workspace_id {
            Some(id) => registry.roots_for(&id),
            None => Ok(None),
        }
    }

    /// 会话视角的工作空间上下文 `(id, name, root)`（红蓝审查 P2-4）：会话绑定的空间在册且
    /// 根可解析 → 该空间；否则 `None`（调用方回落全局当前空间，与 [`Self::session_workspace_roots`]
    /// 返回 None 时回合根的回落目标同源）。
    fn session_workspace_context(
        &self,
        session_id: &str,
    ) -> Option<(String, String, std::path::PathBuf)> {
        let registry = self.workspaces.as_ref()?;
        let workspace_id = self
            .store
            .list()
            .ok()?
            .into_iter()
            .find(|m| m.id == session_id)
            .and_then(|m| m.workspace_id)?;
        // 空间已移除 → roots_for 为 None → 回落（与回合根行为一致）。
        match registry.roots_for(&workspace_id).ok().flatten() {
            Some(_) => registry.context_for(&workspace_id),
            None => None,
        }
    }

    /// 系统提示词（**会话同源版**，send_inner 每回合重建 session.system 时用）：
    /// 空间段落按会话所属空间生成；无绑定/已移除时回落全局当前空间，再无则「未绑定」。
    fn session_system_prompt(&self, session_id: &str) -> Option<String> {
        let base = self.default_system.clone().unwrap_or_else(|| {
            "你是 cmx 企业桌面智能体。用简洁中文回答，优先动手完成用户任务。".to_string()
        });
        let context = match self
            .session_workspace_context(session_id)
            .or_else(|| {
                self.workspaces
                    .as_ref()
                    .and_then(|w| w.current_context())
            }) {
            Some((_, name, path)) => format!(
                "\n\n当前工作空间：{name}\n工作空间根：{}\n文件操作使用相对该根的路径；用户用 @ 引用的文件也相对该根解析。",
                path.display()
            ),
            None => "\n\n当前未绑定工作空间：这是普通任务，不要主动创建或改写本地文件。".to_string(),
        };
        Some(base + &context + &self.skills_prompt_section())
    }

    // ── 斜杠菜单三分与真技能体系（姊妹方案 §2.1–§2.3，2026-09-18 拍板）──

    /// `/` 菜单三类条目（命令/技能/子智能体）。`list_skills`（工具契约改名）保持原样不动
    /// ——agents.js 子智能体编辑器拿它做工具白名单选项。
    pub fn list_slash(&self) -> serde_json::Value {
        let commands = vec![
            serde_json::json!({"name": "compact", "description": "压缩上下文：把较早对话总结成结构化摘要（可附关注点，如 /compact 保留结论）"}),
            serde_json::json!({"name": "plan", "description": "进入计划模式：只做调研与规划，不改文件（可附任务，如 /plan 重构登录页）"}),
        ];
        let skills: Vec<serde_json::Value> = self
            .scan_skills()
            .map(|entries| {
                entries
                    .iter()
                    .map(|e| serde_json::json!({
                        "name": e.name,
                        "description": e.description,
                        "invalid": e.invalid,
                    }))
                    .collect()
            })
            .unwrap_or_default();
        let subagents: Vec<serde_json::Value> = self
            .agents
            .as_ref()
            .map(|reg| {
                reg.list()
                    .into_iter()
                    .filter(|a| a.enabled)
                    .map(|a| {
                        serde_json::json!({
                            "name": a.name,
                            "title": a.title,
                            "description": a.description,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        serde_json::json!({ "commands": commands, "skills": skills, "subagents": subagents })
    }

    /// 技能目录扫描（目录自动初始化 + 种子示例；None = 未配置技能目录）。
    fn scan_skills(&self) -> Option<Vec<crate::skills::SkillEntry>> {
        let dir = self.skills_dir.as_deref()?;
        crate::skills::ensure_initialized(dir).ok()?;
        Some(crate::skills::scan(dir))
    }

    /// 技能清单注入（姊妹方案 §2.1「模型感知」）：没有这段，模型自己不知道技能存在，
    /// 只有用户敲 `/名` 才能触发。与 `list_slash` 同源扫描；system 每回合重建 → 恒新鲜。
    fn skills_prompt_section(&self) -> String {
        let Some(entries) = self.scan_skills() else {
            return String::new();
        };
        let usable: Vec<_> = entries.iter().filter(|e| e.invalid.is_none()).collect();
        if usable.is_empty() {
            return String::new();
        }
        let mut sec = String::from(
            "\n\n【可用技能】用户消息以「/技能名 参数」形式调用技能，调用时系统会注入技能正文；用户口头描述的任务若与某技能描述明显匹配，可提示用户用对应技能：",
        );
        for e in &usable {
            sec.push_str(&format!("\n- {}：{}", e.name, e.description));
        }
        sec
    }

    /// 斜杠分发（§2.2）：`send_inner_locked` 持会话锁后、落任何事件前调用。
    /// 优先级 命令 > 技能 > 子智能体 > 原样；未匹配 `/xxx` 原样发送（现状行为，不报错）。
    async fn dispatch_slash(
        &self,
        session_id: &str,
        session: &mut Session,
        user_input: &str,
    ) -> AppResult<crate::app::SlashRoute> {
        let trimmed = user_input.trim_start();
        if !trimmed.starts_with('/') {
            return Ok(crate::app::SlashRoute::Pass(user_input.to_string()));
        }
        let body = &trimmed[1..];
        let mut parts = body.splitn(2, char::is_whitespace);
        let name = parts.next().unwrap_or("").trim();
        let args = parts.next().unwrap_or("").trim();
        if name.is_empty() {
            return Ok(crate::app::SlashRoute::Pass(user_input.to_string()));
        }
        match name {
            // /compact [关注点]：消费输入，不落 UserMessage（压缩事件经总线实时上屏）
            "compact" => {
                let focus = (!args.is_empty()).then_some(args);
                let (mut events, summary) = self
                    .run_compaction(session, CompactionReason::Manual, focus)
                    .await;
                // 空表 = 无可压缩历史（刚压过/尚无对话）：补「无需压缩」收尾 Note——不给收尾
                // 事件，前端只能按中断兜底画红字、IM 会误报失败（ZCode 同款灰字 2026-09-18）。
                // auto/救援路径保持静默空表，不补此事件。
                let text = match summary {
                    Some(sum) => {
                        format!("🗜 上下文已压缩（手动）· 摘要 {} 字", sum.chars().count())
                    }
                    None if events.is_empty() => {
                        let note = session
                            .log
                            .append(EventKind::Note { text: COMPACT_NOOP_TEXT.to_string() })
                            .clone();
                        events.push(note);
                        COMPACT_NOOP_TEXT.to_string()
                    }
                    None => "压缩失败，已保留现场（摘要请求未成功，详见时间线提示）".to_string(),
                };
                self.persist_extra_events(session_id, &events)?;
                Ok(crate::app::SlashRoute::Handled(SendOutcome {
                    session_id: session_id.to_string(),
                    turn: session.next_turn_no(),
                    reason: StopReason::Completed,
                    steps: 0,
                    final_text: Some(text),
                    new_events: events,
                }))
            }
            // /plan [任务]：映射既有计划模式；带任务则改写文本继续跑回合（execute_turn_locked
            // 按 meta.plan_mode 自动进入只读档）。本分发器在会话锁内 → 必须走免锁内层
            "plan" => {
                self.set_plan_mode_inner(session_id, true).await?;
                if args.is_empty() {
                    Ok(crate::app::SlashRoute::Handled(SendOutcome {
                        session_id: session_id.to_string(),
                        turn: session.next_turn_no(),
                        reason: StopReason::Completed,
                        steps: 0,
                        final_text: Some("已进入计划模式（只调研规划，不改文件）".to_string()),
                        new_events: Vec::new(),
                    }))
                } else {
                    Ok(crate::app::SlashRoute::Pass(args.to_string()))
                }
            }
            _ => {
                // 技能：名称精确命中（非法条目不执行）→ 注入消息进正常回合
                if let Some(e) = self.scan_skills().and_then(|entries| {
                    entries
                        .into_iter()
                        .find(|e| e.name == name && e.invalid.is_none())
                }) {
                        let injected = format!(
                            "[用户调用技能 {name}，请严格按其指引执行]\n\
                             <skill-instruction name=\"{name}\">\n{}\n\n技能基准目录：{}；\
                             正文中出现的相对路径（scripts/、references/ 等）均相对该目录。\n\
                             </skill-instruction>\n\n用户指令：{}",
                            e.body.trim(),
                            e.dir.display(),
                            if args.is_empty() { "（无附加参数，按技能正文执行）" } else { args },
                        );
                return Ok(crate::app::SlashRoute::Pass(injected));
                }
                // 子智能体：一期软委派——注入指令让主对话用 task 工具委派（注册表与 task 工具现成）
                if let Some(reg) = &self.agents
                    && let Some(a) = reg.list().into_iter().find(|a| a.enabled && a.name == name)
                {
                    let target = if a.title.is_empty() { a.name.clone() } else { a.title.clone() };
                    let injected = format!(
                        "请把以下事项委派给子智能体「{target}」执行：使用 task 工具，\
                         subagent_type=\"{}\"，并把下方事项原文作为 prompt 传入；\
                         等待其完成后向用户汇报结果。\n\n事项：{}",
                        a.name,
                        if args.is_empty() { "（用户未附具体事项，先向用户确认要做什么）" } else { args },
                    );
                    return Ok(crate::app::SlashRoute::Pass(injected));
                }
                Ok(crate::app::SlashRoute::Pass(user_input.to_string()))
            }
        }
    }

    // ── 上下文压缩编排（压缩方案 §4.2，auto/manual/rescue 三路共用）──

    /// 压缩执行：序列化压缩段 → 摘要请求（max_tokens 有界 + 自溢出逐条丢最旧）→ 落
    /// `Compacted` 事件。返回（新事件、摘要正文）；摘要 None = 失败降级（已记审计 Note，
    /// 不压缩，绝不因压缩失败丢历史）。无可压缩历史返回空表。
    async fn run_compaction(
        &self,
        session: &mut Session,
        reason: CompactionReason,
        focus: Option<&str>,
    ) -> (Vec<SessionEvent>, Option<String>) {
        let events: Vec<SessionEvent> = session.log.events().to_vec();
        let mut cutoff = 0u64;
        let mut prior: Option<&String> = None;
        for ev in &events {
            if let EventKind::Compacted { up_to_seq, summary, .. } = &ev.kind {
                cutoff = *up_to_seq;
                prior = Some(summary);
            }
        }
        let window_events: Vec<SessionEvent> =
            events.iter().filter(|e| e.seq > cutoff).cloned().collect();
        let entries = cmx_agent_core::compaction::serialize_entries(&window_events);
        if entries.is_empty() {
            return (Vec::new(), None);
        }
        // 自溢出防护（codex remove_first_item 同款）：序列化文本超窗 → 从最旧条目逐条丢弃
        let (window, _) = self.current_model_window();
        let budget = window
            .saturating_sub(cmx_agent_core::compaction::SUMMARY_MAX_TOKENS + 2_000);
        let (conversation, dropped) =
            cmx_agent_core::compaction::fit_entries(&entries, budget);
        let prompt = cmx_agent_core::compaction::summary_prompt(
            prior.map(|s| s.as_str()),
            &conversation,
            focus,
        );
        let ctx = ModelContext {
            system: None,
            messages: vec![ModelMessage::User { text: prompt }],
            tools: Vec::new(),
        };
        let summary = match self
            .agent
            .complete_bounded(&ctx, cmx_agent_core::compaction::SUMMARY_MAX_TOKENS)
            .await
        {
            Ok(resp) => resp.text.filter(|t| !t.trim().is_empty()),
            Err(e) => {
                eprintln!("[compact] 摘要请求失败: {e}");
                None
            }
        };
        let mut new_events: Vec<SessionEvent> = Vec::new();
        match summary {
            Some(text) => {
                let up_to = events.last().map(|e| e.seq).unwrap_or(0);
                let ev = session
                    .log
                    .append(EventKind::Compacted { up_to_seq: up_to, summary: text.clone(), reason })
                    .clone();
                new_events.push(ev);
                if dropped > 0 {
                    let note = session
                        .log
                        .append(EventKind::Note {
                            text: format!(
                                "压缩序列化超出预算，已丢弃最旧 {dropped} 条记录后再摘要（审计）"
                            ),
                        })
                        .clone();
                    new_events.push(note);
                }
                (new_events, Some(text))
            }
            None => {
                let note = session
                    .log
                    .append(EventKind::Note {
                        text: "上下文压缩失败：摘要请求未成功，已保留现场，可稍后重试 /compact"
                            .to_string(),
                    })
                    .clone();
                new_events.push(note);
                (new_events, None)
            }
        }
    }

    /// 回合边界维护（§4.2.3/§4.2.5，仅 auto_compact 开启时）：先 prune、重估，仍超线再压缩。
    /// 返回新追加事件（调用方持久化；SSE 由总线 sink 实时广播）。
    async fn boundary_maintenance(
        &self,
        session: &mut Session,
        incoming: &str,
    ) -> Vec<SessionEvent> {
        if !self.auto_compact_enabled() {
            return Vec::new();
        }
        let mut new_events = Vec::new();
        // ① prune：守卫全过且可清理量足才动手
        let already = self.pruned_union(session);
        let ids = cmx_agent_core::compaction::select_prune(session.log.events(), &already);
        if !ids.is_empty() {
            let ev = session
                .log
                .append(EventKind::ToolOutputsPruned { call_ids: ids })
                .clone();
            new_events.push(ev);
        }
        // ② 重估：真实 usage（未过时）与投影估算取大者 + 新消息估算；≥ usable×85% 再压缩
        let (window, max_out) = self.current_model_window();
        let est = self.estimate_context(session) + cmx_agent_core::token::est_tokens(incoming);
        if est as f64 >= usable_window(window, max_out) as f64 * AUTO_COMPACT_RATIO {
            let (evs, _) = self
                .run_compaction(session, CompactionReason::AutoThreshold, None)
                .await;
            new_events.extend(evs);
        }
        new_events
    }

    /// 已登记清理的 call_id 并集（prune 守卫③的输入）。
    fn pruned_union(&self, session: &Session) -> std::collections::HashSet<String> {
        let mut set = std::collections::HashSet::new();
        for ev in session.log.events() {
            if let EventKind::ToolOutputsPruned { call_ids } = &ev.kind {
                set.extend(call_ids.iter().cloned());
            }
        }
        set
    }

    /// 触发判定用量（§4.2.3）：真实 usage 优先（其 input 即请求时上下文规模），但仅当该回合
    /// 之后没发生过压缩/清理（否则真实值反映的是维护前的更大上下文，会误触发）；否则整投影估算。
    fn estimate_context(&self, session: &Session) -> u64 {
        let events = session.log.events();
        let mut maint_seq = 0u64;
        for ev in events {
            if matches!(
                ev.kind,
                EventKind::Compacted { .. } | EventKind::ToolOutputsPruned { .. }
            ) {
                maint_seq = ev.seq;
            }
        }
        for ev in events.iter().rev() {
            if let EventKind::TurnEnded { usage: Some(u), .. } = &ev.kind {
                if ev.seq > maint_seq {
                    return u.input;
                }
                break;
            }
        }
        let tools = self.agent.tools().specs();
        cmx_agent_core::token::est_context(&session.model_context(tools)).total
    }

    /// 当前模型的（上下文窗口, 最大输出）：providers.json 激活条目里按当前模型 id 查
    /// `ModelEntry`；缺省（1M / None）见压缩方案 §0 拍板。
    fn current_model_window(&self) -> (u64, Option<u64>) {
        if let Some((_, pf)) = self.load_providers()
            && let Some(active_id) = pf.active.as_deref()
            && let Some(p) = pf.get(active_id)
            && let Some(m) = p.models.iter().find(|m| m.id == p.config.model)
        {
            return (
                m.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW),
                m.max_output_tokens,
            );
        }
        (DEFAULT_CONTEXT_WINDOW, None)
    }

    /// 自动压缩开关（providers.json 顶层 `auto_compact`，缺省 true；关闭后仅手动 + 救援）。
    fn auto_compact_enabled(&self) -> bool {
        self.load_providers()
            .map(|(_, pf)| pf.auto_compact)
            .unwrap_or(true)
    }

    /// 追加事件落库（压缩/命令路径专用；回合事件由 execute_turn_locked 统一落）。
    /// 墓碑在册（会话正被删除）时放弃——与 execute_turn_locked 同规，防已删会话复活。
    fn persist_extra_events(&self, session_id: &str, events: &[SessionEvent]) -> AppResult<()> {
        if events.is_empty() {
            return Ok(());
        }
        if self
            .pending_deleted
            .lock()
            .expect("pending deleted lock")
            .contains(session_id)
        {
            return Ok(());
        }
        self.store.append_events(session_id, events)?;
        if let Ok(metas) = self.store.list()
            && let Some(m) = metas.iter().find(|m| m.id == session_id)
        {
            let mut meta = m.clone();
            meta.event_count += events.len();
            meta.updated_at = chrono::Utc::now();
            self.store.put_meta(&meta)?;
        }
        Ok(())
    }

    /// 当前会话上下文用量（圆环+明细卡，§4.4.2）：used=最近真实 usage（无则退投影估算），
    /// 分类占比=估算器分桶，缓存命中率=最近一次 cached_input。
    pub fn context_usage(&self, session_id: &str) -> AppResult<serde_json::Value> {
        let session = self.store.load(session_id)?;
        let tools = self.agent.tools().specs();
        let breakdown = cmx_agent_core::token::est_context(&session.model_context(tools));
        let (window, max_out) = self.current_model_window();
        let mut used = breakdown.total;
        let mut cached = None;
        for ev in session.log.events().iter().rev() {
            if let EventKind::TurnEnded { usage: Some(u), .. } = &ev.kind {
                used = u.input;
                cached = u.cached_input;
                break;
            }
        }
        let usable = usable_window(window, max_out);
        let pct = (used as f64 / usable.max(1) as f64 * 1000.0).round() / 10.0;
        Ok(serde_json::json!({
            "used": used,
            "window": window,
            "usable": usable,
            "pct": pct,
            "cached_input": cached,
            "breakdown": {
                "messages": breakdown.messages,
                "tool_results": breakdown.tool_results,
                "tool_defs": breakdown.tool_defs,
                "system": breakdown.system,
                "other": breakdown.other,
                "total": breakdown.total,
            },
        }))
    }

    /// 手动压缩（壳直调入口；`/compact` 文本识别在 dispatch_slash，两者共用 run_compaction）。
    pub async fn compact_session(
        &self,
        session_id: &str,
        focus: Option<String>,
    ) -> AppResult<serde_json::Value> {
        // 与回合互斥：同一会话锁（压缩也是对日志的写操作）
        let session_lock = {
            let mut locks = self.session_locks.lock().await;
            locks
                .entry(session_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _permit = session_lock.lock().await;
        let mut session = self.store.load(session_id)?;
        let (events, summary) = self
            .run_compaction(&mut session, CompactionReason::Manual, focus.as_deref())
            .await;
        self.persist_extra_events(session_id, &events)?;
        Ok(serde_json::json!({
            "compacted": summary.is_some(),
            "summary_chars": summary.as_ref().map(|s| s.chars().count()).unwrap_or(0),
        }))
    }

    /// 技能 = 当前已注册工具。前端 / 菜单只做选择，发送后模型仍经工具契约与守卫执行。
    pub fn list_skills(&self) -> serde_json::Value {
        let skills: Vec<serde_json::Value> = self
            .agent
            .tools()
            .specs()
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "id": s.name,
                    "name": s.name,
                    "title": s.name,
                    "description": s.description,
                })
            })
            .collect();
        serde_json::json!({ "skills": skills })
    }

    /// 中断活动回合；若正卡在审批等待，也同步拒绝本会话待决审批/待答提问。
    pub fn cancel_session_turn(&self, session_id: &str) -> bool {
        if let Some(a) = &self.approver {
            a.cancel_session(session_id);
        }
        // 同步忽略本会话待决提问：走 oneshot 唤醒（而非杀任务）→ 工具路径回灌 dismissed
        // 结果 → 回合在旗标边界收尾。提问挂起点不经旗标检查，不走 oneshot 就永远停在那里。
        self.agent.questions().cancel_session(session_id);
        // 阶段三：级联取消该会话派生的全部子任务（前台 join_all 内与后台 spawn 的子回合——
        // 修复旧实现对 subtask-* 取消恒 no-op），并连带清掉 subtask-* 会话名下挂起的
        // 审批/提问（审批可挂在子会话名下，是实测路径）。
        if let Some(h) = &self.subagents {
            for sub_id in h.cancel_for_parent_with_ids(session_id) {
                if let Some(a) = &self.approver {
                    a.cancel_session(&sub_id);
                }
                self.agent.questions().cancel_session(&sub_id);
            }
        }
        self.active_turns
            .lock()
            .expect("active turns lock")
            .get(session_id)
            .map(|c| {
                c.cancel();
                true
            })
            .unwrap_or(false)
    }

    /// 前端送回提问答案（`answer_question` 命令）：唤醒挂起的回合。返回是否命中一个待决提问。
    /// 会话绑定校验在服务内（session_hint 必须与提问会话一致，且不允许空）。
    pub fn answer_question(
        &self,
        request_id: &str,
        answers: Vec<Vec<String>>,
        session_id: &str,
    ) -> bool {
        self.agent
            .questions()
            .answer(request_id, answers, session_id)
    }

    /// 前端忽略一个提问（`dismiss_question` 命令）：以 dismissed 唤醒挂起的回合。
    pub fn dismiss_question(&self, request_id: &str, session_id: &str) -> bool {
        self.agent.questions().dismiss(request_id, session_id)
    }

    /// 列待决提问（`list_pending_questions` 命令）：前端在途恢复主路径——回合事件回合末才
    /// 落库，刷新后 GetEvents 拿不到挂起中的提问，只能查进程内 pending。`session_id` 缺省列全部。
    pub fn list_pending_questions(
        &self,
        session_id: Option<&str>,
    ) -> Vec<cmx_agent_core::PendingQuestionInfo> {
        self.agent.questions().pending_in_session(session_id)
    }

    // ———————————————— 阶段一：子智能体注册表（§6.6）————————————————

    /// 列子智能体（内置两条带覆盖项 + 自定义）。未装配注册表（旧测试/CLI）返回空内置表。
    pub fn list_agents(&self) -> AppResult<serde_json::Value> {
        match &self.agents {
            Some(r) => {
                let list = r.list();
                let builtin: Vec<serde_json::Value> = list
                    .iter()
                    .filter(|a| a.builtin)
                    .map(agent_spec_json)
                    .collect();
                let custom: Vec<serde_json::Value> =
                    r.customs().iter().map(agent_spec_json).collect();
                Ok(serde_json::json!({ "builtin": builtin, "custom": custom }))
            }
            None => Ok(serde_json::json!({ "builtin": [], "custom": [] })),
        }
    }

    /// 保存子智能体：内置只允许改 model/enabled（覆盖项）；自定义 upsert（name 唯一）。
    /// 保存即热生效（共享清单 + TaskTool::spec() 动态枚举）。
    pub fn save_agent(
        &self,
        spec: cmx_agent_core::agents::AgentSpec,
    ) -> AppResult<serde_json::Value> {
        let registry = self
            .agents
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("子智能体注册表未启用".into()))?;
        let name = spec.name.trim().to_string();
        let is_builtin = cmx_agent_core::agents::builtin_specs().iter().any(|b| b.name == name);
        if is_builtin {
            registry.set_builtin_override(&name, spec.model.clone(), spec.enabled)?;
        } else {
            registry.upsert_custom(cmx_agent_core::agents::AgentSpec {
                builtin: false,
                ..spec
            })?;
        }
        Ok(serde_json::json!({ "saved": name, "hot": true }))
    }

    /// 删除一个自定义子智能体（内置/未知 → 拒绝）。删除即热生效。
    pub fn delete_agent(&self, name: &str) -> AppResult<serde_json::Value> {
        let registry = self
            .agents
            .as_ref()
            .ok_or_else(|| AppError::BadRequest("子智能体注册表未启用".into()))?;
        registry.delete_custom(name)?;
        Ok(serde_json::json!({ "deleted": name }))
    }

    // ———————————————— 阶段二：计划模式（§7.1）————————————————

    /// 用户切换会话计划模式（模型无任何切换工具，用户独占）。
    /// 先取该会话 `session_locks` permit（与回合生命周期串行——红队 N1：防落在「回合收尾
    /// 摘 flag 之后、对账回写之前」的窗口被旧 flag 覆盖），再校验会话存在（红队 N6：防
    /// put_meta 造幽灵会话），然后写 meta + 落 Note 事件（审计 + UI 系统行）。
    /// im-assistant / im-* 会话拒绝（IM 共享会话 + 无人值守 FULL_ACCESS，不进只读规划态）。
    pub async fn set_plan_mode(&self, session_id: &str, enabled: bool) -> AppResult<serde_json::Value> {
        if session_id == ASSISTANT_SESSION_ID || session_id.starts_with("im-") {
            return Err(AppError::BadRequest("IM 助理会话不支持计划模式".into()));
        }
        if !self.store.list()?.iter().any(|m| m.id == session_id) {
            return Err(AppError::NotFound(format!("session '{session_id}'")));
        }
        let permit = {
            let mut locks = self.session_locks.lock().await;
            locks
                .entry(session_id.to_string())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _permit = permit.lock().await;
        // 持锁后重读 meta（若刚才有回合在收尾，此处拿到的是对账后的最终值）。
        self.set_plan_mode_inner(session_id, enabled).await
    }

    /// 计划模式切换的内层实现：**调用方必须已持有该会话锁**（公开版负责加锁；
    /// 斜杠分发器的 /plan 在 send_inner_locked 锁内调用，走本函数防重复加锁死锁）。
    async fn set_plan_mode_inner(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> AppResult<serde_json::Value> {
        let mut meta = self
            .store
            .list()?
            .into_iter()
            .find(|m| m.id == session_id)
            .ok_or_else(|| AppError::NotFound(format!("session '{session_id}'")))?;
        if meta.plan_mode == enabled {
            return Ok(serde_json::json!({
                "session_id": session_id, "plan_mode": enabled, "changed": false,
            }));
        }
        meta.plan_mode = enabled;
        self.store.put_meta(&meta)?;
        // Note 事件：审计 + UI 系统行。回合外追加——直接构造事件持久化并经总线广播。
        // （不走 store.load：全新会话可能尚无 log.jsonl，load 会报 NotFound。）
        let note = cmx_agent_core::event::SessionEvent {
            seq: 0, // 落库/展示不依赖 seq（投影按 ts 排序；与回合内 append 的派生方式解耦）
            ts: chrono::Utc::now(),
            kind: cmx_agent_core::event::EventKind::Note {
                text: if enabled {
                    "计划模式已开启（只读调研档）".to_string()
                } else {
                    "计划模式已关闭".to_string()
                },
            },
        };
        self.store.append_events(session_id, std::slice::from_ref(&note))?;
        self.event_bus.publish(crate::bus::EventEnvelope {
            session_id: session_id.to_string(),
            parent: None, // 父会话自身的系统 note，非子智能体事件
            event: note,
        });
        // 有进行中回合时同步翻活 flag（立刻生效；无回合时下回合按 meta 读初值）。
        if let Some(f) = self
            .turn_plan_flags
            .lock()
            .expect("plan flags lock")
            .get(session_id)
        {
            f.store(enabled, std::sync::atomic::Ordering::Relaxed);
        }
        tracing::info!("计划模式切换：session={session_id} enabled={enabled}");
        Ok(serde_json::json!({
            "session_id": session_id, "plan_mode": enabled, "changed": true,
        }))
    }

    fn workspace_registry(&self) -> AppResult<Arc<crate::workspace::WorkspaceRegistry>> {
        self.workspaces
            .clone()
            .ok_or_else(|| AppError::BadRequest("工作空间未启用".into()))
    }

    fn current_workspace_id(&self) -> Option<String> {
        self.workspaces
            .as_ref()
            .and_then(|w| w.current_context())
            .map(|(id, _, _)| id)
    }

    fn current_system_prompt(&self) -> Option<String> {
        let base = self.default_system.clone().unwrap_or_else(|| {
            "你是 cmx 企业桌面智能体。用简洁中文回答，优先动手完成用户任务。".to_string()
        });
        let context = match self.workspaces.as_ref().and_then(|w| w.current_context()) {
            Some((_, name, path)) => format!(
                "\n\n当前工作空间：{name}\n工作空间根：{}\n文件操作使用相对该根的路径；用户用 @ 引用的文件也相对该根解析。",
                path.display()
            ),
            None => "\n\n当前未绑定工作空间：这是普通任务，不要主动创建或改写本地文件。".to_string(),
        };
        Some(base + &context + &self.skills_prompt_section())
    }
}

/// 子智能体类型 → 前门 JSON（ListAgents / 设置中心第七分区）。
fn agent_spec_json(a: &cmx_agent_core::agents::AgentSpec) -> serde_json::Value {
    serde_json::json!({
        "name": a.name,
        "title": a.title,
        "description": a.description,
        "tools": a.tools,
        "model": a.model,
        "system_prompt": a.system_prompt,
        "enabled": a.enabled,
        "builtin": a.builtin,
    })
}

/// 回合错误 note 的统一前缀。**前后端契约**：Web/Tauri 壳的 render.js 以
/// `startsWith("⚠ 处理出错")` 识别错误行（豁免回合折叠、红字强化），改这里必须同步改
/// render.js 的判定（报错可见性方案 20260917 A 项）。
pub(crate) const TURN_ERROR_NOTE_PREFIX: &str = "⚠ 处理出错：";

/// B2：按 base_url 推断 provider 展示名（模型选择器用；providers.json 有用户命名时优先，
/// 此函数仅 env / model.json 兜底路径）。
fn provider_label(base_url: &str) -> String {    let b = base_url.to_ascii_lowercase();
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

/// 会话校验错误分类：服务端**明确拒绝**（401/403 或鉴权类业务码）才算会话失效；
/// 传输/解析失败按网络问题处理（回放保留文件）。
fn is_auth_rejection(e: &cmx_agent_connectors::ClientError) -> bool {
    use cmx_agent_connectors::ClientError as C;
    match e {
        C::Http(c) => *c == 401 || *c == 403,
        C::Envelope { code, .. } => *code == 401 || *code == 403,
        _ => false,
    }
}

/// Unix 秒（会话落盘时间戳用）。
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 镜像门户 PasswordPolicy（cmx-auth password/policy.rs）本地快速失败；门户仍为最终裁决。
/// 改密与自助注册共用同一条策略。
fn check_password_policy(p: &str) -> Result<(), String> {
    if p.len() < 8 {
        return Err("密码长度不能少于 8 位".into());
    }
    if !p.chars().any(|c| c.is_ascii_uppercase()) {
        return Err("密码必须包含大写字母".into());
    }
    if !p.chars().any(|c| c.is_ascii_lowercase()) {
        return Err("密码必须包含小写字母".into());
    }
    if !p.chars().any(|c| c.is_ascii_digit()) {
        return Err("密码必须包含数字".into());
    }
    const PWD_SPECIAL: &str = "!@#$%^&*()_+-=[]{}|;':\",./<>?`~";
    if !p.chars().any(|c| PWD_SPECIAL.contains(c)) {
        return Err("密码必须包含特殊字符".into());
    }
    Ok(())
}

/// 注册专用错误文案：部署未放行 whitelist 时门户 mw_auth 拦 401/403——此时应明示
/// 「注册未开放」而非通用的认证失败，避免用户误以为账号密码输错。
fn friendly_register_error(e: cmx_agent_connectors::ClientError, base: &str) -> String {
    use cmx_agent_connectors::ClientError;
    match &e {
        ClientError::Http(401 | 403) => {
            "注册未开放：当前环境未启用自助注册，请联系管理员创建账号".to_string()
        }
        ClientError::Envelope { code: 401 | 403, .. } => {
            "注册未开放：当前环境未启用自助注册，请联系管理员创建账号".to_string()
        }
        _ => friendly_auth_error(e, base),
    }
}

/// 把连接器 [`ClientError`] 映射为面向用户的干净登录错误文案。
fn friendly_auth_error(e: cmx_agent_connectors::ClientError, base: &str) -> String {
    use cmx_agent_connectors::ClientError;
    match e {
        // 服务端业务错误（如 401 用户名/密码错误）——直接用其 msg（已是中文提示）。
        ClientError::Envelope { msg, .. } => msg,
        // 连不上认证服务（门户在内网：未连 VPN / 门户未启动 / 网络不通，对用户而言同一种表现）。
        ClientError::Transport(_) => {
            format!("无法连接认证服务（{base}），请检查 VPN / 内网连接，或确认 cmx 门户服务已启动")
        }
        // 限流（响应体通常已带「请在 N 秒后重试」的信封 msg 走上一分支；此处兜无体的底）。
        ClientError::Http(429) => "登录请求过于频繁，请几分钟后再试".to_string(),
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

#[cfg(test)]
#[path = "receipt_tests.rs"]
mod receipt_tests;
