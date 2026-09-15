//! 回合循环（方案图 4）。一个 **Turn** = 多个 **Step**；每个 Step：
//! ① 模型推理 → ② 工具路由 → ③ 护栏前置(pre) → ④ 审批闸门 → ⑤ 沙箱执行 → ⑥ 观察回灌(post)。
//! 回合内循环直到：模型不再要工具（Completed）/ 达到 max_steps（MaxSteps）/ 被停机（Stopped）。
//!
//! 不变量落地：每一步"模型可见"的产物（模型输出、工具调用、工具结果）都先 append 进日志，
//! 再据日志重建下一步上下文——见 [`crate::session::Session::model_context`]。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{AgentError, AgentResult};
use crate::event::{EventKind, StopReason};
use crate::guard::{GuardCtx, GuardDecision, GuardPhase, GuardPipeline, SandboxMode, Subject};
use crate::model::{ModelResponse, ModelSeam};
use crate::question::{normalize_ask_input, QuestionOutcome, QuestionService};
use crate::session::Session;
use crate::tool::{Approval, Tool, ToolCall, ToolCtx, ToolRegistry, ToolResult, ToolSpec};
use serde::{Deserialize, Serialize};

tokio::task_local! {
    /// 回合主体 task-local：`Some(主体)` = 本回合以该身份过守卫（IM 绑定场景）。
    /// 由 [`Agent::run_turn_observed_as_cancellable`] 在回合外层置位；`task` 子智能体工具
    /// 经 `TURN_SUBJECT.try_with(|s| s.clone()).ok().flatten()` 读取（作用域外为 None），
    /// 并显式传给子回合——子智能体不再回落桌面 `policy.subject` 过守卫。
    pub static TURN_SUBJECT: Option<Subject>;
    /// 子智能体回合标记：task 工具跑子回合时置 true（作用域外为 false）。交互提问工具
    /// （ask_user）据此门控——子回合不可取消（TurnCancel 旗标在子回合内不可达）、事件不
    /// 实时外送、`cancel_session(父id)` 够不到子会话的 pending，挂起即失控，直接降级 dismissed。
    pub static SUBAGENT_TURN: bool;
}

/// 审批策略（两旋钮之「许可」——何时问你）。对齐 codex `approval_policy`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApprovalPolicy {
    /// 从不打断：把所有 NeedApproval 视为拒绝（无人值守自动化用，最安全的自动档）。
    Never,
    /// 按需：仅在守卫要求时询问（日常推荐）。
    #[default]
    OnRequest,
    /// 逢写必问：任何非幂等工具都要审批（谨慎档）。
    UnlessTrusted,
}

/// 回合级权限档覆盖：本回合的沙箱/审批两旋钮以覆盖为准（其余 Policy 字段照抄全局档）。
/// 只作用于显式 scope 的那个回合任务树（含 `task` 子智能体——子回合在同任务树上继承），
/// **不写共享全局档**，桌面并发回合互不影响。IM 无人值守「默认全权」即经它实现。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TurnPolicyOverride {
    pub sandbox: SandboxMode,
    pub approval: ApprovalPolicy,
}

impl TurnPolicyOverride {
    /// 无人值守全权档（IM 遥控默认）：沙箱完全放行 + 从不打断。条件级人审被
    /// [`crate::guard::ApprovalGuard`] 的 danger 豁免跳过（不弹卡）；Always 级硬人审被
    /// `policy:never` 直接拒绝——宁拒不挂，无人值守没有「等 300 秒超时」的受害者。
    pub const FULL_ACCESS: Self = Self {
        sandbox: SandboxMode::DangerFullAccess,
        approval: ApprovalPolicy::Never,
    };
}

tokio::task_local! {
    /// 回合级权限档覆盖（见 [`TurnPolicyOverride`]）。仅在显式 scope 的回合任务树内可见；
    /// 回合流程内的守卫/审批判定经 [`Agent::effective_policy`] 读取，作用域外回落全局档。
    pub static TURN_POLICY_OVERRIDE: TurnPolicyOverride;
    /// 计划模式活开关（方案 20260914 §7.1，阶段二）：`Some(flag)` = 本回合任务树处于计划模式。
    /// 守卫（[`crate::guard::PlanModeGuard`]）每批工具调用现读；exit_plan 批准后同回合置 false
    /// 立即放行写。task-local 随 future 树下传 → task 子回合自动继承且不可绕过；作用域外 =
    /// 非计划模式。app 层回合开始插入、收尾摘除并对账回写 meta。
    pub static TURN_PLAN_MODE: Arc<AtomicBool>;
}

/// 当前回合任务树是否处于计划模式（守卫/内核托管路径读取；作用域外 = false）。
pub fn plan_mode_active() -> bool {
    TURN_PLAN_MODE
        .try_with(|f| f.load(Ordering::Relaxed))
        .unwrap_or(false)
}

/// 退出计划模式（exit_plan 批准路径调用）：同回合下一批工具调用立即放行写。
/// 返回是否在计划模式任务树内（false = 本就不在，调用方兜底）。
pub fn deactivate_plan_mode() -> bool {
    TURN_PLAN_MODE
        .try_with(|f| {
            f.store(false, Ordering::Relaxed);
            true
        })
        .unwrap_or(false)
}

/// 人在环审批者（③ 人在环 的解决方；真实实现接**交互式前端审批卡片** / cmx-flow 审批 / IM 二次确认）。
/// `resolve` 为 async：交互式实现可在此**挂起等待**用户在前端点击「允许/拒绝」后再返回。
#[async_trait::async_trait]
pub trait Approver: Send + Sync {
    /// 返回 (是否批准, 审批人标识)。
    async fn resolve(&self, call: &ToolCall, reason: &str) -> (bool, String);

    /// 该会话是否已被授予「本对话全部允许」——若是，内核跳过审批直接放行（不再弹审批卡）。默认 false。
    /// 交互式审批者据此实现「本对话全部允许」：用户点一次后，本会话后续需审批的工具全部自动放行。
    fn is_preapproved(&self, _session_id: &str) -> bool {
        false
    }

    /// 按会话等待审批，默认退化为不区分会话的 [`Approver::resolve`]。
    async fn resolve_for_session(
        &self,
        _session_id: &str,
        call: &ToolCall,
        reason: &str,
    ) -> (bool, String) {
        self.resolve(call, reason).await
    }

    /// 按会话等待审批并带回**用户附言**（ZCode 式「告诉模型接下来应该怎么做」）：
    /// 拒绝时附言回灌给模型帮其自愈；三元组为 (是否批准, 审批人标识, 附言)。
    /// 默认退化为不带附言的 [`Approver::resolve_for_session`]——旧实现零波及。
    async fn resolve_for_session_with_note(
        &self,
        session_id: &str,
        call: &ToolCall,
        reason: &str,
    ) -> (bool, String, Option<String>) {
        let (ok, by) = self.resolve_for_session(session_id, call, reason).await;
        (ok, by, None)
    }

    /// 中断会话时拒绝其全部待决审批，避免回合卡在审批等待。默认无待决可撤。
    fn cancel_session(&self, _session_id: &str) {}
}

/// 自动审批者（测试/自动化）：按固定答案回应。
pub struct AutoApprover {
    approve: bool,
    by: String,
}

impl AutoApprover {
    pub fn approve() -> Self {
        Self {
            approve: true,
            by: "auto".into(),
        }
    }

    pub fn reject() -> Self {
        Self {
            approve: false,
            by: "auto".into(),
        }
    }
}

#[async_trait::async_trait]
impl Approver for AutoApprover {
    async fn resolve(&self, _call: &ToolCall, _reason: &str) -> (bool, String) {
        (self.approve, self.by.clone())
    }
}

/// 内核策略：两旋钮 + 步数上限 + 允许的文件根 + 主体。
#[derive(Debug, Clone)]
pub struct Policy {
    pub sandbox: SandboxMode,
    pub approval: ApprovalPolicy,
    /// 单回合最大 Step 数（防失控循环）。
    pub max_steps: usize,
    pub allowed_roots: Vec<PathBuf>,
    pub subject: Subject,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            sandbox: SandboxMode::WorkspaceWrite,
            approval: ApprovalPolicy::OnRequest,
            max_steps: 16,
            allowed_roots: vec![],
            subject: Subject::new("anon"),
        }
    }
}

/// 一个回合的结果摘要。
#[derive(Debug, Clone)]
pub struct TurnOutcome {
    pub turn: u64,
    pub reason: StopReason,
    pub steps: usize,
    /// 最后一条模型文本（便于前门直接展示）。
    pub final_text: Option<String>,
}

/// 单回合中断旗标。由应用层注册到活动回合表，取消时置位；
/// 内核在模型步边界检查，模型流读取器可提前停止 HTTP 消费。
#[derive(Debug, Clone, Default)]
pub struct TurnCancel(Arc<AtomicBool>);

impl TurnCancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// 智能体内核。
pub struct Agent {
    model: Arc<dyn ModelSeam>,
    tools: ToolRegistry,
    guards: GuardPipeline,
    approver: Arc<dyn Approver>,
    /// 交互提问服务（ask_user 的挂起-裁决端）。装配侧决定真服务（桌面双壳）或降级实例
    /// （CLI/e2e）——内核只认 `enabled()` 门控；非交互来源（IM/无人值守/子回合）在 pre 相
    /// 直接回灌 dismissed，不发生挂起。
    questions: Arc<QuestionService>,
    /// 策略（两旋钮等）。RwLock：`Agent` 经 Arc 共享，两旋钮须运行时可切（前门 `set_policy`）；
    /// 回合内按快照读取（clone），写入方仅 set_policy。
    policy: std::sync::RwLock<Policy>,
}

impl Agent {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    /// 当前策略快照（clone；Policy 小，回合内每步取一次开销可忽略）。
    pub fn policy(&self) -> Policy {
        self.policy.read().expect("policy lock poisoned").clone()
    }

    /// 守卫管道引用（子智能体派生装配用：`Agent` 字段私有，方案 §11.1 补的访问器之一）。
    pub fn guards(&self) -> &GuardPipeline {
        &self.guards
    }

    /// 审批者句柄（子智能体派生装配用：审批卡照常弹父 UI，call_id 校验跨会话不误命中）。
    pub fn approver(&self) -> Arc<dyn Approver> {
        self.approver.clone()
    }

    /// 当前模型缝（子智能体派生装配兜底用；正常走 ModelResolver 解析）。
    pub fn model(&self) -> Arc<dyn ModelSeam> {
        self.model.clone()
    }

    /// 运行时替换策略（前门 set_policy 用；其余字段照抄当前值由调用方组装）。
    pub fn set_policy(&self, p: Policy) {
        *self.policy.write().expect("policy lock poisoned") = p;
    }

    /// 回合内生效的策略快照：任务树上挂了 [`TURN_POLICY_OVERRIDE`] 时，其 sandbox/approval
    /// 盖过全局档（IM 无人值守全权等回合级场景），其余字段照抄全局档。回合流程内的
    /// 守卫/审批判定一律经此取值；全局档直读只允许在回合外（UI 展示、set_policy）。
    fn effective_policy(&self) -> Policy {
        let mut p = self.policy();
        if let Ok(ov) = TURN_POLICY_OVERRIDE.try_with(|ov| *ov) {
            p.sandbox = ov.sandbox;
            p.approval = ov.approval;
        }
        p
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
    }

    /// 交互提问服务句柄（app 层前门命令 answer/dismiss/list 与内核共享同一 Arc）。
    pub fn questions(&self) -> Arc<QuestionService> {
        self.questions.clone()
    }

    /// 跑一个回合：喂入用户输入，循环 Step 直至停机。全过程写 `session.log`。
    ///
    /// 取 `&self`：一个共享 `Agent` 可服务多个会话。回合号从会话日志派生（历史 `TurnEnded` 计数 + 1），
    /// 故重启后从持久化日志恢复的会话能续上正确的回合号——桌面壳多会话/跨重启的地基。
    pub async fn run_turn(
        &self,
        session: &mut Session,
        user_input: &str,
    ) -> AgentResult<TurnOutcome> {
        self.run_turn_observed(session, user_input, None).await
    }

    /// 以指定主体（谁在驱动本回合）跑一个回合——IM 绑定场景：飞书消息按**绑定用户**的身份过守卫
    /// （GuardCtx.subject 用传入值），与桌面登录用户（policy.subject）互不干扰、并发无竞态。
    /// `subject=None` 等价 [`Self::run_turn`]（回落 policy.subject）。
    pub async fn run_turn_as(
        &self,
        session: &mut Session,
        user_input: &str,
        subject: &crate::guard::Subject,
    ) -> AgentResult<TurnOutcome> {
        self.run_turn_observed_as(session, user_input, None, Some(subject)).await
    }

    /// 流式回合：同 [`Self::run_turn`]，但模型文字增量经 `observer` 实时回调（打字机效果）。
    /// deltas 不进日志；最终完整文本仍以 `ModelMessage` 事件落库。
    pub async fn run_turn_observed(
        &self,
        session: &mut Session,
        user_input: &str,
        observer: Option<&dyn crate::model::TurnObserver>,
    ) -> AgentResult<TurnOutcome> {
        self.run_turn_observed_as(session, user_input, observer, None).await
    }

    /// [`Self::run_turn_observed`] 的主体注入版：`turn_subject=Some` 时守卫按该主体判定
    /// （IM 绑定：绑定用户身份跑回合）；`None` 回落 `policy.subject`。
    pub async fn run_turn_observed_as(
        &self,
        session: &mut Session,
        user_input: &str,
        observer: Option<&dyn crate::model::TurnObserver>,
        turn_subject: Option<&crate::guard::Subject>,
    ) -> AgentResult<TurnOutcome> {
        self.run_turn_observed_as_cancellable(session, user_input, observer, turn_subject, None, None)
            .await
    }

    /// [`Self::run_turn_observed_as`] 的可中断版：不传旗标时行为完全一致。
    ///
    /// 回合主体经 [`TURN_SUBJECT`] task-local 随调用链下传：`task` 子智能体工具读取当前回合主体
    /// 并显式传给子回合——IM 绑定用户派发的子任务不再回落 `policy.subject`（桌面主体）过守卫。
    pub async fn run_turn_observed_as_cancellable(
        &self,
        session: &mut Session,
        user_input: &str,
        observer: Option<&dyn crate::model::TurnObserver>,
        turn_subject: Option<&crate::guard::Subject>,
        cancel: Option<&TurnCancel>,
        roots: Option<&[std::path::PathBuf]>,
    ) -> AgentResult<TurnOutcome> {
        self.run_turn_scoped(session, user_input, observer, turn_subject, cancel, roots, None)
            .await
    }

    /// TURN_SUBJECT 作用域 + 回合主循环的统一入口（各公开 wrapper 共用）；`deferred` 语义见
    /// [`Self::run_turn_observed_as_cancellable_with_policy_deferred`]。
    #[allow(clippy::too_many_arguments)]
    async fn run_turn_scoped(
        &self,
        session: &mut Session,
        user_input: &str,
        observer: Option<&dyn crate::model::TurnObserver>,
        turn_subject: Option<&crate::guard::Subject>,
        cancel: Option<&TurnCancel>,
        roots: Option<&[std::path::PathBuf]>,
        deferred: Option<&(dyn Fn() -> Option<String> + Send + Sync)>,
    ) -> AgentResult<TurnOutcome> {
        let subject = turn_subject.cloned();
        TURN_SUBJECT
            .scope(subject, self.run_turn_inner(session, user_input, observer, turn_subject, cancel, roots, deferred))
            .await
    }

    /// [`Self::run_turn_observed_as_cancellable`] 的回合级权限档覆盖版：`Some(覆盖档)` 时
    /// 本回合（含 `task` 子智能体——同任务树继承 task-local）的沙箱/审批以覆盖为准，
    /// 不写全局档；`None` 与原方法完全一致。IM 无人值守「默认全权」由此实现——
    /// IM 桥驱动的回合全权执行，桌面回合仍走全局两旋钮。
    ///
    /// `plan`：计划模式活开关（§7.1）。`Some(flag)` 时整个回合 future 在
    /// [`TURN_PLAN_MODE`] 作用域内——守卫每批现读，exit_plan 批准后同回合放行写；
    /// task 子回合随 future 树自动继承且不可绕过。`None` = 非计划模式。
    // 与 run_turn_observed_as_cancellable 同形 + 2 个可选参数：展平签名比拆结构体更贴调用侧。
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn_observed_as_cancellable_with_policy(
        &self,
        session: &mut Session,
        user_input: &str,
        observer: Option<&dyn crate::model::TurnObserver>,
        turn_subject: Option<&crate::guard::Subject>,
        cancel: Option<&TurnCancel>,
        roots: Option<&[std::path::PathBuf]>,
        policy_override: Option<TurnPolicyOverride>,
        plan: Option<Arc<AtomicBool>>,
    ) -> AgentResult<TurnOutcome> {
        self.run_turn_observed_as_cancellable_with_policy_deferred(
            session, user_input, observer, turn_subject, cancel, roots, policy_override, plan, None,
        )
        .await
    }

    /// [`Self::run_turn_observed_as_cancellable_with_policy`] 的**折尾注入版**（方案
    /// 20260914 改造二）：后台子任务回执折进当前回合。`deferred` 在回合自然收口点
    /// （模型不再要工具、即将 Completed）轮询一次——返回 `Some(text)` 则把 text 作为
    /// UserMessage 落日志并续跑一轮模型调用，让模型在同一回复末尾转述回执（ZCode 图二
    /// 形态）；返回 None 行为与原方法逐字节一致。注入迭代照常计入 steps、受 max_steps
    /// 约束（触顶未吸收的回执由调用方收尾兜底 drain 接管，不丢）。
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn_observed_as_cancellable_with_policy_deferred(
        &self,
        session: &mut Session,
        user_input: &str,
        observer: Option<&dyn crate::model::TurnObserver>,
        turn_subject: Option<&crate::guard::Subject>,
        cancel: Option<&TurnCancel>,
        roots: Option<&[std::path::PathBuf]>,
        policy_override: Option<TurnPolicyOverride>,
        plan: Option<Arc<AtomicBool>>,
        deferred: Option<&(dyn Fn() -> Option<String> + Send + Sync)>,
    ) -> AgentResult<TurnOutcome> {
        let inner =
            self.run_turn_scoped(session, user_input, observer, turn_subject, cancel, roots, deferred);
        match (policy_override, plan) {
            (Some(ov), Some(flag)) => {
                TURN_POLICY_OVERRIDE.scope(ov, TURN_PLAN_MODE.scope(flag, inner)).await
            }
            (Some(ov), None) => TURN_POLICY_OVERRIDE.scope(ov, inner).await,
            (None, Some(flag)) => TURN_PLAN_MODE.scope(flag, inner).await,
            (None, None) => inner.await,
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_turn_inner(
        &self,
        session: &mut Session,
        user_input: &str,
        observer: Option<&dyn crate::model::TurnObserver>,
        turn_subject: Option<&Subject>,
        cancel: Option<&TurnCancel>,
        roots: Option<&[std::path::PathBuf]>,
        deferred: Option<&(dyn Fn() -> Option<String> + Send + Sync)>,
    ) -> AgentResult<TurnOutcome> {
        let turn = session.next_turn_no();
        session.log.append(EventKind::TurnStarted {
            turn,
            user_input: user_input.to_string(),
        });
        session.log.append(EventKind::UserMessage {
            text: user_input.to_string(),
        });

        let mut steps = 0usize;
        let mut final_text = None;
        // 「勿绕道」提示同回合只发全量一次：连续多工具被拦时重复全文只会膨胀上下文，后续拦截给短句重申。
        let mut detour_hinted = false;
        let reason = loop {
            if cancel.is_some_and(|c| c.is_cancelled()) {
                break StopReason::Stopped;
            }
            if steps >= self.effective_policy().max_steps {
                break StopReason::MaxSteps;
            }
            steps += 1;

            // ① 模型推理（上下文只从日志派生）。有 observer → 走流式，文字增量实时回调。
            let ctx = session.model_context(self.tools.specs());
            let model_result = match observer {
                Some(obs) => self.model.complete_streaming(&ctx, obs).await,
                None => self.model.complete(&ctx).await,
            };
            let resp: ModelResponse = match model_result {
                Ok(resp) => resp,
                Err(_e) if cancel.is_some_and(|c| c.is_cancelled()) => break StopReason::Stopped,
                Err(e) => return Err(AgentError::Model(e.0)),
            };

            // 思考过程可审计/可回放，但不进入 model_context。
            if let Some(text) = resp.reasoning.as_deref()
                && !text.trim().is_empty()
            {
                session
                    .log
                    .append(EventKind::Reasoning { text: text.to_string() });
            }
            // 记录模型输出（模型可见 → 必落日志）
            session.log.append(EventKind::ModelMessage {
                text: resp.text.clone(),
                tool_calls: resp.tool_calls.clone(),
            });
            if let Some(t) = &resp.text {
                final_text = Some(t.clone());
            }

            // 无工具调用 → 优先吸收延迟注入（后台子任务回执折尾，方案 20260914 改造二）：
            // 取到回执则作为 UserMessage 落日志并续跑一轮（模型在同一回复末尾转述）；
            // 无注入才真正收口。注入迭代照常计入 steps，循环顶部 max_steps 仍在约束。
            if !resp.wants_tools() {
                if let Some(text) = deferred.and_then(|f| f()) {
                    session.log.append(EventKind::UserMessage { text });
                    continue;
                }
                break StopReason::Completed;
            }

            if cancel.is_some_and(|c| c.is_cancelled()) {
                break StopReason::Stopped;
            }

            // ②–⑥ 处理工具调用：一步内的多个调用**并发执行**（真并行 fan-out）。
            // 前置(路由/守卫/审批)与结果回灌仍按序（借用 &mut session + 保持日志有序），
            // 只有工具体 invoke() 并发——子智能体/网络 I/O 型调用总耗时≈最慢者而非累加。
            self.handle_tool_calls_as(session, &resp.tool_calls, turn_subject, roots, cancel, &mut detour_hinted).await;

            if cancel.is_some_and(|c| c.is_cancelled()) {
                break StopReason::Stopped;
            }
        };

        session.log.append(EventKind::TurnEnded {
            turn,
            reason,
            steps,
        });
        Ok(TurnOutcome {
            turn,
            reason,
            steps,
            final_text,
        })
    }

    /// 处理一步内的**一批**工具调用：**三段式**兼顾并发、借用安全与日志保序。
    /// - 前置(串行, 需 `&mut session`)：ToolInvoked 落库 → 路由 → pre 守卫 → 审批闸门；
    ///   失败者(未知工具 / 拒绝 / 审批否)当场以 `ToolResult::err` 回灌（让模型自愈，不中止回合）。
    /// - 执行(**并发**)：所有通过前置的工具体 `invoke()` 用 `join_all` 并发跑——这一段不碰 session，
    ///   故子智能体(task)/网络型工具由此**真并行**，一步总耗时≈最慢者而非累加。
    /// - 回灌(串行, 需 `&mut session`)：按调用序逐个 post 守卫 + ToolResult 落库（保持日志有序）。
    ///
    /// 说明：`&mut Session` 无法被多个并发 future 共享，故只把「纯执行」并发化，
    /// 而把所有会话写(事件落库/审批)留在串行段——这是 Rust 借用规则下并发与不变量的正确切分。
    ///
    /// 守卫按 `subject` 判定：`turn_subject=Some`（IM 绑定：绑定用户身份过守卫，与桌面登录主体
    /// policy.subject 并发无竞态）；`None` 回落本批策略快照的 subject。
    async fn handle_tool_calls_as(
        &self,
        session: &mut Session,
        calls: &[ToolCall],
        turn_subject: Option<&crate::guard::Subject>,
        roots: Option<&[std::path::PathBuf]>,
        cancel: Option<&TurnCancel>,
        // 「勿绕道」全量提示是否已发（回合级去重：跨步骤只发一次全量，后续短句重申）。
        detour_hinted: &mut bool,
    ) {
        /// 通过前置、待并发执行的工具调用。
        struct Pending<'c> {
            call: &'c ToolCall,
            tool: Arc<dyn Tool>,
            spec: ToolSpec,
            /// 交互提问票据（user_interactive 且过门控时 Some）：执行段 take 走。
            question: Option<QuestionTicket>,
        }

        // 本批工具调用的策略快照（两旋钮运行时可切；一批内取一致值；回合级覆盖优先）。
        let policy = self.effective_policy();
        let subject = turn_subject.unwrap_or(&policy.subject);
        // —— ①–④ 前置(串行)：落库调用、路由、pre 守卫、审批 ——
        let mut pending: Vec<Pending<'_>> = Vec::new();
        for call in calls {
            // 取消旗标在批次内也生效：中断发生在本批第 N 个调用的审批等待时，
            // 第 N+1.. 个调用直接以取消回灌，不再弹下一张审批卡（旧实现继续走完本批，
            // 人走开后每张卡各挂 300s 超时）。
            if cancel.is_some_and(|c| c.is_cancelled()) {
                self.push_result(session, &call.id, ToolResult::err("cancelled by user"));
                continue;
            }
            session
                .log
                .append(EventKind::ToolInvoked { call: call.clone() });

            // 路由：工具不存在 → 错误结果回灌
            let Some(tool) = self.tools.get(&call.name) else {
                self.push_result(
                    session,
                    &call.id,
                    ToolResult::err(format!("unknown tool '{}'", call.name)),
                );
                continue;
            };
            let spec = tool.spec();

            // 护栏前置(pre)
            let (guard_name, decision) = {
                let gctx = GuardCtx {
                    phase: GuardPhase::PreExecute,
                    call,
                    spec: &spec,
                    sandbox: policy.sandbox,
                    subject,
                    result: None,
                };
                self.guards.run(&gctx)
            };
            match decision {
                GuardDecision::Deny { reason } => {
                    session.log.append(EventKind::GuardDecision {
                        call_id: call.id.clone(),
                        phase: GuardPhase::PreExecute,
                        guard: guard_name,
                        decision: GuardDecision::deny(reason.clone()),
                    });
                    self.push_result(
                        session,
                        &call.id,
                        ToolResult::err(format!(
                            "denied: {reason}；{}",
                            { let t = if *detour_hinted { NO_DETOUR_SHORT } else { NO_DETOUR }; *detour_hinted = true; t }
                        )),
                    );
                    continue;
                }
                GuardDecision::NeedApproval { reason } => {
                    // 审批闸门（可交互挂起等待用户）——串行，避免多卡片竞态。
                    let (ok, note) =
                        self.resolve_approval(session, call, &spec.name, &reason).await;
                    if !ok {
                        self.push_result(
                            session,
                            &call.id,
                            ToolResult::err(approval_rejected_text(note)),
                        );
                        continue;
                    }
                }
                GuardDecision::Allow => {
                    // 即便守卫未要求审批，UnlessTrusted 策略下对非幂等工具也要问一次
                    if policy.approval == ApprovalPolicy::UnlessTrusted
                        && spec.guard.requires_approval == Approval::Never
                        && !spec.guard.idempotent
                    {
                        let (ok, note) = self
                            .resolve_approval(
                                session,
                                call,
                                &spec.name,
                                "policy UnlessTrusted: non-idempotent tool",
                            )
                            .await;
                        if !ok {
                            self.push_result(
                                session,
                                &call.id,
                                ToolResult::err(approval_rejected_text(note)),
                            );
                            continue;
                        }
                    }
                }
            }
            // 交互提问/计划审批等 user_interactive 工具内核托管：门控 → 分派 → 登记 → 落事件。
            // 工具不触碰 SessionLog，事件只从内核写入（方案 20260914 §4.3 不变量）。
            let mut question: Option<QuestionTicket> = None;
            if spec.user_interactive {
                // 门控矩阵：挂起只允许发生在"用户看得见、答得了"的桌面交互回合。
                // - 服务降级（CLI/e2e 装配）：fail-open，直接 dismissed；
                // - policy=Never（无人值守档，含 IM FULL_ACCESS 覆盖）：对齐"宁拒不挂"语义，
                //   IM tick 串行内联 await 回合，挂起会冻死整条 IM 通道；
                // - task 子回合：TurnCancel 旗标在子回合内不可达、事件不实时外送，挂起即失控。
                //   （红队 N4 的内核级闸门：子代理另有 ToolRegistry 层收走交互控制面工具，双保险。）
                let subagent = SUBAGENT_TURN.try_with(|s| *s).unwrap_or(false);
                if !self.questions.enabled()
                    || policy.approval == ApprovalPolicy::Never
                    || subagent
                {
                    let note = if spec.name == crate::exit_plan::EXIT_PLAN_TOOL_NAME {
                        "非交互回合，计划审批不可用（已自动忽略）。请继续调整计划并等待用户指示"
                    } else {
                        "非交互回合，提问不可用（已自动忽略）"
                    };
                    self.push_result(session, &call.id, question_dismissed_result(note));
                    continue;
                }
                if spec.name == crate::exit_plan::EXIT_PLAN_TOOL_NAME {
                    // —— exit_plan（计划审批）托管路径（§7.4）——
                    // 模式外调用 → 显式报错（不挂起、不误触答题卡）。
                    if !plan_mode_active() {
                        self.push_result(
                            session,
                            &call.id,
                            ToolResult::err("当前不在计划模式：exit_plan 仅在计划模式可用"),
                        );
                        continue;
                    }
                    let plan_text = match crate::exit_plan::normalize_plan_input(&call.input) {
                        Ok(p) => p,
                        Err(e) => {
                            self.push_result(session, &call.id, ToolResult::err(format!("exit_plan 参数无效：{e}")));
                            continue;
                        }
                    };
                    let (request_id, rx) =
                        self.questions.register(&session.id, crate::exit_plan::approval_question());
                    session.log.append(EventKind::QuestionAsked {
                        request_id: request_id.clone(),
                        questions: crate::exit_plan::approval_question(),
                    });
                    question = Some(QuestionTicket {
                        request_id,
                        rx,
                        kind: TicketKind::ExitPlan {
                            plan: plan_text,
                            roots: roots.unwrap_or(&policy.allowed_roots).to_vec(),
                        },
                    });
                } else {
                    // —— ask_user 托管路径 ——
                    // 同会话串行化：已有待答提问时拒绝新问（审批闸门刻意串行的同款考量，
                    // join_all 下两个 ask_user 会并发弹双卡）。
                    if self.questions.has_pending(&session.id) {
                        self.push_result(
                            session,
                            &call.id,
                            ToolResult::err("已有待答提问，请等待用户回答当前问题后再发起新提问"),
                        );
                        continue;
                    }
                    match normalize_ask_input(&call.input) {
                        Ok(questions) => {
                            let (request_id, rx) = self.questions.register(&session.id, questions.clone());
                            session.log.append(EventKind::QuestionAsked {
                                request_id: request_id.clone(),
                                questions,
                            });
                            question = Some(QuestionTicket {
                                request_id,
                                rx,
                                kind: TicketKind::Ask,
                            });
                        }
                        Err(e) => {
                            self.push_result(
                                session,
                                &call.id,
                                ToolResult::err(format!("ask_user 参数无效：{e}")),
                            );
                            continue;
                        }
                    }
                }
            }
            pending.push(Pending {
                call,
                tool,
                spec,
                question,
            });
        }

        if pending.is_empty() {
            return;
        }

        // —— ⑤ 沙箱执行(并发)：只有工具体 invoke() 并发；ToolCtx 只读、被所有 future 共享借用。
        // join_all 在同一任务上协作式并发：子智能体/网络 I/O 型工具在此段真并行推进。
        let tctx = ToolCtx {
            sandbox: policy.sandbox,
            // 回合级文件根：app 层按「会话所属空间」快照传入（None=回落共享 policy 的当前空间根）。
            // 旧实现直接读共享 policy.allowed_roots——两会话并发回合互相覆盖对方的工作空间根（lost update）。
            allowed_roots: roots.unwrap_or(&policy.allowed_roots),
            // 会话归属（阶段一）：子智能体每父会话并发计数等 per-session 能力用。
            session_id: &session.id,
        };
        // 提问票据先 take 出来（oneshot await 需独占所有权；闭包按索引取走，None = 普通工具）。
        let mut waiters: Vec<Option<QuestionTicket>> = Vec::with_capacity(pending.len());
        for p in pending.iter_mut() {
            waiters.push(p.question.take());
        }
        let results: Vec<CallOutcome> =
            futures_util::future::join_all(pending.iter().enumerate().map(|(i, p)| {
                let tool = p.tool.clone();
                let input = p.call.input.clone();
                let tctx = &tctx;
                // take 在闭包体内、async move 之外：oneshot 等待端需独占所有权，
                // 而 async move 会整体捕获捕获变量（不能把 &mut waiters 带进 future）。
                let ticket = waiters[i].take();
                async move {
                    if let Some(ticket) = ticket {
                        return self.resolve_question_ticket(ticket).await;
                    }
                    CallOutcome {
                        result: match tool.invoke(input, tctx).await {
                            Ok(r) => r,
                            Err(e) => ToolResult::err(e.0),
                        },
                        question_verdict: None,
                    }
                }
            }))
            .await; // join_all 保序：results[i] 对应 pending[i]

        // —— ⑥ 观察回灌 + post 守卫(串行, 按调用序保序落库) ——
        for (p, outcome) in pending.iter().zip(results) {
            let result = outcome.result;
            let (post_guard, post_decision) = {
                let gctx = GuardCtx {
                    phase: GuardPhase::PostExecute,
                    call: p.call,
                    spec: &p.spec,
                    sandbox: policy.sandbox,
                    subject,
                    result: Some(&result),
                };
                self.guards.run(&gctx)
            };
            let final_result = if let GuardDecision::Deny { reason } = &post_decision {
                session.log.append(EventKind::GuardDecision {
                    call_id: p.call.id.clone(),
                    phase: GuardPhase::PostExecute,
                    guard: post_guard,
                    decision: GuardDecision::deny(reason.clone()),
                });
                let tail = if *detour_hinted { NO_DETOUR_SHORT } else { NO_DETOUR };
                *detour_hinted = true;
                ToolResult::err(format!("post-guard denied: {reason}；{tail}"))
            } else {
                result
            };
            self.push_result(session, &p.call.id, final_result);
            // 提问解决事件（UI 把答题卡替换为结果态；审计/重放锚点；答案随事件落库供 UI 还原 Q/A）。
            if let Some(v) = outcome.question_verdict {
                session.log.append(EventKind::QuestionResolved {
                    request_id: v.request_id,
                    answered: v.answered,
                    by: v.by.to_string(),
                    answers: v.answers,
                });
            }
        }
    }

    /// 等待一个提问的结局并组装 tool result（内核托管挂起点）。
    ///
    /// 兜底超时（服务装配决定，默认 30min）：挂起回合在 Tauri 壳钉死 blocking 线程、同会话
    /// 后续请求在 session_lock 排队，无限挂起可饿死线程池。超时/清理唤醒都按 dismissed 回灌
    /// （`dismissed:true` 让模型自行降级继续，不中止回合——对齐审批拒绝后回合继续的现状）。
    async fn resolve_question_ticket(&self, ticket: QuestionTicket) -> CallOutcome {
        let request_id = ticket.request_id.clone();
        let wait = ticket.rx;
        let outcome = match self.questions.timeout() {
            Some(d) => match tokio::time::timeout(d, wait).await {
                Ok(Ok(o)) => o,
                // 发送端被丢弃（清理路径/幽灵摘除）→ 按取消忽略。
                Ok(Err(_)) => QuestionOutcome::Dismissed("canceled"),
                Err(_elapsed) => {
                    self.questions.expire(&request_id);
                    QuestionOutcome::Dismissed("timeout")
                }
            },
            None => wait.await.unwrap_or(QuestionOutcome::Dismissed("canceled")),
        };
        let (result, verdict) = match ticket.kind {
            TicketKind::Ask => match outcome {
                QuestionOutcome::Answered(map) => {
                    let answers = map.clone(); // 事件要带一份给 UI 还原 Q/A；json! 按值消费原 map
                    (
                        ToolResult::ok(serde_json::json!({ "answers": map })),
                        QuestionVerdict { request_id, answered: true, by: "user", answers },
                    )
                }
                QuestionOutcome::Dismissed(by) => (
                    ToolResult::ok(serde_json::json!({
                        "answers": {},
                        "dismissed": true,
                        "note": format!("用户未回答（{by}），请自行选择合理默认继续，不要反复追问")
                    })),
                    QuestionVerdict {
                        request_id,
                        answered: false,
                        by,
                        answers: serde_json::Map::new(),
                    },
                ),
            },
            TicketKind::ExitPlan { plan, roots } => {
                // 批准判定（§7.4）：首项 == 批准 label 且无附言。批准 → 同回合翻活开关
                // （托管路径与守卫同在回合任务树内，try_with 必命中）→ 下一批工具立即放行写；
                // 计划文本落盘 `<roots[0]>/.cmx/plans/<ts>.md`（失败不阻塞，计划仍在会话内）。
                let answered = matches!(&outcome, QuestionOutcome::Answered(_));
                let approved = match &outcome {
                    QuestionOutcome::Answered(m) => crate::exit_plan::is_approval_answer(m),
                    QuestionOutcome::Dismissed(_) => false,
                };
                if approved {
                    deactivate_plan_mode();
                }
                let saved_path = if approved {
                    crate::exit_plan::save_plan(&plan, &roots)
                } else {
                    None
                };
                let answers = match &outcome {
                    QuestionOutcome::Answered(m) => m.clone(),
                    QuestionOutcome::Dismissed(_) => serde_json::Map::new(),
                };
                let by = match &outcome {
                    QuestionOutcome::Dismissed(b) => *b,
                    QuestionOutcome::Answered(_) => "user",
                };
                (
                    crate::exit_plan::compose_result(&outcome, saved_path.as_ref()),
                    QuestionVerdict { request_id, answered, by, answers },
                )
            }
        };
        CallOutcome {
            result,
            question_verdict: Some(verdict),
        }
    }

    /// 触发并记录一次人在环审批，返回 (是否批准, 用户附言——拒绝时给模型的自愈提示)。
    async fn resolve_approval(
        &self,
        session: &mut Session,
        call: &ToolCall,
        tool: &str,
        reason: &str,
    ) -> (bool, Option<String>) {
        let summary = call_summary(call);
        if self.effective_policy().approval == ApprovalPolicy::Never {
            // 不打断策略：视 NeedApproval 为拒绝
            session.log.append(EventKind::ApprovalRequested {
                call_id: call.id.clone(),
                tool: tool.to_string(),
                reason: reason.to_string(),
                summary,
            });
            session.log.append(EventKind::ApprovalResolved {
                call_id: call.id.clone(),
                approved: false,
                by: "policy:never".into(),
            });
            return (false, None);
        }
        // 本对话已授予「全部允许」→ 自动放行（留审计事件，不再弹审批卡）。
        if self.approver.is_preapproved(&session.id) {
            session.log.append(EventKind::ApprovalResolved {
                call_id: call.id.clone(),
                approved: true,
                by: "auto:approve-all".into(),
            });
            return (true, None);
        }
        session.log.append(EventKind::ApprovalRequested {
            call_id: call.id.clone(),
            tool: tool.to_string(),
            reason: reason.to_string(),
            summary,
        });
        let (approved, by, note) = self
            .approver
            .resolve_for_session_with_note(&session.id, call, reason)
            .await;
        session.log.append(EventKind::ApprovalResolved {
            call_id: call.id.clone(),
            approved,
            by,
        });
        (approved, note.filter(|_| !approved))
    }

    fn push_result(&self, session: &mut Session, call_id: &str, result: ToolResult) {
        session.log.append(EventKind::ToolResult {
            call_id: call_id.to_string(),
            ok: result.ok,
            output: result.output,
        });
    }
}

/// 一次提问挂起的票据：执行段据 rx 等待结局，回灌段据 request_id/by 落解决事件。
/// `kind` 区分 ask_user（通用提问）与 exit_plan（计划审批，结果需判定+翻旗标+落盘）。
struct QuestionTicket {
    request_id: String,
    rx: tokio::sync::oneshot::Receiver<QuestionOutcome>,
    kind: TicketKind,
}

enum TicketKind {
    /// ask_user：结果即答案 JSON。
    Ask,
    /// exit_plan：批准 → 翻 TURN_PLAN_MODE + 计划落盘（roots 为本回合工作区根快照）。
    ExitPlan { plan: String, roots: Vec<PathBuf> },
}

/// 工具调用参数的单行摘要（审批卡 `$ …` 展示 + 待决恢复用）：
/// 单字符串字段直接取值（如 shell 的 cmd / write 的 path），其余取紧凑 JSON，超长截断。
pub fn call_summary(call: &ToolCall) -> String {
    const MAX: usize = 240;
    let text = match call.input.as_object() {
        Some(map) if map.len() == 1 => match map.values().next() {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(v) => v.to_string(),
            None => String::new(),
        },
        _ => call.input.to_string(),
    };
    let mut out: String = text.chars().take(MAX).collect();
    if text.chars().count() > MAX {
        out.push('…');
    }
    out
}

/// 守卫拦截回灌的统一「勿绕道」尾巴（与 approval_rejected_text 同语义）：拦截针对的是这件事本身，
/// 不是只拦这一把工具——不讲清模型就会换工具变通绕过闸门。
const NO_DETOUR: &str =
    "请勿换用其它工具或变通手段达成同一目的，也不要原样重试；请简要说明情况后停下，等待用户的进一步指示。";
/// 同回合已给过全量提示后的短句重申（防连续拦截时上下文膨胀）。
const NO_DETOUR_SHORT: &str = "重申前述要求：勿绕道、勿重试，停下等待用户指示。";

/// 拒绝回灌文本：把「拒绝」的语义讲全——用户拒的是**这件事本身**，不是只拒这一把工具调用。
/// 不讲清模型就会绕道换工具变相执行（实测：shell 被拒后改用内置 echo 免审批完成同一操作）。
/// 用户附言（ZCode 式「告诉模型接下来该怎么做」）优先：附言即用户的明确去向，按附言办。
fn approval_rejected_text(note: Option<String>) -> String {
    match note {
        Some(n) if !n.trim().is_empty() => format!(
            "denied: 用户拒绝了这次操作，附言指示：「{}」。附言优先：按附言执行；\
             若附言未另行安排，请勿换用其它工具或变通手段达成被拒的原目的，也不要原样重试。",
            n.trim()
        ),
        _ => String::from(
            "denied: 用户拒绝了这次操作。拒绝针对的是这件事本身，不是只针对当前工具：\
             请勿换用其它工具或变通手段达成同一目的，也不要原样重试；\
             请简要说明情况，然后停下等待用户的进一步指示。",
        ),
    }
}

/// 执行段的每个调用产出：工具结果 + 提问裁决（供回灌段落 QuestionResolved）。
struct CallOutcome {
    result: ToolResult,
    question_verdict: Option<QuestionVerdict>,
}

/// 提问裁决：answered + 放弃方（Answered 时 by="user"）+ 答案（随 QuestionResolved 落库，
/// UI 轨迹行展开还原 Q/A；未答为空表）。
struct QuestionVerdict {
    request_id: String,
    answered: bool,
    by: &'static str,
    answers: serde_json::Map<String, serde_json::Value>,
}

/// 门控降级（非交互回合/降级服务）时的提问结果：`dismissed:true` 让模型自行降级继续，
/// 不中止回合——无人值守拿空答案好过永久挂死。
fn question_dismissed_result(note: impl Into<String>) -> ToolResult {
    ToolResult::ok(serde_json::json!({
        "answers": {},
        "dismissed": true,
        "note": note.into()
    }))
}

/// 内核装配器。
#[derive(Default)]
pub struct AgentBuilder {
    model: Option<Arc<dyn ModelSeam>>,
    tools: ToolRegistry,
    guards: GuardPipeline,
    approver: Option<Arc<dyn Approver>>,
    questions: Option<Arc<QuestionService>>,
    policy: Policy,
}

impl AgentBuilder {
    pub fn model(mut self, m: Arc<dyn ModelSeam>) -> Self {
        self.model = Some(m);
        self
    }

    pub fn tools(mut self, t: ToolRegistry) -> Self {
        self.tools = t;
        self
    }

    pub fn guards(mut self, g: GuardPipeline) -> Self {
        self.guards = g;
        self
    }

    pub fn approver(mut self, a: Arc<dyn Approver>) -> Self {
        self.approver = Some(a);
        self
    }

    /// 交互提问服务（桌面双壳传真服务；不传 = 降级实例，ask_user 直接 dismissed）。
    pub fn questions(mut self, q: Arc<QuestionService>) -> Self {
        self.questions = Some(q);
        self
    }

    pub fn policy(mut self, p: Policy) -> Self {
        self.policy = p;
        self
    }

    /// 装配。缺模型缝 → `NotConfigured`。审批者缺省为"自动拒绝"（最安全默认）；
    /// 提问服务缺省为降级实例（不挂起，fail-open）。
    pub fn build(self) -> AgentResult<Agent> {
        let model = self
            .model
            .ok_or_else(|| AgentError::NotConfigured("model seam is required".into()))?;
        Ok(Agent {
            model,
            tools: self.tools,
            guards: self.guards,
            approver: self
                .approver
                .unwrap_or_else(|| Arc::new(AutoApprover::reject())),
            questions: self
                .questions
                .unwrap_or_else(|| Arc::new(QuestionService::disabled())),
            policy: std::sync::RwLock::new(self.policy),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::EventKind;
    use crate::model::{MockModel, ModelResponse};

    #[tokio::test]
    async fn cancelled_turn_is_persisted_as_stopped() {
        let agent = Agent::builder()
            .model(Arc::new(MockModel::saying("不应执行")))
            .build()
            .unwrap();
        let cancel = TurnCancel::new();
        cancel.cancel();
        let mut session = Session::new("s-cancel");
        let outcome = agent
            .run_turn_observed_as_cancellable(&mut session, "长任务", None, None, Some(&cancel), None)
            .await
            .unwrap();
        assert_eq!(outcome.reason, StopReason::Stopped);
        assert!(matches!(
            session.log.events().last().unwrap().kind,
            EventKind::TurnEnded { reason: StopReason::Stopped, .. }
        ));
    }

    /// 改造二（方案 20260914）：折尾注入——模型第一轮给纯文本（自然收口点），deferred 吐出
    /// 后台回执 → 作为 UserMessage 注入并续跑一轮；两条 UserMessage / 两条 ModelMessage
    /// 落在**同一个回合**里（无新 TurnStarted），注入轮计入 steps。
    #[tokio::test]
    async fn deferred_receipt_folds_into_running_turn() {
        let model = MockModel::new([
            ModelResponse::text("北京今天晴。"),
            ModelResponse::text("另外，后台子任务完成了：负责服务器监控与故障处理。"),
        ]);
        let agent = Agent::builder().model(Arc::new(model)).build().unwrap();
        let mut session = Session::new("s-fold");
        let queue = std::sync::Mutex::new(vec![
            "<task_result id=\"subtask-1\">负责服务器监控与故障处理</task_result>".to_string(),
        ]);
        let deferred = || {
            let mut q = queue.lock().unwrap();
            if q.is_empty() { None } else { Some(q.remove(0)) }
        };
        let outcome = agent
            .run_turn_observed_as_cancellable_with_policy_deferred(
                &mut session,
                "北京天气怎么样",
                None,
                None,
                None,
                None,
                None,
                None,
                Some(&deferred),
            )
            .await
            .unwrap();
        assert_eq!(outcome.reason, StopReason::Completed);
        assert_eq!(outcome.steps, 2, "注入轮计入 steps：2 次模型调用");
        let events = session.log.events();
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::TurnStarted { .. }))
                .count(),
            1,
            "折尾不另开回合：仍是一个 TurnStarted"
        );
        let user_texts: Vec<String> = events
            .iter()
            .filter_map(|e| match &e.kind {
                EventKind::UserMessage { text } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            user_texts,
            vec![
                "北京天气怎么样".to_string(),
                "<task_result id=\"subtask-1\">负责服务器监控与故障处理</task_result>".to_string()
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|e| matches!(e.kind, EventKind::ModelMessage { .. }))
                .count(),
            2,
            "原始回复 + 折尾续写都落日志"
        );
        assert!(matches!(
            events.last().unwrap().kind,
            EventKind::TurnEnded { reason: StopReason::Completed, .. }
        ));
    }

    /// 改造二回归：deferred 恒 None 时行为与原方法一致——单轮纯文本即收口，不注入。
    #[tokio::test]
    async fn deferred_none_keeps_single_step_turn() {
        let agent = Agent::builder()
            .model(Arc::new(MockModel::saying("就一句")))
            .build()
            .unwrap();
        let mut session = Session::new("s-nofold");
        let outcome = agent
            .run_turn_observed_as_cancellable_with_policy_deferred(
                &mut session, "hi", None, None, None, None, None, None, None,
            )
            .await
            .unwrap();
        assert_eq!(outcome.reason, StopReason::Completed);
        assert_eq!(outcome.steps, 1);
        assert_eq!(outcome.final_text.as_deref(), Some("就一句"));
    }
}
