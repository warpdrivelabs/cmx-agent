//! 回合循环（方案图 4）。一个 **Turn** = 多个 **Step**；每个 Step：
//! ① 模型推理 → ② 工具路由 → ③ 护栏前置(pre) → ④ 审批闸门 → ⑤ 沙箱执行 → ⑥ 观察回灌(post)。
//! 回合内循环直到：模型不再要工具（Completed）/ 达到 max_steps（MaxSteps）/ 被停机（Stopped）。
//!
//! 不变量落地：每一步"模型可见"的产物（模型输出、工具调用、工具结果）都先 append 进日志，
//! 再据日志重建下一步上下文——见 [`crate::session::Session::model_context`]。

use std::path::PathBuf;
use std::sync::Arc;

use crate::error::{AgentError, AgentResult};
use crate::event::{EventKind, StopReason};
use crate::guard::{GuardCtx, GuardDecision, GuardPhase, GuardPipeline, SandboxMode, Subject};
use crate::model::{ModelResponse, ModelSeam};
use crate::session::Session;
use crate::tool::{Approval, ToolCall, ToolCtx, ToolRegistry, ToolResult};

/// 审批策略（两旋钮之「许可」——何时问你）。对齐 codex `approval_policy`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApprovalPolicy {
    /// 从不打断：把所有 NeedApproval 视为拒绝（无人值守自动化用，最安全的自动档）。
    Never,
    /// 按需：仅在守卫要求时询问（日常推荐）。
    #[default]
    OnRequest,
    /// 逢写必问：任何非幂等工具都要审批（谨慎档）。
    UnlessTrusted,
}

/// 人在环审批者（③ 人在环 的解决方；真实实现接 cmx-flow 审批 / IM 二次确认）。
pub trait Approver: Send + Sync {
    /// 返回 (是否批准, 审批人标识)。
    fn resolve(&self, call: &ToolCall, reason: &str) -> (bool, String);
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

impl Approver for AutoApprover {
    fn resolve(&self, _call: &ToolCall, _reason: &str) -> (bool, String) {
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

/// 智能体内核。
pub struct Agent {
    model: Arc<dyn ModelSeam>,
    tools: ToolRegistry,
    guards: GuardPipeline,
    approver: Arc<dyn Approver>,
    policy: Policy,
}

impl Agent {
    pub fn builder() -> AgentBuilder {
        AgentBuilder::default()
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.tools
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
        let reason = loop {
            if steps >= self.policy.max_steps {
                break StopReason::MaxSteps;
            }
            steps += 1;

            // ① 模型推理（上下文只从日志派生）
            let ctx = session.model_context(self.tools.specs());
            let resp: ModelResponse = self
                .model
                .complete(&ctx)
                .await
                .map_err(|e| AgentError::Model(e.0))?;

            // 记录模型输出（模型可见 → 必落日志）
            session.log.append(EventKind::ModelMessage {
                text: resp.text.clone(),
                tool_calls: resp.tool_calls.clone(),
            });
            if let Some(t) = &resp.text {
                final_text = Some(t.clone());
            }

            // 无工具调用 → 回合自然完成
            if !resp.wants_tools() {
                break StopReason::Completed;
            }

            // ②–⑥ 逐个处理工具调用
            for call in &resp.tool_calls {
                self.handle_tool_call(session, call).await;
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

    /// 处理单个工具调用：路由 → pre 守卫 → 审批 → 执行 → post 守卫 → 回灌结果。
    /// 任一环拒绝都以 `ToolResult::err` 回灌（让模型自愈，不中止回合）。
    async fn handle_tool_call(&self, session: &mut Session, call: &ToolCall) {
        session
            .log
            .append(EventKind::ToolInvoked { call: call.clone() });

        // ② 路由：工具不存在 → 错误结果回灌
        let Some(tool) = self.tools.get(&call.name) else {
            self.push_result(
                session,
                &call.id,
                ToolResult::err(format!("unknown tool '{}'", call.name)),
            );
            return;
        };
        let spec = tool.spec();

        // ③ 护栏前置（pre）
        let (guard_name, decision) = {
            let gctx = GuardCtx {
                phase: GuardPhase::PreExecute,
                call,
                spec: &spec,
                sandbox: self.policy.sandbox,
                subject: &self.policy.subject,
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
                    ToolResult::err(format!("denied: {reason}")),
                );
                return;
            }
            GuardDecision::NeedApproval { reason } => {
                // ④ 审批闸门
                if !self.resolve_approval(session, call, &spec.name, &reason) {
                    self.push_result(session, &call.id, ToolResult::err("approval rejected"));
                    return;
                }
            }
            GuardDecision::Allow => {
                // 即便守卫未要求审批，UnlessTrusted 策略下对非幂等工具也要问一次
                if self.policy.approval == ApprovalPolicy::UnlessTrusted
                    && spec.guard.requires_approval == Approval::Never
                    && !spec.guard.idempotent
                    && !self.resolve_approval(
                        session,
                        call,
                        &spec.name,
                        "policy UnlessTrusted: non-idempotent tool",
                    )
                {
                    self.push_result(session, &call.id, ToolResult::err("approval rejected"));
                    return;
                }
            }
        }

        // ⑤ 沙箱执行
        let tctx = ToolCtx {
            sandbox: self.policy.sandbox,
            allowed_roots: &self.policy.allowed_roots,
        };
        let result = match tool.invoke(call.input.clone(), &tctx).await {
            Ok(r) => r,
            Err(e) => ToolResult::err(e.0),
        };

        // ⑥ 观察回灌 + post 守卫（审计/脱敏），post 拒绝则替换为错误结果
        let (post_guard, post_decision) = {
            let gctx = GuardCtx {
                phase: GuardPhase::PostExecute,
                call,
                spec: &spec,
                sandbox: self.policy.sandbox,
                subject: &self.policy.subject,
                result: Some(&result),
            };
            self.guards.run(&gctx)
        };
        let final_result = if let GuardDecision::Deny { reason } = &post_decision {
            session.log.append(EventKind::GuardDecision {
                call_id: call.id.clone(),
                phase: GuardPhase::PostExecute,
                guard: post_guard,
                decision: GuardDecision::deny(reason.clone()),
            });
            ToolResult::err(format!("post-guard denied: {reason}"))
        } else {
            result
        };

        self.push_result(session, &call.id, final_result);
    }

    /// 触发并记录一次人在环审批，返回是否批准。
    fn resolve_approval(
        &self,
        session: &mut Session,
        call: &ToolCall,
        tool: &str,
        reason: &str,
    ) -> bool {
        if self.policy.approval == ApprovalPolicy::Never {
            // 不打断策略：视 NeedApproval 为拒绝
            session.log.append(EventKind::ApprovalRequested {
                call_id: call.id.clone(),
                tool: tool.to_string(),
                reason: reason.to_string(),
            });
            session.log.append(EventKind::ApprovalResolved {
                call_id: call.id.clone(),
                approved: false,
                by: "policy:never".into(),
            });
            return false;
        }
        session.log.append(EventKind::ApprovalRequested {
            call_id: call.id.clone(),
            tool: tool.to_string(),
            reason: reason.to_string(),
        });
        let (approved, by) = self.approver.resolve(call, reason);
        session.log.append(EventKind::ApprovalResolved {
            call_id: call.id.clone(),
            approved,
            by,
        });
        approved
    }

    fn push_result(&self, session: &mut Session, call_id: &str, result: ToolResult) {
        session.log.append(EventKind::ToolResult {
            call_id: call_id.to_string(),
            ok: result.ok,
            output: result.output,
        });
    }
}

/// 内核装配器。
#[derive(Default)]
pub struct AgentBuilder {
    model: Option<Arc<dyn ModelSeam>>,
    tools: ToolRegistry,
    guards: GuardPipeline,
    approver: Option<Arc<dyn Approver>>,
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

    pub fn policy(mut self, p: Policy) -> Self {
        self.policy = p;
        self
    }

    /// 装配。缺模型缝 → `NotConfigured`。审批者缺省为"自动拒绝"（最安全默认）。
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
            policy: self.policy,
        })
    }
}
