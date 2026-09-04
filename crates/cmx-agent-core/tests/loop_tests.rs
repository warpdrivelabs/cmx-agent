//! 回合循环集成测试：多 step 编排、工具结果回灌、停机原因、上下文只从日志派生。

use std::sync::Arc;

use cmx_agent_core::event::{EventKind, StopReason};
use cmx_agent_core::{
    Agent, ApprovalPolicy, AutoApprover, GuardPipeline, MockModel, ModelResponse, Policy,
    SandboxMode, Session, ToolCall,
};
use cmx_agent_tools::default_registry;

/// 装配一个用给定模型脚本 + 默认工具 + 全放行守卫的 agent。
fn agent_with(model: MockModel, approval: ApprovalPolicy) -> Agent {
    Agent::builder()
        .model(Arc::new(model))
        .tools(default_registry())
        .guards(GuardPipeline::new()) // 无守卫 = 全放行，聚焦循环本身
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy {
            sandbox: SandboxMode::WorkspaceWrite,
            approval,
            ..Default::default()
        })
        .build()
        .expect("build agent")
}

#[tokio::test]
async fn single_text_turn_completes() {
    let agent = agent_with(MockModel::saying("你好"), ApprovalPolicy::OnRequest);
    let mut s = Session::new("s1");
    let out = agent.run_turn(&mut s, "hi").await.unwrap();
    assert_eq!(out.reason, StopReason::Completed);
    assert_eq!(out.steps, 1);
    assert_eq!(out.final_text.as_deref(), Some("你好"));
}

#[tokio::test]
async fn tool_call_then_finish_two_steps() {
    // step1: 调 add(2,3)；step2: 收到结果后收尾
    let model = MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "add",
            serde_json::json!({"a":2,"b":3}),
        )]),
        ModelResponse::text("结果是 5"),
    ]);
    let agent = agent_with(model, ApprovalPolicy::OnRequest);
    let mut s = Session::new("s2");
    let out = agent.run_turn(&mut s, "算 2+3").await.unwrap();

    assert_eq!(out.reason, StopReason::Completed);
    assert_eq!(out.steps, 2);

    // 工具结果事件应包含 sum=5
    let tr = s.log.iter().find_map(|e| match &e.kind {
        EventKind::ToolResult {
            call_id,
            output,
            ok,
        } if call_id == "c1" => Some((*ok, output.clone())),
        _ => None,
    });
    let (ok, output) = tr.expect("tool result present");
    assert!(ok);
    assert_eq!(output["sum"], 5.0);
}

#[tokio::test]
async fn tool_result_is_fed_back_into_model_context() {
    // 第二次调用模型时，上下文里必须能看到第一步工具结果（回灌）
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "add",
            serde_json::json!({"a":10,"b":20}),
        )]),
        ModelResponse::text("done"),
    ]));
    let agent = Agent::builder()
        .model(model.clone())
        .tools(default_registry())
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("s3");
    agent.run_turn(&mut s, "算 10+20").await.unwrap();

    // 模型第二次看到的上下文应含一条 Tool 消息，output.sum == 30
    let ctx2 = model.nth_context(1).expect("second context");
    let has_tool_result = ctx2.messages.iter().any(
        |m| matches!(m, cmx_agent_core::ModelMessage::Tool { output, .. } if output["sum"] == 30.0),
    );
    assert!(
        has_tool_result,
        "second model context must include the fed-back tool result"
    );
}

#[tokio::test]
async fn unknown_tool_yields_error_result_not_crash() {
    let model = MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "no_such_tool",
            serde_json::json!({}),
        )]),
        ModelResponse::text("ok"),
    ]);
    let agent = agent_with(model, ApprovalPolicy::OnRequest);
    let mut s = Session::new("s4");
    let out = agent.run_turn(&mut s, "x").await.unwrap();
    assert_eq!(out.reason, StopReason::Completed);

    let errored = s.log.iter().any(|e| matches!(&e.kind,
        EventKind::ToolResult { ok: false, output, .. } if output["error"].as_str().unwrap_or("").contains("unknown tool")));
    assert!(errored, "unknown tool must produce an error ToolResult");
}

#[tokio::test]
async fn max_steps_guard_stops_runaway_loop() {
    // 模型永远请求工具 → 必须被 max_steps 截断
    let looping = MockModel::new([]).fallback(ModelResponse::calls(vec![ToolCall::with_id(
        "loop",
        "clock",
        serde_json::json!({}),
    )]));
    let agent = Agent::builder()
        .model(Arc::new(looping))
        .tools(default_registry())
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy {
            max_steps: 4,
            ..Default::default()
        })
        .build()
        .unwrap();
    let mut s = Session::new("s5");
    let out = agent.run_turn(&mut s, "loop forever").await.unwrap();
    assert_eq!(out.reason, StopReason::MaxSteps);
    assert_eq!(out.steps, 4);
}

#[tokio::test]
async fn parallel_tool_calls_in_one_step_all_execute() {
    // 一步里请求两个工具调用，都要执行并各自回灌
    let model = MockModel::new([
        ModelResponse::calls(vec![
            ToolCall::with_id("a", "add", serde_json::json!({"a":1,"b":1})),
            ToolCall::with_id("b", "echo", serde_json::json!({"text":"hi"})),
        ]),
        ModelResponse::text("both done"),
    ]);
    let agent = agent_with(model, ApprovalPolicy::OnRequest);
    let mut s = Session::new("s6");
    agent.run_turn(&mut s, "do two").await.unwrap();

    let results: Vec<_> = s
        .log
        .iter()
        .filter(|e| matches!(e.kind, EventKind::ToolResult { .. }))
        .collect();
    assert_eq!(results.len(), 2, "both tool calls must produce results");
}

#[tokio::test]
async fn multi_turn_accumulates_history() {
    let agent = agent_with(
        MockModel::new([ModelResponse::text("t1"), ModelResponse::text("t2")]),
        ApprovalPolicy::OnRequest,
    );
    let mut s = Session::new("s7");
    let o1 = agent.run_turn(&mut s, "first").await.unwrap();
    let o2 = agent.run_turn(&mut s, "second").await.unwrap();
    assert_eq!(o1.turn, 1);
    assert_eq!(o2.turn, 2);
    // 两个回合的 UserMessage 都在日志里
    let users = s.log.count(|k| matches!(k, EventKind::UserMessage { .. }));
    assert_eq!(users, 2);
}
