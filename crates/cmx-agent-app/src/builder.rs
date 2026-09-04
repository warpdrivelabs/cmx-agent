//! [`DesktopAppBuilder`]：一键装配桌面壳后端。给定工作目录 + 模型缝，产出一个 [`AgentApp`]，
//! 其沙箱根设为工作目录（本地文件工具只能碰工作区）、挂上全部内置工具与五层守卫。
//!
//! 真实模型缝（HTTP 客户端）在此注入即可；桌面壳/CLI/Headless 前门均复用同一装配。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_connectors::{ConnectorConfig, ConnectorRegistry};
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
            system: Some(
                "你是 cmx 企业桌面智能体。安全第一：能用工具就用工具，不确定就问。".to_string(),
            ),
            connectors: None,
        }
    }

    pub fn approver(mut self, a: Arc<dyn Approver>) -> Self {
        self.approver = a;
        self
    }

    /// 启用 cmx 连接器（flow/onto/report）。传 `ConnectorConfig::default()` 即用本机标准端口。
    pub fn connectors(mut self, cfg: ConnectorConfig) -> Self {
        self.connectors = Some(cfg);
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

        // 工具注册表：内置工具 + （可选）连接器工具。
        let mut registry = default_registry();
        let connectors = self.connectors.map(|cfg| {
            let cr = ConnectorRegistry::new(cfg);
            cr.register_into(&mut registry);
            Arc::new(cr)
        });

        let agent = Agent::builder()
            .model(self.model)
            .tools(registry)
            .guards(guards)
            .approver(self.approver)
            .policy(policy)
            .build()?;

        let store = FileSessionStore::new(self.data_dir)?;
        let mut app = AgentApp::new(Arc::new(agent), Arc::new(store));
        if let Some(sys) = self.system {
            app = app.with_default_system(sys);
        }
        if let Some(cr) = connectors {
            app = app.with_connectors(cr);
        }
        Ok(app)
    }
}
