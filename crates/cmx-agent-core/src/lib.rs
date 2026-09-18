//! cmx-agent 内核（形/核/体之「核」）。
//!
//! 架构对齐方案 docs/20260902_WorkBuddy复刻方案：借 codex-rs 的回合循环/工具路由/上下文观念 +
//! dsh 的工具注册表/append-only 会话日志/pre-execute-post 守卫管道 + 不变量「Model-visible means logged」。
//!
//! 该 crate 与平台/框架解耦：不依赖 DB / Web / cmx-container 基础设施，便于快速、确定性地测试。
//! 真实模型、真实沙箱、cmx-* 工具、持久化都以 trait 注入，在上层 crate 提供实现。

pub mod agent;
pub mod agents;
pub mod compaction;
pub mod error;
pub mod event;
pub mod exit_plan;
pub mod guard;
pub mod model;
pub mod question;
pub mod session;
pub mod token;
pub mod tool;

pub use agent::{
    call_summary, deactivate_plan_mode, plan_mode_active, Agent, AgentBuilder, ApprovalPolicy,
    Approver, AutoApprover, Policy, TurnCancel, TurnOutcome, TurnPolicyOverride, SUBAGENT_TURN,
    TURN_PLAN_MODE, TURN_POLICY_OVERRIDE, TURN_SUBJECT,
};
pub use agents::{builtin_specs, AgentSpec, ModelResolver, ToolSelection, EXPLORE, GENERAL_PURPOSE};
pub use error::{AgentError, AgentResult};
pub use event::{CompactionReason, EventKind, EventSink, SessionEvent, SessionLog, StopReason};
pub use exit_plan::{ExitPlanTool, EXIT_PLAN_TOOL_NAME};
pub use guard::{
    ApprovalGuard, AuthGuard, Guard, GuardCtx, GuardDecision, GuardPhase, GuardPipeline,
    HighRiskGuard, PlanModeGuard, PLAN_READ_TOOLS, SandboxGuard, SandboxMode, Subject,
};
pub use model::{
    MockModel, ModelContext, ModelError, ModelMessage, ModelResponse, ModelSeam, ModelUsage,
    TurnObserver,
};
pub use question::{
    normalize_ask_input, AskOption, AskQuestion, AskUserTool, PendingQuestionInfo, QuestionOutcome,
    QuestionService, QUESTION_TOOL_NAME,
};
pub use session::Session;
pub use tool::{
    Approval, GuardHints, Tool, ToolCall, ToolCtx, ToolError, ToolRegistry, ToolResult, ToolSpec,
};
