//! 守卫管道（方案图 7：五层护栏）。每次工具调用在 **pre-execute → execute（放行后由内核跑）→
//! post-execute** 三相穿过一串 [`Guard`]。对齐 dsh 守卫管道 + 《ERP-AI 五层护栏》。
//!
//! 五层落点：
//! ① 权限接地  [`AuthGuard`]（pre）——接 cmx-data-auth PDP/PEP（M0 用注入的判定闭包）。
//! ② 确定性校验（pre）——工具 inputSchema 校验 + CmxErrCode（M0 预留，见 tools crate）。
//! ③ 人在环    [`ApprovalGuard`]（pre）——高风险转 [`crate::agent::Approver`]。
//! ④ 全量审计（贯穿）——由 [`crate::session::Session`] 把每条裁决落日志实现，非单独 Guard。
//! ⑤ 沙箱执行  [`HighRiskGuard`]（pre）+ 工具在 [`SandboxMode`] 下执行。
//!
//! 失败即闭合（fail-closed）：任一 Guard 返回 Deny，调用即被拒，绝不"出错就放行"。

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::tool::{Approval, ToolCall, ToolSpec};

/// 沙箱模式（两旋钮之「能力」——能做什么）。对齐 codex `sandbox_mode`。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SandboxMode {
    /// 只读：禁止写文件/网络/高危。
    #[default]
    ReadOnly,
    /// 工作区可写：可读写 allowed_roots，禁网络与高危（日常推荐）。
    WorkspaceWrite,
    /// 完全访问：放行高危（≈ --yolo，仅自动化+显式最小权限时用）。
    DangerFullAccess,
}

impl SandboxMode {
    pub fn allows_high_risk(self) -> bool {
        matches!(self, SandboxMode::DangerFullAccess)
    }

    pub fn allows_write(self) -> bool {
        matches!(
            self,
            SandboxMode::WorkspaceWrite | SandboxMode::DangerFullAccess
        )
    }
}

/// 守卫相位。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum GuardPhase {
    /// 执行前（权限/校验/审批/高危拦截都在此）。
    PreExecute,
    /// 执行后（审计/脱敏/副作用登记）。
    PostExecute,
}

/// 守卫裁决。`flatten` 进事件时字段为 `decision` + 可选 `reason`。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum GuardDecision {
    /// 放行。
    Allow,
    /// 拒绝（fail-closed）。
    Deny { reason: String },
    /// 需人工审批后方可继续（由内核在 pre 相调 Approver 解决）。
    NeedApproval { reason: String },
}

impl GuardDecision {
    pub fn deny(reason: impl Into<String>) -> Self {
        GuardDecision::Deny {
            reason: reason.into(),
        }
    }

    pub fn is_allow(&self) -> bool {
        matches!(self, GuardDecision::Allow)
    }
}

/// 守卫看到的上下文。
pub struct GuardCtx<'a> {
    pub phase: GuardPhase,
    pub call: &'a ToolCall,
    pub spec: &'a ToolSpec,
    pub sandbox: SandboxMode,
    /// 主体身份（M0：租户/用户/角色，接 IAM 后扩展）。
    pub subject: &'a Subject,
    /// post 相可见的工具结果（pre 相为 None）。
    pub result: Option<&'a crate::tool::ToolResult>,
}

/// 主体（谁在调用）。M0 极简；后续对齐 cmx-data-auth Subject（tenant 须有默认值）。
#[derive(Debug, Clone, Default)]
pub struct Subject {
    pub tenant: String,
    pub user: String,
    pub roles: Vec<String>,
}

impl Subject {
    pub fn new(user: impl Into<String>) -> Self {
        Self {
            tenant: "default".into(),
            user: user.into(),
            roles: vec![],
        }
    }
}

/// 单个守卫。
pub trait Guard: Send + Sync {
    fn name(&self) -> &str;
    /// 该守卫参与哪些相。
    fn phases(&self) -> &[GuardPhase];
    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision;
}

/// 守卫管道：按注册序在某一相逐个执行；遇第一个非 Allow 即短路返回（fail-closed）。
#[derive(Default, Clone)]
pub struct GuardPipeline {
    guards: Vec<Arc<dyn Guard>>,
}

impl GuardPipeline {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, g: Arc<dyn Guard>) -> &mut Self {
        self.guards.push(g);
        self
    }

    /// 在给定相运行管道。返回 (守卫名, 裁决)；全 Allow 时守卫名为空串。
    pub fn run(&self, ctx: &GuardCtx<'_>) -> (String, GuardDecision) {
        for g in &self.guards {
            if !g.phases().contains(&ctx.phase) {
                continue;
            }
            let d = g.check(ctx);
            if !d.is_allow() {
                return (g.name().to_string(), d);
            }
        }
        (String::new(), GuardDecision::Allow)
    }

    pub fn len(&self) -> usize {
        self.guards.len()
    }

    pub fn is_empty(&self) -> bool {
        self.guards.is_empty()
    }
}

// ————————————————————— 内置守卫 —————————————————————

/// ① 权限接地：对声明了 `requires_auth` 的工具，用注入的判定函数决定放行/拒绝。
/// 真实实现接 cmx-data-auth PDP/PEP；M0 用闭包便于测试各种策略。
pub struct AuthGuard {
    #[allow(clippy::type_complexity)]
    decide: Box<dyn Fn(&Subject, &str) -> bool + Send + Sync>,
}

impl AuthGuard {
    /// `decide(subject, permission) -> allowed`。
    pub fn new(decide: impl Fn(&Subject, &str) -> bool + Send + Sync + 'static) -> Self {
        Self {
            decide: Box::new(decide),
        }
    }

    /// 放行一切（无鉴权环境的显式占位）。
    pub fn allow_all() -> Self {
        Self::new(|_, _| true)
    }
}

impl Guard for AuthGuard {
    fn name(&self) -> &str {
        "auth"
    }

    fn phases(&self) -> &[GuardPhase] {
        &[GuardPhase::PreExecute]
    }

    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
        match &ctx.spec.guard.requires_auth {
            None => GuardDecision::Allow,
            Some(perm) => {
                if (self.decide)(ctx.subject, perm) {
                    GuardDecision::Allow
                } else {
                    GuardDecision::deny(format!(
                        "subject '{}' lacks permission '{}'",
                        ctx.subject.user, perm
                    ))
                }
            }
        }
    }
}

/// ③ 人在环：把 `requires_approval` 翻译为 `NeedApproval`（由内核在 pre 相解决）。
pub struct ApprovalGuard;

impl Guard for ApprovalGuard {
    fn name(&self) -> &str {
        "approval"
    }

    fn phases(&self) -> &[GuardPhase] {
        &[GuardPhase::PreExecute]
    }

    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
        match ctx.spec.guard.requires_approval {
            Approval::Never => GuardDecision::Allow,
            Approval::Conditional | Approval::Always => GuardDecision::NeedApproval {
                reason: format!("tool '{}' requires human approval", ctx.spec.name),
            },
        }
    }
}

/// ⑤ 高危拦截：标注 `high_risk` 的工具仅在 danger-full-access 沙箱放行，否则拒绝。
pub struct HighRiskGuard;

impl Guard for HighRiskGuard {
    fn name(&self) -> &str {
        "high_risk"
    }

    fn phases(&self) -> &[GuardPhase] {
        &[GuardPhase::PreExecute]
    }

    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
        if ctx.spec.guard.high_risk && !ctx.sandbox.allows_high_risk() {
            GuardDecision::deny(format!(
                "tool '{}' is high-risk and blocked under sandbox {:?}",
                ctx.spec.name, ctx.sandbox
            ))
        } else {
            GuardDecision::Allow
        }
    }
}
