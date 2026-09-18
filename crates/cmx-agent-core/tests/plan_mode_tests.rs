//! 计划模式（方案 20260914 阶段二）内核契约：
//! - PlanModeGuard 白名单默认拒（未列工具拒绝、白名单内放行、无 scope 恒放行）；
//! - exit_plan 批准 → 同回合翻 [`TURN_PLAN_MODE`] → 后续非白名单工具立即放行（§7.4 核心承诺）；
//! - exit_plan 超时（兜底）→ dismissed → 保持只读；
//! - 子回合 / 非交互门控由 agent.rs 内核路径与 SUBAGENT_TURN 承担（question_turn_tests 同源）。

use std::sync::Arc;
use std::time::Duration;

use cmx_agent_core::agents::builtin_specs;
use cmx_agent_core::model::{ModelResponse, MockModel};
use cmx_agent_core::question::QuestionService;
use cmx_agent_core::{
    Agent, ApprovalGuard, ApprovalPolicy, GuardPipeline, PlanModeGuard, Session, Tool, ToolRegistry,
    ToolResult, ToolSpec, TurnPolicyOverride, TURN_PLAN_MODE,
};
use serde_json::json;

fn test_guards() -> GuardPipeline {
    // 计划模式在审批前拒绝，审批策略不能覆盖用户的只读意图。
    let mut g = GuardPipeline::new();
    g.add(Arc::new(PlanModeGuard));
    g.add(Arc::new(ApprovalGuard));
    g
}

/// 探针工具（可命中任意名字）：记录是否真的执行过。
struct Probe {
    name: &'static str,
    ran: Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl Tool for Probe {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(self.name, "探针")
            .schema(json!({"type":"object"}))
    }
    async fn invoke(
        &self,
        _input: serde_json::Value,
        _ctx: &cmx_agent_core::ToolCtx<'_>,
    ) -> Result<ToolResult, cmx_agent_core::ToolError> {
        self.ran.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(ToolResult::ok(json!({"ran": true})))
    }
}

/// 白名单内的假 echo（名字在 PLAN_READ_TOOLS 中）。
fn echo_probe(ran: Arc<std::sync::atomic::AtomicBool>) -> Arc<dyn Tool> {
    Arc::new(Probe { name: "echo", ran })
}

fn build_agent(
    model: Arc<MockModel>,
    questions: Arc<QuestionService>,
    tools: Vec<Arc<dyn Tool>>,
) -> Arc<Agent> {
    let mut reg = ToolRegistry::new();
    for t in tools {
        reg.register(t);
    }
    Arc::new(
        Agent::builder()
            .model(model)
            .tools(reg)
            .guards(test_guards())
            .questions(questions)
            .build()
            .unwrap(),
    )
}

fn plan_flag(on: bool) -> Arc<std::sync::atomic::AtomicBool> {
    Arc::new(std::sync::atomic::AtomicBool::new(on))
}

#[tokio::test]
async fn plan_mode_blocks_non_whitelisted_tool() {
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
            "c1",
            "shell_probe",
            json!({}),
        )]),
        ModelResponse::text("好的"),
    ]));
    let agent = build_agent(
        model.clone(),
        Arc::new(QuestionService::disabled()),
        vec![Arc::new(Probe { name: "shell_probe", ran: ran.clone() })],
    );
    let mut s = Session::new("plan-deny");
    agent
        .run_turn_observed_as_cancellable_with_policy(
            &mut s, "看看", None, None, None, None, None,
            Some(plan_flag(true)),
        )
        .await
        .unwrap();
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "非白名单工具不得执行");
    let denied = s.log.events().iter().any(|e| {
        matches!(&e.kind, cmx_agent_core::EventKind::ToolResult { output, .. }
            if output.get("error").and_then(|v| v.as_str()).map(|x| x.contains("计划模式")).unwrap_or(false))
    });
    assert!(denied, "应有计划模式拒绝回灌");
}

#[tokio::test]
async fn plan_mode_allows_whitelisted_tool_and_defaults_inactive() {
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id("c1", "echo", json!({}))]),
        ModelResponse::text("好的"),
    ]));
    let agent = build_agent(
        model.clone(),
        Arc::new(QuestionService::disabled()),
        vec![echo_probe(ran.clone())],
    );
    // 计划模式开：白名单内放行
    let mut s = Session::new("plan-allow");
    agent
        .run_turn_observed_as_cancellable_with_policy(
            &mut s, "看看", None, None, None, None, None,
            Some(plan_flag(true)),
        )
        .await
        .unwrap();
    assert!(ran.load(std::sync::atomic::Ordering::SeqCst), "白名单工具应放行");
    // 无 plan flag（普通回合）：同名工具照常放行（守卫默认不活跃）
    let ran2 = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model2 = Arc::new(MockModel::new([
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id("c1", "shell_probe", json!({}))]),
        ModelResponse::text("好的"),
    ]));
    let agent2 = build_agent(
        model2,
        Arc::new(QuestionService::disabled()),
        vec![Arc::new(Probe { name: "shell_probe", ran: ran2.clone() })],
    );
    let mut s2 = Session::new("no-plan");
    agent2
        .run_turn_observed_as_cancellable_with_policy(
            &mut s2, "看看", None, None, None, None, None, None,
        )
        .await
        .unwrap();
    assert!(ran2.load(std::sync::atomic::Ordering::SeqCst), "无 scope 时守卫不拦截");
}

#[tokio::test]
async fn exit_plan_approval_unlocks_write_same_turn() {
    use cmx_agent_core::exit_plan::APPROVE_LABEL;
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = Arc::new(MockModel::new([
        // ① 计划模式下先调 exit_plan 提交计划
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
            "c1",
            "exit_plan",
            json!({ "plan": "# 计划\n1. 改文件\n2. 验证" }),
        )]),
        // ② 批准后同回合立即调非白名单工具 → 应放行
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id("c2", "shell_probe", json!({}))]),
        ModelResponse::text("已按计划实施"),
    ]));
    let questions = Arc::new(QuestionService::interactive(None));
    let agent = build_agent(
        model.clone(),
        questions.clone(),
        vec![Arc::new(Probe { name: "shell_probe", ran: ran.clone() }), Arc::new(cmx_agent_core::ExitPlanTool)],
    );
    // 后台应答者：轮询 pending 并以「批准」作答（对齐 UI 提交路径）
    {
        let svc = questions.clone();
        tokio::spawn(async move {
            for _ in 0..500 {
                if let Some(p) = svc.pending_in_session(None).first() {
                    svc.answer(&p.request_id, vec![vec![APPROVE_LABEL.to_string()]], "s-plan-ok");
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
    }
    let mut s = Session::new("s-plan-ok");
    agent
        .run_turn_observed_as_cancellable_with_policy(
            &mut s, "出计划然后执行", None, None, None, None, None,
            Some(plan_flag(true)),
        )
        .await
        .unwrap();
    assert!(ran.load(std::sync::atomic::Ordering::SeqCst), "批准后同回合非白名单工具应放行");
    let approved = s.log.events().iter().any(|e| {
        matches!(&e.kind, cmx_agent_core::EventKind::ToolResult { ok, output, .. }
            if *ok && output.get("approved") == Some(&json!(true)))
    });
    assert!(approved, "exit_plan 应回灌 approved:true");
    let note = s.log.events().iter().any(|e| {
        matches!(&e.kind, cmx_agent_core::EventKind::QuestionResolved { answered: true, .. })
    });
    assert!(note, "QuestionResolved 事件应落库");
}

#[tokio::test]
async fn unattended_exit_plan_never_asks_or_unlocks_writes() {
    for approval in [ApprovalPolicy::Never, ApprovalPolicy::Auto] {
        let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let model = Arc::new(MockModel::new([
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c1", "exit_plan", json!({"plan": "批准后写文件"}),
            )]),
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c2", "shell_probe", json!({}),
            )]),
            ModelResponse::text("等待用户"),
        ]));
        let questions = Arc::new(QuestionService::interactive(None));
        let agent = build_agent(model, questions.clone(), vec![
            Arc::new(cmx_agent_core::ExitPlanTool),
            Arc::new(Probe { name: "shell_probe", ran: ran.clone() }),
        ]);
        let flag = plan_flag(true);
        let mut s = Session::new("unattended-plan");
        tokio::time::timeout(Duration::from_secs(1),
            agent.run_turn_observed_as_cancellable_with_policy(
                &mut s, "提交计划", None, None, None, None,
                Some(TurnPolicyOverride { approval }), Some(flag.clone()),
            )
        ).await.expect("无人值守不得等待计划审批").unwrap();
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst), "未人工批准不得退出计划模式");
        assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "无人值守不得绕过计划模式执行写工具");
        assert!(questions.pending_in_session(None).is_empty());
        assert!(!s.log.iter().any(|e| matches!(e.kind, cmx_agent_core::EventKind::QuestionAsked { .. })));
        assert!(s.log.iter().any(|e| matches!(&e.kind,
            cmx_agent_core::EventKind::ToolResult { call_id, output, .. }
                if call_id == "c1" && output["dismissed"] == json!(true))));
        assert!(s.log.iter().any(|e| matches!(&e.kind,
            cmx_agent_core::EventKind::GuardDecision { guard, decision: cmx_agent_core::GuardDecision::Deny { .. }, .. }
                if guard == "plan_mode")));
    }
}

#[tokio::test]
async fn exit_plan_timeout_keeps_plan_mode_locked() {
    let ran = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
            "c1",
            "exit_plan",
            json!({ "plan": "计划" }),
        )]),
        // 超时后模型又想直接写 → 仍应被拒
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id("c2", "shell_probe", json!({}))]),
        ModelResponse::text("等待用户"),
    ]));
    // 50ms 兜底超时（实装默认 30 分钟，测试缩短）
    let questions = Arc::new(QuestionService::interactive(Some(Duration::from_millis(50))));
    let agent = build_agent(
        model,
        questions,
        vec![Arc::new(Probe { name: "shell_probe", ran: ran.clone() }), Arc::new(cmx_agent_core::ExitPlanTool)],
    );
    let mut s = Session::new("s-plan-timeout");
    agent
        .run_turn_observed_as_cancellable_with_policy(
            &mut s, "出计划", None, None, None, None, None,
            Some(plan_flag(true)),
        )
        .await
        .unwrap();
    assert!(!ran.load(std::sync::atomic::Ordering::SeqCst), "超时=继续调整：写仍被拒");
    let dismissed = s.log.events().iter().any(|e| {
        matches!(&e.kind, cmx_agent_core::EventKind::ToolResult { ok, output, .. }
            if *ok && output.get("approved") == Some(&json!(false)))
    });
    assert!(dismissed, "超时应回灌 approved:false（继续调整语义）");
}

#[tokio::test]
async fn exit_plan_outside_plan_mode_errors_without_asking() {
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
            "c1",
            "exit_plan",
            json!({ "plan": "计划" }),
        )]),
        ModelResponse::text("好的"),
    ]));
    let questions = Arc::new(QuestionService::interactive(None));
    let agent = build_agent(
        model,
        questions.clone(),
        vec![Arc::new(cmx_agent_core::ExitPlanTool)],
    );
    let mut s = Session::new("s-no-plan");
    agent
        .run_turn_observed_as_cancellable_with_policy(
            &mut s, "退出", None, None, None, None, None,
            None, // 非计划模式
        )
        .await
        .unwrap();
    assert!(questions.pending_in_session(None).is_empty(), "模式外调用不得弹审批卡");
    let err = s.log.events().iter().any(|e| {
        matches!(&e.kind, cmx_agent_core::EventKind::ToolResult { ok, output, .. }
            if !*ok && output.get("error").and_then(|v| v.as_str()).map(|x| x.contains("当前不在计划模式")).unwrap_or(false))
    });
    assert!(err, "模式外调用应显式报错");
}

#[tokio::test]
async fn plan_mode_inherits_into_task_child_via_shared_specs() {
    // 白名单含 task：计划模式下父派 general-purpose 子任务，子回合随 future 树继承只读——
    // 子代理调非白名单工具也应被拒（继承不可绕过，§7.5）。此处直接验证 flag 随 scope 下传。
    let flag = plan_flag(true);
    TURN_PLAN_MODE
        .scope(flag, async {
            assert!(cmx_agent_core::plan_mode_active());
        })
        .await;
    assert!(!cmx_agent_core::plan_mode_active(), "scope 外恒为非计划模式");
    // builtins 名单仍完好（explore 白名单不含写工具）
    let specs = builtin_specs();
    assert_eq!(specs.len(), 2);
}
