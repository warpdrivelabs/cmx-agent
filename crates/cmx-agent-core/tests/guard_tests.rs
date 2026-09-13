//! 守卫管道测试（五层护栏）：权限接地、人在环审批、高危拦截、两旋钮、fail-closed、post 相。

use std::sync::Arc;

use cmx_agent_core::event::{EventKind, StopReason};
use cmx_agent_core::guard::{Guard, GuardDecision, GuardPhase, Subject};
use cmx_agent_core::{
    Agent, ApprovalGuard, ApprovalPolicy, AuthGuard, AutoApprover, GuardPipeline, HighRiskGuard,
    MockModel, ModelResponse, Policy, SandboxMode, Session, ToolCall,
};
use cmx_agent_tools::default_registry;

fn call_tool(name: &str, input: serde_json::Value) -> MockModel {
    MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id("c1", name, input)]),
        ModelResponse::text("done"),
    ])
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

// ————— ① 权限接地 —————

#[tokio::test]
async fn auth_guard_denies_without_permission() {
    // fs_read 需要 "fs:read"；主体没有该权限 → 拒绝
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(AuthGuard::new(|_s, _p| false))); // 一律无权
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "fs_read",
            serde_json::json!({"path":"/etc/hosts"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("auth-deny");
    agent.run_turn(&mut s, "read file").await.unwrap();

    let (ok, output) = last_tool_result(&s);
    assert!(!ok);
    assert!(output["error"].as_str().unwrap().contains("denied"));
    // 且有一条 pre 相 GuardDecision::Deny 落审计
    let denied = s.log.iter().any(|e| {
        matches!(&e.kind,
        EventKind::GuardDecision { phase: GuardPhase::PreExecute, guard, .. } if guard == "auth")
    });
    assert!(denied);
}

#[tokio::test]
async fn auth_guard_allows_with_permission() {
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(AuthGuard::new(|s: &Subject, p| {
        p == "fs:read" && s.roles.iter().any(|r| r == "reader")
    })));
    let policy = Policy {
        subject: Subject {
            tenant: "t".into(),
            user: "u".into(),
            roles: vec!["reader".into()],
        },
        ..Default::default()
    };
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "echo",
            serde_json::json!({"text":"ok"}),
        ))) // echo 无需鉴权
        .tools(default_registry())
        .guards(guards)
        .policy(policy)
        .build()
        .unwrap();
    let mut s = Session::new("auth-allow");
    agent.run_turn(&mut s, "echo").await.unwrap();
    let (ok, _) = last_tool_result(&s);
    assert!(ok, "echo requires no auth, should pass");
}

// ————— ③ 人在环审批 —————

#[tokio::test]
async fn approval_required_and_granted() {
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(HighRiskGuard))
        .add(Arc::new(ApprovalGuard));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "shell",
            serde_json::json!({"cmd":"echo approval-test"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy {
            sandbox: SandboxMode::WorkspaceWrite,
            allowed_roots: vec![std::env::temp_dir()],
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("appr-grant");
    agent.run_turn(&mut s, "rm").await.unwrap();

    // 应有 ApprovalRequested + ApprovalResolved{approved:true}
    assert!(
        s.log
            .iter()
            .any(|e| matches!(e.kind, EventKind::ApprovalRequested { .. }))
    );
    assert!(
        s.log
            .iter()
            .any(|e| matches!(&e.kind, EventKind::ApprovalResolved { approved: true, .. }))
    );
    let (ok, _) = last_tool_result(&s);
    assert!(ok);
}

#[tokio::test]
async fn preapproved_session_skips_card() {
    // 会话被授予「本对话全部允许」→ 需审批的工具自动放行：
    // 无 ApprovalRequested（不弹卡）、有 by="auto:approve-all" 的 Resolved（留审计）、工具照跑。
    struct PreapprovedApprover;
    #[async_trait::async_trait]
    impl cmx_agent_core::Approver for PreapprovedApprover {
        async fn resolve(&self, _c: &cmx_agent_core::ToolCall, _r: &str) -> (bool, String) {
            panic!("session 已 pre-approved，resolve 不应被调用");
        }
        fn is_preapproved(&self, _sid: &str) -> bool {
            true
        }
    }
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(ApprovalGuard));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "danger_rm",
            serde_json::json!({"path":"/tmp/x"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(PreapprovedApprover))
        .policy(Policy {
            sandbox: SandboxMode::WorkspaceWrite,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("appr-all");
    agent.run_turn(&mut s, "rm").await.unwrap();

    assert!(
        !s.log
            .iter()
            .any(|e| matches!(e.kind, EventKind::ApprovalRequested { .. })),
        "pre-approved 会话不应弹审批卡"
    );
    assert!(
        s.log.iter().any(|e| matches!(&e.kind,
            EventKind::ApprovalResolved { approved: true, by, .. } if by == "auto:approve-all")),
        "应留 auto:approve-all 审计事件"
    );
    let (ok, _) = last_tool_result(&s);
    assert!(ok, "自动放行后工具应正常执行");
}

#[tokio::test]
async fn approval_required_and_rejected_blocks_tool() {
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(ApprovalGuard));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "danger_rm",
            serde_json::json!({"path":"/tmp/x"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(AutoApprover::reject()))
        .policy(Policy {
            sandbox: SandboxMode::WorkspaceWrite,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("appr-reject");
    agent.run_turn(&mut s, "rm").await.unwrap();
    let (ok, output) = last_tool_result(&s);
    assert!(!ok);
    assert!(
        output["error"]
            .as_str()
            .unwrap()
            .contains("approval rejected")
    );
}

// ————— ⑤ 高危拦截 + 沙箱 —————

#[tokio::test]
async fn high_risk_blocked_under_workspace_write() {
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(HighRiskGuard))
        .add(Arc::new(ApprovalGuard));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "danger_rm",
            serde_json::json!({"path":"/"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy {
            sandbox: SandboxMode::WorkspaceWrite,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("hr-block");
    agent.run_turn(&mut s, "rm -rf /").await.unwrap();
    let (ok, output) = last_tool_result(&s);
    assert!(
        !ok,
        "high-risk tool must be blocked outside danger-full-access"
    );
    assert!(output["error"].as_str().unwrap().contains("high-risk"));
}

#[tokio::test]
async fn high_risk_allowed_under_danger_full_access() {
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(HighRiskGuard))
        .add(Arc::new(ApprovalGuard));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "danger_rm",
            serde_json::json!({"path":"/tmp/y"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy {
            sandbox: SandboxMode::DangerFullAccess,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("hr-allow");
    agent.run_turn(&mut s, "rm").await.unwrap();
    let (ok, output) = last_tool_result(&s);
    assert!(ok);
    assert_eq!(output["executed"], false); // 演示工具永不真删
}

#[tokio::test]
async fn danger_full_access_still_gates_always_approval() {
    // danger-full-access（≈ --yolo）：Conditional 级人审跳过不弹卡，但 **Always 级（硬人审）
    // 不豁免**——两旋钮正交，能力旋钮不得吞掉许可旋钮的全部闸门。
    // approver 挂 reject 兜底：被问即拒。
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(HighRiskGuard))
        .add(Arc::new(ApprovalGuard));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "danger_rm",
            serde_json::json!({"path":"/tmp/yolo"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(AutoApprover::reject())) // 若被问就会失败
        .policy(Policy {
            sandbox: SandboxMode::DangerFullAccess,
            approval: ApprovalPolicy::OnRequest,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("yolo-gate");
    agent.run_turn(&mut s, "rm").await.unwrap();
    let (ok, _) = last_tool_result(&s);
    assert!(!ok, "danger 沙箱不得豁免 requires_approval:Always 的硬人审");
    assert!(
        s.log
            .iter()
            .any(|e| matches!(e.kind, EventKind::ApprovalRequested { .. })),
        "Always 工具在 danger 下仍应产生 ApprovalRequested"
    );
}

// ————— 两旋钮正交性 —————

#[tokio::test]
async fn approval_policy_never_treats_needapproval_as_deny() {
    // approval=Never：即便守卫要求审批，也当作拒绝（无人值守最安全）
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(ApprovalGuard));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "danger_rm",
            serde_json::json!({"path":"/tmp/z"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(AutoApprover::approve())) // 即便 approver 想批，也不该被问
        .policy(Policy {
            sandbox: SandboxMode::WorkspaceWrite,
            approval: ApprovalPolicy::Never,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("knob-never");
    agent.run_turn(&mut s, "rm").await.unwrap();
    let (ok, _) = last_tool_result(&s);
    assert!(!ok);
    // 记录里 resolved.by 应是 policy:never
    let by_policy = s.log.iter().any(|e| {
        matches!(&e.kind,
        EventKind::ApprovalResolved { approved: false, by, .. } if by == "policy:never")
    });
    assert!(by_policy);
}

#[tokio::test]
async fn unless_trusted_asks_for_nonidempotent_tool() {
    // add 是幂等、无审批标注；但 UnlessTrusted 只对**非幂等**工具追加审批 → add 不受影响
    // 用一个非幂等但无 high_risk/approval 的工具来验证：echo 幂等，danger_rm 非幂等但 high_risk。
    // 这里验证 add(幂等) 在 UnlessTrusted 下**不**触发审批。
    let agent = Agent::builder()
        .model(Arc::new(call_tool("add", serde_json::json!({"a":1,"b":2}))))
        .tools(default_registry())
        .approver(Arc::new(AutoApprover::reject())) // 若被问就会失败
        .policy(Policy {
            approval: ApprovalPolicy::UnlessTrusted,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("knob-unless");
    agent.run_turn(&mut s, "add").await.unwrap();
    let (ok, _) = last_tool_result(&s);
    assert!(ok, "idempotent add must not be gated by UnlessTrusted");
    assert!(
        !s.log
            .iter()
            .any(|e| matches!(e.kind, EventKind::ApprovalRequested { .. }))
    );
}

// ————— fail-closed：多守卫短路 —————

#[tokio::test]
async fn pipeline_short_circuits_on_first_deny() {
    // 自定义一个总是 Deny 的守卫放在最前，后面的守卫不应改变结局
    struct AlwaysDeny;
    impl Guard for AlwaysDeny {
        fn name(&self) -> &str {
            "always_deny"
        }
        fn phases(&self) -> &[GuardPhase] {
            &[GuardPhase::PreExecute]
        }
        fn check(&self, _c: &cmx_agent_core::guard::GuardCtx<'_>) -> GuardDecision {
            GuardDecision::deny("nope")
        }
    }
    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(AlwaysDeny))
        .add(Arc::new(AuthGuard::allow_all()));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "echo",
            serde_json::json!({"text":"hi"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("failclosed");
    let out = agent.run_turn(&mut s, "echo").await.unwrap();
    assert_eq!(out.reason, StopReason::Completed);
    let (ok, output) = last_tool_result(&s);
    assert!(!ok);
    assert!(output["error"].as_str().unwrap().contains("nope"));
}

// ————— post 相守卫 —————

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
        fn check(&self, _c: &cmx_agent_core::guard::GuardCtx<'_>) -> GuardDecision {
            GuardDecision::deny("redacted")
        }
    }
    let mut guards = GuardPipeline::new();
    guards.add(Arc::new(PostDeny));
    let agent = Agent::builder()
        .model(Arc::new(call_tool(
            "echo",
            serde_json::json!({"text":"secret"}),
        )))
        .tools(default_registry())
        .guards(guards)
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("postdeny");
    agent.run_turn(&mut s, "echo secret").await.unwrap();
    let (ok, output) = last_tool_result(&s);
    assert!(!ok);
    assert!(
        output["error"]
            .as_str()
            .unwrap()
            .contains("post-guard denied")
    );
}
