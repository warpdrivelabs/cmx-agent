//! 守卫管道（方案图 7：五层护栏）。每次工具调用在 **pre-execute → execute（放行后由内核跑）→
//! post-execute** 三相穿过一串 [`Guard`]。对齐 dsh 守卫管道 + 《ERP-AI 五层护栏》。
//!
//! 五层落点：
//! ① 权限接地  [`AuthGuard`]（pre）——接 cmx-data-auth PDP/PEP（M0 用注入的判定闭包）。
//! ② 确定性校验（pre）——工具 inputSchema 校验 + CmxErrCode（M0 预留，见 tools crate）。
//! ③ 人在环    [`ApprovalGuard`]（pre）——高风险转 [`crate::agent::Approver`]。
//! ④ 全量审计（贯穿）——由 [`crate::session::Session`] 把每条裁决落日志实现，非单独 Guard。
//! ⑤ 高危审批  [`HighRiskGuard`]（pre）——高危工具必须经审批，不能自动放行。
//!
//! 失败即闭合（fail-closed）：任一 Guard 返回 Deny，调用即被拒，绝不"出错就放行"。

use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use crate::tool::{Approval, ToolCall, ToolSpec};

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
    /// 主体身份（M0：租户/用户/角色，接 IAM 后扩展）。
    pub subject: &'a Subject,
    /// 工作目录根（相对路径解析基准）：[`WorkspaceWriteGuard`] 据此判定写入是否越出工作目录。
    /// 不是安全围栏——沙箱已移除，这里只用于审批分级。
    pub workspace_roots: &'a [PathBuf],
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
                        "用户「{}」缺少权限「{}」",
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
                reason: format!("工具「{}」需要人工审批", ctx.spec.name),
            },
        }
    }
}

/// ⑤ 高危审批：标注 `high_risk` 的工具总是需要审批，即使审批标注为 Never。
pub struct HighRiskGuard;

impl Guard for HighRiskGuard {
    fn name(&self) -> &str {
        "high_risk"
    }

    fn phases(&self) -> &[GuardPhase] {
        &[GuardPhase::PreExecute]
    }

    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
        if ctx.spec.guard.high_risk {
            GuardDecision::NeedApproval {
                reason: format!("工具「{}」属于高危操作，需要人工审批", ctx.spec.name),
            }
        } else {
            GuardDecision::Allow
        }
    }
}

/// 词法归一化路径：移除 `.`，按 `..` 回退。不触碰文件系统（core 无 IO 不变量）。
fn normalize_lexical(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 目标路径是否越出所有工作目录根（词法判定，不解析符号链接——审批分级用，非安全围栏）。
/// 绝对路径按原样判定；相对路径按第一个根拼接；无根时视为不可校验（越界，从严要求审批）。
fn escapes_workspace(path: &str, roots: &[PathBuf]) -> bool {
    let raw = PathBuf::from(path);
    let candidate = if raw.is_absolute() {
        normalize_lexical(&raw)
    } else {
        match roots.first() {
            Some(root) => normalize_lexical(&root.join(&raw)),
            None => return true,
        }
    };
    !roots.iter().any(|r| candidate.starts_with(normalize_lexical(r)))
}

/// 工作目录写入审批分级（沙箱移除后的确定性缺口修复）：
/// - `write_path_args` 非空：逐字段取路径，任一目标越出工作目录 → 需审批（目录内放行）。
/// - `approval_arg_values`：入参指定字段命中取值集合（如 git 写子命令）→ 需审批。
///
/// 装配在 [`ApprovalGuard`] 之前；只把「越界写 / 写命令」从 Allow 升级为 NeedApproval，
/// 不放宽任何静态 `requires_approval`。
pub struct WorkspaceWriteGuard;

impl Guard for WorkspaceWriteGuard {
    fn name(&self) -> &str {
        "workspace_write"
    }

    fn phases(&self) -> &[GuardPhase] {
        &[GuardPhase::PreExecute]
    }

    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
        if let Some((field, values)) = &ctx.spec.guard.approval_arg_values
            && let Some(v) = ctx.call.input.get(field).and_then(|v| v.as_str())
            && values.iter().any(|x| x == v)
        {
            return GuardDecision::NeedApproval {
                reason: format!("工具「{}」的「{v}」为写操作，需要人工审批", ctx.spec.name),
            };
        }
        for field in &ctx.spec.guard.write_path_args {
            if let Some(path) = ctx.call.input.get(field).and_then(|v| v.as_str())
                && escapes_workspace(path, ctx.workspace_roots)
            {
                return GuardDecision::NeedApproval {
                    reason: format!(
                        "工具「{}」写入工作目录之外的路径「{path}」，需要人工审批",
                        ctx.spec.name
                    ),
                };
            }
        }
        GuardDecision::Allow
    }
}

/// 计划模式白名单（方案 §7.2 定稿；23 名逐一核对注册名）。**默认拒绝 + 显式白名单**：
/// 白名单可拦住 MCP/git/shell/run_tests/连接器写侧/fs_write 及一切后装工具，
/// 新工具默认拒绝（fail-closed），不依赖各工具的副作用标注。
pub const PLAN_READ_TOOLS: &[&str] = &[
    // 本地只读
    "fs_read",
    "grep",
    "glob",
    "repo_map",
    "lsp",
    "data_describe",
    "doc_read",
    // 联网只读（调研需要）
    "web_fetch",
    "web_search",
    "browser_read",
    // 控制面（user_interactive 工具先过守卫管道再进内核特判——漏列会在计划模式里
    // 连提问与退出批准一并拦死；task 放行的安全性 = 子回合 task-local 继承只读，§7.5）
    "update_plan",
    "ask_user",
    "exit_plan",
    "task",
    // 只读连接器与插件面
    "enterprise_context",
    "flow_list_definitions",
    "onto_list_object_types",
    "report_list_reports",
    "plugin_list",
    "plugin_marketplace",
    // 演示工具（纯计算）
    "echo",
    "clock",
    "add",
];

/// 计划模式守卫（阶段二）：[`super::agent::TURN_PLAN_MODE`] 活开关开启时，工具名不在
/// [`PLAN_READ_TOOLS`] 一律拒绝（§7.2）。装配在 HighRiskGuard / ApprovalGuard 之前；
/// 不受审批策略影响，无人值守 Auto 也不能绕过计划模式。
pub struct PlanModeGuard;

impl Guard for PlanModeGuard {
    fn name(&self) -> &str {
        "plan_mode"
    }

    fn phases(&self) -> &[GuardPhase] {
        &[GuardPhase::PreExecute]
    }

    fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
        if !super::agent::plan_mode_active() {
            return GuardDecision::Allow;
        }
        if PLAN_READ_TOOLS.contains(&ctx.spec.name.as_str()) {
            return GuardDecision::Allow;
        }
        GuardDecision::deny(format!(
            "计划模式：工具「{}」被拒（只读档）。完成调研后调用 exit_plan 提交计划请求用户批准",
            ctx.spec.name
        ))
    }
}
