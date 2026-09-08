//! [`DesktopAppBuilder`]：一键装配桌面壳后端。给定工作目录 + 模型缝，产出一个 [`AgentApp`]，
//! 其沙箱根设为工作目录（本地文件工具只能碰工作区）、挂上全部内置工具与五层守卫。
//!
//! 真实模型缝（HTTP 客户端）在此注入即可；桌面壳/CLI/Headless 前门均复用同一装配。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_connectors::{AuthConfig, AuthProvider, ConnectorConfig, ConnectorRegistry};
use cmx_agent_core::{
    Agent, ApprovalGuard, ApprovalPolicy, Approver, AuthGuard, AutoApprover, GuardPipeline,
    HighRiskGuard, ModelSeam, Policy, SandboxMode, Subject,
};
use cmx_agent_tools::default_registry;

use crate::app::AgentApp;
use crate::error::AppResult;
use crate::store::FileSessionStore;

/// 桌面壳后端装配器。
pub struct DesktopAppBuilder {
    workdir: PathBuf,
    data_dir: PathBuf,
    model: Arc<dyn ModelSeam>,
    approver: Arc<dyn Approver>,
    sandbox: SandboxMode,
    approval: ApprovalPolicy,
    subject: Subject,
    system: Option<String>,
    /// 连接器配置（None = 不挂连接器；Some = 挂 flow/onto/report 三连接器工具 + 面板）。
    connectors: Option<ConnectorConfig>,
    /// 认证配置（None = 不启用登录门；Some = 对接门户 /api/auth/*）。
    auth: Option<AuthConfig>,
    /// 交互式审批（X4）：true = 桌面壳交互审批卡片；false = 用 `approver`（CLI 自动审批）。
    interactive_approval: bool,
    /// MCP 工具（U3）：外部 MCP server 的工具代理（由壳异步连接后传入）。
    mcp_tools: Vec<Arc<dyn cmx_agent_core::Tool>>,
    /// U13 数据权限：Some(base_url) = 接 cmx-data-auth PDP 做权限接地（AuthGuard 换真 PEP）；None = allow_all 占位。
    data_auth: Option<String>,
    /// Web 壳 per-user 模型配置基目录：Some(base) = `<base>/<username>/model.json`；None = 全局（Tauri 用）。
    user_config_base: Option<PathBuf>,
}

impl DesktopAppBuilder {
    /// `workdir`：智能体可读写的工作区（沙箱根）。`data_dir`：会话落库根。`model`：模型缝。
    pub fn new(
        workdir: impl Into<PathBuf>,
        data_dir: impl Into<PathBuf>,
        model: Arc<dyn ModelSeam>,
    ) -> Self {
        Self {
            workdir: workdir.into(),
            data_dir: data_dir.into(),
            model,
            approver: Arc::new(AutoApprover::reject()),
            sandbox: SandboxMode::WorkspaceWrite,
            approval: ApprovalPolicy::OnRequest,
            subject: Subject::new("desktop-user"),
            system: Some(default_office_system_prompt()),
            connectors: None,
            auth: None,
            interactive_approval: false,
            mcp_tools: Vec::new(),
            data_auth: None,
            user_config_base: None,
        }
    }

    pub fn approver(mut self, a: Arc<dyn Approver>) -> Self {
        self.approver = a;
        self
    }

    /// 启用交互式审批（X4）：需审批的工具会挂起等前端点按。桌面壳（web/tauri）调用；CLI 不调。
    pub fn interactive_approval(mut self) -> Self {
        self.interactive_approval = true;
        self
    }

    /// 注入 MCP 工具代理（U3，由壳异步连接 MCP server 后传入）。
    pub fn mcp_tools(mut self, tools: Vec<Arc<dyn cmx_agent_core::Tool>>) -> Self {
        self.mcp_tools = tools;
        self
    }

    /// 启用 cmx 连接器（flow/onto/report）。传 `ConnectorConfig::default()` 即用本机标准端口。
    pub fn connectors(mut self, cfg: ConnectorConfig) -> Self {
        self.connectors = Some(cfg);
        self
    }

    /// 启用登录门（对接门户 /api/auth/*）。传 `AuthConfig::default()` 即用本机门户 :8080。
    pub fn auth(mut self, cfg: AuthConfig) -> Self {
        self.auth = Some(cfg);
        self
    }

    /// U13 启用数据权限接地：`base_url` 指 cmx-data-auth（如 http://127.0.0.1:8094）。
    /// 启用后 AuthGuard 用真 PEP（PDP /decide 判定 + 缓存 + fail-closed）替代 allow_all 占位。
    pub fn data_auth(mut self, base_url: impl Into<String>) -> Self {
        self.data_auth = Some(base_url.into());
        self
    }

    /// 便捷：`Some(非空)` 才启用数据权限，`None`/空串保持 allow_all（壳里读 env 直接传入）。
    pub fn maybe_data_auth(self, base_url: Option<String>) -> Self {
        match base_url {
            Some(u) if !u.is_empty() => self.data_auth(u),
            _ => self,
        }
    }

    /// 启用 per-user 模型配置目录（Web 壳调用）：已登录用户的配置存 `<dir>/<username>/model.json`。
    /// Tauri 壳不调用此方法，始终用全局 data_dir/model.json（单机单用户场景）。
    pub fn user_config_base(mut self, dir: impl Into<PathBuf>) -> Self {
        self.user_config_base = Some(dir.into());
        self
    }

    pub fn sandbox(mut self, s: SandboxMode) -> Self {
        self.sandbox = s;
        self
    }

    pub fn approval(mut self, p: ApprovalPolicy) -> Self {
        self.approval = p;
        self
    }

    pub fn subject(mut self, s: Subject) -> Self {
        self.subject = s;
        self
    }

    pub fn system(mut self, s: impl Into<String>) -> Self {
        self.system = Some(s.into());
        self
    }

    /// 装配。沙箱根 = workdir；挂全部内置工具 + 五层守卫（Auth/HighRisk/Approval）；
    /// 若启用连接器，把 flow/onto/report 三连接器工具也挂进注册表，并把 ConnectorRegistry 交给 app 供面板查询。
    pub fn build(self) -> AppResult<AgentApp> {
        // U13 数据权限：启用则 AuthGuard 用真 PEP（PDP /decide 判定 + 缓存 + fail-closed）；否则 allow_all 占位。
        // 授权按**当前登录用户**（共享 identity 单元，登录后由 app 更新 + 重热）——非静态 desktop 主体。
        let identity: Arc<std::sync::RwLock<Subject>> =
            Arc::new(std::sync::RwLock::new(self.subject.clone()));
        let mut data_auth_wire: Option<(Arc<cmx_agent_connectors::DataAuthPep>, Arc<std::sync::RwLock<Subject>>)> = None;
        let auth_guard = match &self.data_auth {
            Some(base) => {
                let pep = cmx_agent_connectors::DataAuthPep::new(
                    base.clone(),
                    self.subject.tenant.clone(),
                    self.subject.user.clone(),
                    true, // enforce=严格：缓存未命中/服务不可达即拒绝
                );
                // 初始预热（desktop 主体；登录后 app 会以真实用户重热覆盖）。build 非 async → 阻塞跑一次。
                let subject = self.subject.clone();
                let pep2 = pep.clone();
                if let Ok(h) = tokio::runtime::Handle::try_current() {
                    tokio::task::block_in_place(|| {
                        h.block_on(async {
                            pep2.prewarm(&subject, cmx_agent_connectors::ENFORCED_PERMS).await;
                        });
                    });
                }
                let pep_arc = Arc::new(pep);
                let pep_guard = pep_arc.clone();
                let id_guard = identity.clone();
                data_auth_wire = Some((pep_arc, identity.clone()));
                // 守卫按共享 identity 判权（忽略静态 ctx.subject）→ 登录后即以真实用户角色接地。
                AuthGuard::new(move |_ctx_subj, perm| {
                    let subj = id_guard.read().expect("auth identity");
                    pep_guard.cached_allow(&subj, perm)
                })
            }
            None => AuthGuard::allow_all(), // 未启用数据权限：显式放行占位
        };
        let mut guards = GuardPipeline::new();
        guards
            .add(Arc::new(auth_guard))
            .add(Arc::new(HighRiskGuard))
            .add(Arc::new(ApprovalGuard));

        let policy = Policy {
            sandbox: self.sandbox,
            approval: self.approval,
            max_steps: 16,
            allowed_roots: vec![self.workdir.clone()],
            subject: self.subject,
        };

        // 工具注册表：内置工具 + 子智能体 task 工具 + （可选）连接器工具。
        let mut registry = default_registry();
        // U1 子智能体：task 工具持 Weak<Agent> 句柄，构建出 Arc<Agent> 后注入（不成环）。max_depth=2。
        let sub_handle = Arc::new(cmx_agent_tools::SubagentHandle::new(2));
        registry.register(Arc::new(cmx_agent_tools::TaskTool::new(sub_handle.clone())));
        // U2 LSP：代码智能工具（懒连语言服务器，配置读 <data_dir>/lsp.json；无配置则调用时降级提示）。
        registry.register(Arc::new(cmx_agent_lsp::LspTool::from_config_file(
            &self.data_dir.join("lsp.json"),
        )));
        // U6 办公文档：读 Excel/PDF/Word/PPT + 写 Excel。
        registry.register(Arc::new(cmx_agent_office::DocReadTool));
        registry.register(Arc::new(cmx_agent_office::XlsxWriteTool));
        // U10 联网研究：web_fetch 抓网页取正文 + web_search 网络搜索（SSRF 基线；端点/私网经 env 配置）。
        registry.register(Arc::new(cmx_agent_net::WebFetchTool::from_env()));
        registry.register(Arc::new(cmx_agent_net::WebSearchTool::from_env()));
        // U8 浏览器：browser_read 无头 Chrome 渲染 JS 后取正文 + browser_screenshot 截图存工作区。
        registry.register(Arc::new(cmx_agent_net::BrowserReadTool::from_env()));
        registry.register(Arc::new(cmx_agent_net::BrowserScreenshotTool::from_env()));
        // U8 交互：browser_do 经 CDP 点击/填表/等待后取结果文本（需登录/搜索/点按的动态站点）。
        registry.register(Arc::new(cmx_agent_net::BrowserDoTool::from_env()));
        // U8 computer-use（视觉回环）：视觉模型配置从当前模型配置派生（CMX_AGENT_VISION_MODEL 覆盖模型名；
        // DeepSeek 默认用 vision-exp）。未配置模型则工具在调用时返回「未配置视觉模型」。
        let vision = cmx_agent_model::ModelProviderConfig::resolve(Some(self.data_dir.as_path())).map(|c| {
            let model = std::env::var("CMX_AGENT_VISION_MODEL").unwrap_or_else(|_| {
                if c.base_url.contains("deepseek") {
                    "deepseek-v4-flash-vision-exp".to_string()
                } else {
                    c.model.clone()
                }
            });
            cmx_agent_net::VisionCfg { base_url: c.base_url, api_key: c.api_key, model }
        });
        let allow_private = std::env::var("CMX_AGENT_NET_ALLOW_PRIVATE").is_ok();
        registry.register(Arc::new(cmx_agent_net::ComputerUseTool::new(allow_private, vision)));
        // U15 插件面：扫 <data_dir>/plugins/*/cmx-plugin.json，把 http/command/wasm 插件包装成工具 +
        // plugin_list/install/marketplace。保留清单摘要供前门 list_plugins（master-detail 视图）消费。
        let plugins_dir = self.data_dir.join("plugins");
        let (plugin_tools, plugin_manifests) = cmx_agent_plugin::load_plugins(&plugins_dir);
        for t in plugin_tools {
            registry.register(t);
        }
        registry.register(Arc::new(cmx_agent_plugin::PluginListTool::new(
            &plugin_manifests,
            &plugins_dir,
        )));
        registry.register(Arc::new(cmx_agent_plugin::PluginInstallTool::new(plugins_dir.clone())));
        registry.register(Arc::new(cmx_agent_plugin::PluginMarketplaceTool::default()));
        let plugin_summaries = cmx_agent_plugin::plugin_summaries(&plugin_manifests);
        let plugin_market = std::env::var("CMX_AGENT_PLUGIN_MARKET").ok().filter(|s| !s.is_empty());
        // 共享令牌槽：登录后 app 写入 access_token，连接器读出带 Bearer（auth=on 服务如 cmx-flow 必需）。
        let token_store: cmx_agent_connectors::TokenStore =
            Arc::new(std::sync::RwLock::new(None));
        let connectors = self.connectors.map(|cfg| {
            let mut cr = ConnectorRegistry::new(cfg).with_token(token_store.clone());
            // U13：启用数据权限时，把 PEP + 共享主体也给连接器 → business_chain 链内逐步判权。
            if let Some((pep, identity)) = &data_auth_wire {
                cr = cr.with_data_auth(pep.clone(), identity.clone());
            }
            cr.register_into(&mut registry);
            Arc::new(cr)
        });
        // U3：MCP server 工具代理（外部生态）挂进同一注册表——模型只见「工具」，不关心来源。
        for t in self.mcp_tools {
            registry.register(t);
        }

        // 审批者：交互式（桌面壳，挂起等前端）或用配置的 approver（CLI 自动）。
        let interactive = if self.interactive_approval {
            Some(Arc::new(crate::approval::InteractiveApprover::default()))
        } else {
            None
        };
        let approver: Arc<dyn Approver> = match &interactive {
            Some(a) => a.clone(),
            None => self.approver,
        };

        // B2 模型选择器：把选定模型包进可热换的 ModelSlot（Agent 持 wrapper，app 经句柄换实现）。
        let model_slot = crate::ModelSlot::new(self.model);
        let model_config_dir = self.data_dir.clone();

        let agent = Agent::builder()
            .model(Arc::new(model_slot.clone()))
            .tools(registry)
            .guards(guards)
            .approver(approver)
            .policy(policy)
            .build()?;

        let agent = Arc::new(agent);
        sub_handle.attach(&agent); // 注入弱引用，task 工具据此跑子回合

        let store = FileSessionStore::new(self.data_dir)?;
        let mut app = AgentApp::new(agent, Arc::new(store))
            .with_token_store(token_store)
            .with_plugins(plugin_summaries)
            .with_plugins_dir(plugins_dir)
            .with_plugin_market(plugin_market)
            .with_model(model_slot, model_config_dir);
        if let Some(a) = interactive {
            app = app.with_approver(a);
        }
        if let Some(sys) = self.system {
            app = app.with_default_system(sys);
        }
        if let Some(cr) = connectors {
            app = app.with_connectors(cr);
        }
        if let Some(auth_cfg) = self.auth {
            app = app.with_auth(Arc::new(AuthProvider::new(auth_cfg)));
        }
        // U13：把 PEP + 共享 identity 交给 app——登录后按真实用户角色重热 PDP 判定（授权门接地）。
        if let Some((pep, identity)) = data_auth_wire {
            app = app.with_data_auth_identity(pep, identity);
        }
        if let Some(base) = self.user_config_base {
            app = app.with_user_config_base(base);
        }
        Ok(app)
    }
}

/// 默认系统提示：把智能体框定为「办公助手」，并交代工作区与工具约定（减少模型试错）。
pub fn default_office_system_prompt() -> String {
    "你是 cmx 企业桌面办公智能体，帮助用户处理日常办公与文件工作。\n\
     \n\
     工作区（沙箱根）：你的所有文件操作都在当前工作区内进行。**文件路径用相对工作根的相对路径即可**\
     （例如直接写 `notes.md`、`docs/plan.md`），工具会自动挂到工作区根下；不要臆造绝对路径。\n\
     \n\
     可用能力：\n\
     - 文件：fs_read 读文件、fs_write 写/建文件、fs_edit 精确改、apply_patch 打补丁、glob 找文件、grep 搜内容、repo_map 看目录结构。\n\
     - 执行：bash 跑命令、run_tests 跑测试、git 版本控制（都在工作区内）。\n\
     - 联网：web_search 搜索、web_fetch 抓网页取正文（查资料、读在线文档；优先用它们而不是 bash+curl）；\
     动态/JS 页面(SPA)用 browser_read 无头渲染后取正文、browser_screenshot 网页截图；\
     需点按/填表/搜索的交互页用 browser_do；无稳定选择器、必须看画面操作的用 computer_use(视觉回环)。\n\
     - 计划：update_plan 登记多步任务清单（复杂任务先列计划再逐步执行）。\n\
     - 企业引擎（如可用）：flow 流程、onto 本体、report 报表等连接器工具。**做企业读写前先用 enterprise_context 拉域模型接地**（知道有哪些对象/动作/流程再动手，别猜 key）。\n\
     \n\
     工作原则：\n\
     1. 能用工具就用工具，别只是描述；动手完成用户的实际诉求。\n\
     2. 多步任务先用 update_plan 列清单，再逐步推进。\n\
     3. 改动文件后自我校验（读回确认 / 跑测试）。\n\
     4. 高危或不确定的操作先说明并征询，安全第一。\n\
     5. 用简洁中文回复，结论在前。"
        .to_string()
}
