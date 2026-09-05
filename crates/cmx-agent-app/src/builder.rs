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
        let mut guards = GuardPipeline::new();
        guards
            .add(Arc::new(AuthGuard::allow_all())) // M1 占位；M3 接 cmx-data-auth
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
        let connectors = self.connectors.map(|cfg| {
            let cr = ConnectorRegistry::new(cfg);
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

        let agent = Agent::builder()
            .model(self.model)
            .tools(registry)
            .guards(guards)
            .approver(approver)
            .policy(policy)
            .build()?;

        let agent = Arc::new(agent);
        sub_handle.attach(&agent); // 注入弱引用，task 工具据此跑子回合

        let store = FileSessionStore::new(self.data_dir)?;
        let mut app = AgentApp::new(agent, Arc::new(store));
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
     - 计划：update_plan 登记多步任务清单（复杂任务先列计划再逐步执行）。\n\
     - 企业引擎（如可用）：flow 流程、onto 本体、report 报表等连接器工具。\n\
     \n\
     工作原则：\n\
     1. 能用工具就用工具，别只是描述；动手完成用户的实际诉求。\n\
     2. 多步任务先用 update_plan 列清单，再逐步推进。\n\
     3. 改动文件后自我校验（读回确认 / 跑测试）。\n\
     4. 高危或不确定的操作先说明并征询，安全第一。\n\
     5. 用简洁中文回复，结论在前。"
        .to_string()
}
