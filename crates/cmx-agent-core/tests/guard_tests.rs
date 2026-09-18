//! 守卫与审批契约：权限、审批标注、高危人审、无人值守、快照、fail-closed 与审计。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use cmx_agent_core::{
    Agent, Approval, ApprovalGuard, ApprovalPolicy, Approver, AuthGuard, EventKind, Guard,
    GuardCtx, GuardDecision, GuardHints, GuardPhase, GuardPipeline, HighRiskGuard, MockModel,
    ModelResponse, PlanModeGuard, Policy, Session, StopReason, Subject, TURN_POLICY_OVERRIDE, Tool,
    ToolCall, ToolCtx, ToolError, ToolRegistry, ToolResult, ToolSpec, TurnPolicyOverride,
};
use serde_json::json;

/// 内存探针，不执行 shell 或触碰文件；守卫测试不依赖下游工具的实现与 OS。
struct Probe {
    hints: GuardHints,
    runs: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("probe", "守卫探针").guard(self.hints.clone())
    }

    async fn invoke(
        &self,
        _input: serde_json::Value,
        ctx: &ToolCtx<'_>,
    ) -> Result<ToolResult, ToolError> {
        self.runs.fetch_add(1, Ordering::SeqCst);
        Ok(ToolResult::ok(
            json!({"ran": true, "roots": ctx.workspace_roots}),
        ))
    }
}

struct NoInteraction;

#[async_trait::async_trait]
impl Approver for NoInteraction {
    async fn resolve(&self, _call: &ToolCall, _reason: &str) -> (bool, String) {
        panic!("无人值守或无需审批的调用不得进入审批等待");
    }

    fn is_preapproved(&self, _session_id: &str) -> bool {
        true // 无人值守规则必须优先于会话「全部允许」。
    }
}

struct RecordingApprover {
    approve: bool,
    preapproved: bool,
    calls: AtomicUsize,
}

impl RecordingApprover {
    fn new(approve: bool, preapproved: bool) -> Self {
        Self {
            approve,
            preapproved,
            calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait::async_trait]
impl Approver for RecordingApprover {
    async fn resolve(&self, _call: &ToolCall, _reason: &str) -> (bool, String) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (self.approve, "test:user".into())
    }

    fn is_preapproved(&self, _session_id: &str) -> bool {
        self.preapproved
    }
}

fn guards() -> GuardPipeline {
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(AuthGuard::allow_all()))
        .add(Arc::new(PlanModeGuard))
        .add(Arc::new(HighRiskGuard))
        .add(Arc::new(ApprovalGuard));
    guards
}

fn call_tool() -> MockModel {
    MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id("c1", "probe", json!({}))]),
        ModelResponse::text("done"),
    ])
}

fn probe_agent(
    model: MockModel,
    hints: GuardHints,
    policy: Policy,
    guards: GuardPipeline,
    approver: Arc<dyn Approver>,
) -> (Agent, Arc<AtomicUsize>) {
    let runs = Arc::new(AtomicUsize::new(0));
    let mut tools = ToolRegistry::new();
    tools.register(Arc::new(Probe {
        hints,
        runs: runs.clone(),
    }));
    let agent = Agent::builder()
        .model(Arc::new(model))
        .tools(tools)
        .guards(guards)
        .approver(approver)
        .policy(policy)
        .build()
        .unwrap();
    (agent, runs)
}

fn last_tool_result(s: &Session) -> (bool, serde_json::Value) {
    s.log
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::ToolResult { ok, output, .. } => Some((*ok, output.clone())),
            _ => None,
        })
        .expect("a tool result")
}

fn assert_approval_audit(s: &Session, approved: bool, by: &str) {
    let events = s.log.events();
    let requested = events
        .iter()
        .position(|e| {
            matches!(&e.kind,
        EventKind::ApprovalRequested { call_id, .. } if call_id == "c1")
        })
        .unwrap();
    let resolved = events
        .iter()
        .position(|e| {
            matches!(&e.kind,
        EventKind::ApprovalResolved { call_id, approved: actual, by: actor }
            if call_id == "c1" && *actual == approved && actor == by)
        })
        .unwrap();
    let result = events
        .iter()
        .position(|e| {
            matches!(&e.kind,
        EventKind::ToolResult { call_id, .. } if call_id == "c1")
        })
        .unwrap();
    assert!(
        requested < resolved && resolved < result,
        "审批及结果须按序追加审计"
    );
}

#[test]
fn approval_defaults_and_wire_values() {
    assert_eq!(ApprovalPolicy::default(), ApprovalPolicy::OnRequest);
    assert_eq!(Policy::default().approval, ApprovalPolicy::OnRequest);
    assert_eq!(
        TurnPolicyOverride::UNATTENDED.approval,
        ApprovalPolicy::Auto
    );
    for (policy, wire) in [
        (ApprovalPolicy::Auto, "auto"),
        (ApprovalPolicy::Never, "never"),
        (ApprovalPolicy::OnRequest, "on-request"),
        (ApprovalPolicy::UnlessTrusted, "unless-trusted"),
    ] {
        assert_eq!(serde_json::to_value(policy).unwrap(), json!(wire));
        assert_eq!(
            serde_json::from_value::<ApprovalPolicy>(json!(wire)).unwrap(),
            policy
        );
    }
}

#[test]
fn guards_preserve_approval_annotations_and_require_high_risk_review() {
    let call = ToolCall::with_id("c1", "probe", json!({}));
    let subject = Subject::new("tester");
    for requires_approval in [Approval::Never, Approval::Conditional, Approval::Always] {
        for high_risk in [false, true] {
            let spec = ToolSpec::new("probe", "test").guard(GuardHints {
                requires_approval,
                high_risk,
                ..Default::default()
            });
            let ctx = GuardCtx {
                phase: GuardPhase::PreExecute,
                call: &call,
                spec: &spec,
                subject: &subject,
                workspace_roots: &[],
                result: None,
            };
            assert_eq!(
                ApprovalGuard.check(&ctx).is_allow(),
                requires_approval == Approval::Never
            );
            assert_eq!(HighRiskGuard.check(&ctx).is_allow(), !high_risk);
            if high_risk {
                assert!(matches!(
                    HighRiskGuard.check(&ctx),
                    GuardDecision::NeedApproval { .. }
                ));
            }
        }
    }
}

#[tokio::test]
async fn auth_guard_is_not_bypassed_by_auto() {
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(AuthGuard::new(|subject, permission| {
            subject.user == "authorized" && permission == "probe:invoke"
        })))
        .add(Arc::new(ApprovalGuard));
    for (user, allowed) in [("unauthorized", false), ("authorized", true)] {
        let (agent, runs) = probe_agent(
            call_tool(),
            GuardHints {
                requires_auth: Some("probe:invoke".into()),
                requires_approval: Approval::Conditional,
                ..Default::default()
            },
            Policy {
                approval: ApprovalPolicy::Auto,
                subject: Subject::new(user),
                ..Default::default()
            },
            guards.clone(),
            Arc::new(NoInteraction),
        );
        let mut s = Session::new(user);
        agent.run_turn(&mut s, "run").await.unwrap();
        assert_eq!(last_tool_result(&s).0, allowed);
        assert_eq!(runs.load(Ordering::SeqCst), usize::from(allowed));
        if !allowed {
            assert!(s.log.iter().any(|e| matches!(&e.kind,
                EventKind::GuardDecision { guard, decision: GuardDecision::Deny { .. }, .. }
                    if guard == "auth")));
            assert!(
                !s.log
                    .iter()
                    .any(|e| matches!(e.kind, EventKind::ApprovalResolved { .. }))
            );
        }
    }
}

#[tokio::test]
async fn unattended_policies_never_wait_and_only_auto_grants_non_high_risk_conditional() {
    for policy in [ApprovalPolicy::Auto, ApprovalPolicy::Never] {
        for requires_approval in [Approval::Never, Approval::Conditional, Approval::Always] {
            for high_risk in [false, true] {
                let needs_approval = requires_approval != Approval::Never || high_risk;
                let allowed = !needs_approval
                    || (policy == ApprovalPolicy::Auto
                        && requires_approval == Approval::Conditional
                        && !high_risk);
                let (agent, runs) = probe_agent(
                    call_tool(),
                    GuardHints {
                        requires_approval,
                        high_risk,
                        network: true,
                        writes: true,
                        ..Default::default()
                    },
                    Policy {
                        approval: policy,
                        ..Default::default()
                    },
                    guards(),
                    Arc::new(NoInteraction),
                );
                let mut s = Session::new("unattended");
                agent.run_turn(&mut s, "run").await.unwrap();
                assert_eq!(
                    last_tool_result(&s).0,
                    allowed,
                    "{policy:?}/{requires_approval:?}/{high_risk}"
                );
                assert_eq!(runs.load(Ordering::SeqCst), usize::from(allowed));
                if needs_approval {
                    assert_approval_audit(
                        &s,
                        allowed,
                        if policy == ApprovalPolicy::Auto {
                            "policy:auto"
                        } else {
                            "policy:never"
                        },
                    );
                } else {
                    assert!(
                        !s.log
                            .iter()
                            .any(|e| matches!(e.kind, EventKind::ApprovalRequested { .. }))
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn on_request_honors_approval_and_rejection_including_high_risk() {
    for requires_approval in [Approval::Never, Approval::Conditional, Approval::Always] {
        for high_risk in [false, true] {
            for approve in [false, true] {
                let approver = Arc::new(RecordingApprover::new(approve, false));
                let needs_approval = requires_approval != Approval::Never || high_risk;
                let allowed = !needs_approval || approve;
                let (agent, runs) = probe_agent(
                    call_tool(),
                    GuardHints {
                        requires_approval,
                        high_risk,
                        ..Default::default()
                    },
                    Policy::default(),
                    guards(),
                    approver.clone(),
                );
                let mut s = Session::new("on-request");
                assert_eq!(
                    agent.run_turn(&mut s, "run").await.unwrap().reason,
                    StopReason::Completed
                );
                let (ok, output) = last_tool_result(&s);
                assert_eq!(ok, allowed);
                assert_eq!(runs.load(Ordering::SeqCst), usize::from(allowed));
                assert_eq!(
                    approver.calls.load(Ordering::SeqCst),
                    usize::from(needs_approval)
                );
                if needs_approval {
                    assert_approval_audit(&s, approve, "test:user");
                }
                if !allowed {
                    assert!(
                        output["error"]
                            .as_str()
                            .unwrap()
                            .contains("用户拒绝了这次操作")
                    );
                    assert!(
                        output["error"]
                            .as_str()
                            .unwrap()
                            .contains("请勿换用其它工具")
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn auto_does_not_bypass_plan_mode_for_conditional_tool() {
    let (agent, runs) = probe_agent(
        call_tool(),
        GuardHints {
            requires_approval: Approval::Conditional,
            writes: true,
            ..Default::default()
        },
        Policy::default(),
        guards(),
        Arc::new(NoInteraction),
    );
    let mut s = Session::new("auto-plan");
    agent
        .run_turn_observed_as_cancellable_with_policy(
            &mut s,
            "run",
            None,
            None,
            None,
            None,
            Some(TurnPolicyOverride::UNATTENDED),
            Some(Arc::new(std::sync::atomic::AtomicBool::new(true))),
        )
        .await
        .unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert!(!last_tool_result(&s).0);
    assert!(s.log.iter().any(|e| matches!(&e.kind,
        EventKind::GuardDecision { guard, decision: GuardDecision::Deny { .. }, .. }
            if guard == "plan_mode")));
    assert!(
        !s.log
            .iter()
            .any(|e| matches!(e.kind, EventKind::ApprovalResolved { .. }))
    );
}

#[tokio::test]
async fn session_preapproval_cannot_approve_always_or_high_risk() {
    for (requires_approval, high_risk) in [
        (Approval::Conditional, false),
        (Approval::Always, false),
        (Approval::Never, true),
        (Approval::Conditional, true),
        (Approval::Always, true),
    ] {
        let approver = Arc::new(RecordingApprover::new(false, true));
        let (agent, runs) = probe_agent(
            call_tool(),
            GuardHints {
                requires_approval,
                high_risk,
                ..Default::default()
            },
            Policy::default(),
            guards(),
            approver.clone(),
        );
        let mut s = Session::new("preapproved");
        agent.run_turn(&mut s, "run").await.unwrap();
        let allowed = requires_approval == Approval::Conditional && !high_risk;
        assert_eq!(last_tool_result(&s).0, allowed);
        assert_eq!(runs.load(Ordering::SeqCst), usize::from(allowed));
        assert_eq!(approver.calls.load(Ordering::SeqCst), usize::from(!allowed));
        if allowed {
            assert!(
                !s.log
                    .iter()
                    .any(|e| matches!(e.kind, EventKind::ApprovalRequested { .. }))
            );
            assert!(s.log.iter().any(|e| matches!(&e.kind,
                EventKind::ApprovalResolved { approved: true, by, .. } if by == "auto:approve-all")));
        } else {
            assert_approval_audit(&s, false, "test:user");
        }
    }
}

#[tokio::test]
async fn unless_trusted_only_adds_approval_for_nonidempotent_tools() {
    for idempotent in [false, true] {
        let approver = Arc::new(RecordingApprover::new(false, false));
        let (agent, runs) = probe_agent(
            call_tool(),
            GuardHints {
                idempotent,
                ..Default::default()
            },
            Policy {
                approval: ApprovalPolicy::UnlessTrusted,
                ..Default::default()
            },
            guards(),
            approver.clone(),
        );
        let mut s = Session::new("unless-trusted");
        agent.run_turn(&mut s, "run").await.unwrap();
        assert_eq!(last_tool_result(&s).0, idempotent);
        assert_eq!(runs.load(Ordering::SeqCst), usize::from(idempotent));
        assert_eq!(
            approver.calls.load(Ordering::SeqCst),
            usize::from(!idempotent)
        );
    }
}

#[tokio::test]
async fn pipeline_short_circuits_on_first_deny() {
    struct Deny;
    impl Guard for Deny {
        fn name(&self) -> &str {
            "first_deny"
        }
        fn phases(&self) -> &[GuardPhase] {
            &[GuardPhase::PreExecute]
        }
        fn check(&self, _ctx: &GuardCtx<'_>) -> GuardDecision {
            GuardDecision::deny("nope")
        }
    }
    struct MustNotRun;
    impl Guard for MustNotRun {
        fn name(&self) -> &str {
            "unreachable"
        }
        fn phases(&self) -> &[GuardPhase] {
            &[GuardPhase::PreExecute]
        }
        fn check(&self, _ctx: &GuardCtx<'_>) -> GuardDecision {
            panic!("守卫必须短路")
        }
    }
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(Deny)).add(Arc::new(MustNotRun));
    let (agent, runs) = probe_agent(
        call_tool(),
        GuardHints::default(),
        Policy::default(),
        guards,
        Arc::new(NoInteraction),
    );
    let mut s = Session::new("fail-closed");
    assert_eq!(
        agent.run_turn(&mut s, "run").await.unwrap().reason,
        StopReason::Completed
    );
    assert_eq!(runs.load(Ordering::SeqCst), 0);
    assert!(
        last_tool_result(&s).1["error"]
            .as_str()
            .unwrap()
            .contains("nope")
    );
}

#[tokio::test]
async fn post_guard_can_deny_after_execution() {
    struct PostDeny;
    impl Guard for PostDeny {
        fn name(&self) -> &str {
            "post_deny"
        }
        fn phases(&self) -> &[GuardPhase] {
            &[GuardPhase::PostExecute]
        }
        fn check(&self, ctx: &GuardCtx<'_>) -> GuardDecision {
            assert!(ctx.result.unwrap().ok);
            GuardDecision::deny("redacted")
        }
    }
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(PostDeny));
    let (agent, runs) = probe_agent(
        call_tool(),
        GuardHints::default(),
        Policy::default(),
        guards,
        Arc::new(NoInteraction),
    );
    let mut s = Session::new("post-deny");
    agent.run_turn(&mut s, "run").await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert!(!last_tool_result(&s).0);
    assert!(
        last_tool_result(&s).1["error"]
            .as_str()
            .unwrap()
            .contains("post-guard denied")
    );
    assert!(s.log.iter().any(|e| matches!(&e.kind,
        EventKind::GuardDecision { phase: GuardPhase::PostExecute, guard, .. } if guard == "post_deny")));
}

#[tokio::test]
async fn unattended_override_is_scoped_and_preserves_global_policy_and_roots() {
    let model = MockModel::new((0..3).flat_map(|i| {
        [
            ModelResponse::calls(vec![ToolCall::with_id(format!("c{i}"), "probe", json!({}))]),
            ModelResponse::text("done"),
        ]
    }));
    let root = std::env::temp_dir();
    let (agent, runs) = probe_agent(
        model,
        GuardHints {
            requires_approval: Approval::Conditional,
            ..Default::default()
        },
        Policy {
            workspace_roots: vec![root.clone()],
            ..Default::default()
        },
        guards(),
        Arc::new(RecordingApprover::new(false, false)),
    );
    for (id, unattended) in [
        ("desktop-before", false),
        ("im", true),
        ("desktop-after", false),
    ] {
        let mut s = Session::new(id);
        agent
            .run_turn_observed_as_cancellable_with_policy(
                &mut s,
                "run",
                None,
                None,
                None,
                None,
                unattended.then_some(TurnPolicyOverride::UNATTENDED),
                None,
            )
            .await
            .unwrap();
        assert_eq!(last_tool_result(&s).0, unattended);
        assert_eq!(agent.policy().approval, ApprovalPolicy::OnRequest);
        assert_eq!(agent.policy().workspace_roots, vec![root.clone()]);
        if unattended {
            assert_eq!(last_tool_result(&s).1["roots"], json!([root]));
        }
    }
    assert_eq!(runs.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn unattended_override_inherits_and_rejects_hard_approval_without_waiting() {
    for (requires_approval, high_risk, allowed) in [
        (Approval::Conditional, false, true),
        (Approval::Always, false, false),
        (Approval::Never, true, false),
        (Approval::Conditional, true, false),
    ] {
        let (agent, runs) = probe_agent(
            call_tool(),
            GuardHints {
                requires_approval,
                high_risk,
                ..Default::default()
            },
            Policy::default(),
            guards(),
            Arc::new(NoInteraction),
        );
        let mut s = Session::new("inherited");
        // 模拟子回合不显式传覆盖参数，仍继承父任务树的无人值守档。
        TURN_POLICY_OVERRIDE
            .scope(
                TurnPolicyOverride::UNATTENDED,
                agent.run_turn(&mut s, "run"),
            )
            .await
            .unwrap();
        assert_eq!(last_tool_result(&s).0, allowed);
        assert_eq!(runs.load(Ordering::SeqCst), usize::from(allowed));
        assert_approval_audit(&s, allowed, "policy:auto");
        assert_eq!(agent.policy().approval, ApprovalPolicy::OnRequest);
    }
}

#[tokio::test]
async fn approvals_share_batch_snapshot_even_when_global_policy_changes() {
    struct SwitchPolicy {
        agent: OnceLock<Weak<Agent>>,
        calls: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl Approver for SwitchPolicy {
        async fn resolve(&self, _call: &ToolCall, _reason: &str) -> (bool, String) {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let agent = self.agent.get().unwrap().upgrade().unwrap();
            let mut policy = agent.policy();
            policy.approval = ApprovalPolicy::Never;
            agent.set_policy(policy);
            (true, "test:switch".into())
        }
    }
    let model = MockModel::new([
        ModelResponse::calls(vec![
            ToolCall::with_id("c1", "probe", json!({})),
            ToolCall::with_id("c2", "probe", json!({})),
        ]),
        ModelResponse::calls(vec![ToolCall::with_id("c3", "probe", json!({}))]),
        ModelResponse::text("done"),
    ]);
    let approver = Arc::new(SwitchPolicy {
        agent: OnceLock::new(),
        calls: AtomicUsize::new(0),
    });
    let (agent, runs) = probe_agent(
        model,
        GuardHints {
            requires_approval: Approval::Conditional,
            ..Default::default()
        },
        Policy::default(),
        guards(),
        approver.clone(),
    );
    let agent = Arc::new(agent);
    assert!(approver.agent.set(Arc::downgrade(&agent)).is_ok());
    let mut s = Session::new("snapshot");
    agent.run_turn(&mut s, "run").await.unwrap();
    assert_eq!(
        approver.calls.load(Ordering::SeqCst),
        2,
        "同批审批使用初始 OnRequest 快照"
    );
    assert_eq!(runs.load(Ordering::SeqCst), 2);
    assert!(!last_tool_result(&s).0, "下一批读取更新后的 Never");
    assert!(s.log.iter().any(|e| matches!(&e.kind,
        EventKind::ApprovalResolved { call_id, approved: false, by }
            if call_id == "c3" && by == "policy:never")));
}

// ── 工作目录写入审批分级（WorkspaceWriteGuard，沙箱移除后的补位）──

struct CountingApprover {
    calls: AtomicUsize,
}
#[async_trait::async_trait]
impl Approver for CountingApprover {
    async fn resolve(&self, _call: &ToolCall, _reason: &str) -> (bool, String) {
        self.calls.fetch_add(1, Ordering::SeqCst);
        (false, "audit-reject".into())
    }
}

fn write_guards() -> GuardPipeline {
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(AuthGuard::allow_all()))
        .add(Arc::new(PlanModeGuard))
        .add(Arc::new(HighRiskGuard))
        .add(Arc::new(cmx_agent_core::WorkspaceWriteGuard))
        .add(Arc::new(ApprovalGuard));
    guards
}

#[test]
fn workspace_write_guard_lexical_classification() {
    use std::path::PathBuf;
    let spec = ToolSpec::new("fs_write", "探针")
        .guard(GuardHints { writes: true, write_path_args: vec!["path".into()], ..Default::default() });
    let _call = ToolCall::with_id("c1", "fs_write", json!({"path":"out.txt"}));
    let subject = Subject::new("tester");
    let roots = vec![PathBuf::from("/tmp/ws")];
    let check_path = |path: &str, roots: &[PathBuf]| {
        let call = ToolCall::with_id("c1", "fs_write", json!({"path": path}));
        let ctx = GuardCtx {
            phase: GuardPhase::PreExecute,
            call: &call,
            spec: &spec,
            subject: &subject,
            workspace_roots: roots,
            result: None,
        };
        cmx_agent_core::WorkspaceWriteGuard.check(&ctx)
    };
    assert!(check_path("out.txt", &roots).is_allow(), "目录内相对路径放行");
    assert!(check_path("a/b/c.txt", &roots).is_allow(), "子目录放行");
    assert!(check_path("/tmp/ws/x.txt", &roots).is_allow(), "根内绝对路径放行");
    assert!(check_path("/tmp/ws/a/../b.txt", &roots).is_allow(), "词法回退仍在根内");
    assert!(!check_path("../escape.txt", &roots).is_allow(), ".. 越出根需审批");
    assert!(!check_path("/tmp/other/x.txt", &roots).is_allow(), "根外绝对路径需审批");
    assert!(!check_path("out.txt", &[]).is_allow(), "无根不可校验，从严需审批");
}

#[tokio::test]
async fn in_workspace_write_runs_without_approval_on_request() {
    let dir = std::env::temp_dir().join(format!("cmx-wsg-in-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let approver = Arc::new(CountingApprover { calls: AtomicUsize::new(0) });
    let (agent, runs) = probe_agent(
        MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id(
                "c1", "probe", json!({"path": "inside.txt"}),
            )]),
            ModelResponse::text("done"),
        ]),
        GuardHints { writes: true, write_path_args: vec!["path".into()], ..Default::default() },
        Policy { workspace_roots: vec![dir.clone()], ..Default::default() },
        write_guards(),
        approver.clone(),
    );
    let mut s = Session::new("wsg-in");
    agent.run_turn(&mut s, "go").await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 1, "目录内写放行执行");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 0, "不得弹审批");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn out_of_workspace_write_requires_approval_on_request() {
    let dir = std::env::temp_dir().join(format!("cmx-wsg-out-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let approver = Arc::new(CountingApprover { calls: AtomicUsize::new(0) });
    let (agent, runs) = probe_agent(
        MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id(
                "c1", "probe", json!({"path": "../escape.txt"}),
            )]),
            ModelResponse::text("done"),
        ]),
        GuardHints { writes: true, write_path_args: vec!["path".into()], ..Default::default() },
        Policy { workspace_roots: vec![dir.clone()], ..Default::default() },
        write_guards(),
        approver.clone(),
    );
    let mut s = Session::new("wsg-out");
    agent.run_turn(&mut s, "go").await.unwrap();
    assert_eq!(runs.load(Ordering::SeqCst), 0, "越界写未获批不执行");
    assert_eq!(approver.calls.load(Ordering::SeqCst), 1, "越界写需人工审批");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn approval_arg_values_triggers_on_git_write_subcommand_only() {
    use std::path::PathBuf;
    let dir = std::env::temp_dir().join(format!("cmx-wsg-git-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let write_subs = ["add", "commit", "checkout", "restore", "switch", "stash", "init"];
    for (sub, expect_approval) in [("status", false), ("commit", true)] {
        let approver = Arc::new(CountingApprover { calls: AtomicUsize::new(0) });
        let (agent, runs) = probe_agent(
            MockModel::new([
                ModelResponse::calls(vec![ToolCall::with_id(
                    "c1", "probe", json!({"subcommand": sub}),
                )]),
                ModelResponse::text("done"),
            ]),
            GuardHints {
                approval_arg_values: Some((
                    "subcommand".into(),
                    write_subs.iter().map(|s| s.to_string()).collect(),
                )),
                ..Default::default()
            },
            Policy { workspace_roots: vec![PathBuf::from(&dir)], ..Default::default() },
            write_guards(),
            approver.clone(),
        );
        let mut s = Session::new("wsg-git");
        agent.run_turn(&mut s, "go").await.unwrap();
        let want = if expect_approval { 0 } else { 1 };
        assert_eq!(runs.load(Ordering::SeqCst), want, "子命令 {sub} 执行情况");
        let approvals = approver.calls.load(Ordering::SeqCst);
        assert_eq!(
            approvals,
            usize::from(expect_approval),
            "子命令 {sub} 审批次数"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
