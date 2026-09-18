//! 交互提问（ask_user）内核托管路径集成测试：事件序列、挂起-唤醒、门控矩阵、上下文配对。

use std::sync::Arc;
use std::time::Duration;

use cmx_agent_core::event::EventKind;
use cmx_agent_core::{
    Agent, ApprovalPolicy, AutoApprover, GuardPipeline, MockModel, ModelResponse, Policy,
    QuestionService, Session, ToolCall, SUBAGENT_TURN,
};
use serde_json::json;

fn ask_input() -> serde_json::Value {
    json!({"questions":[{"id":"s","header":"场景","question":"选一个","options":[
        {"label":"甲","description":"d1"},{"label":"乙","description":"d2"}]}]})
}

fn registry_with_ask() -> cmx_agent_core::ToolRegistry {
    let mut reg = cmx_agent_core::ToolRegistry::new();
    reg.register(Arc::new(cmx_agent_core::AskUserTool));
    reg
}

fn agent_with(model: MockModel, approval: ApprovalPolicy, svc: Option<Arc<QuestionService>>) -> Agent {
    let mut b = Agent::builder()
        .model(Arc::new(model))
        .tools(registry_with_ask())
        .guards(GuardPipeline::new()) // 无守卫 = 全放行，聚焦提问路径本身
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy {
            approval,
            ..Default::default()
        });
    if let Some(s) = svc {
        b = b.questions(s);
    }
    b.build().expect("build agent")
}

/// 事件流中某类事件的个数。
fn count(events: &[cmx_agent_core::SessionEvent], f: impl Fn(&EventKind) -> bool) -> usize {
    events.iter().filter(|e| f(&e.kind)).count()
}

#[tokio::test]
async fn ask_user_hangs_until_answered_then_continues() {
    let svc = Arc::new(QuestionService::interactive(None));
    let model = MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id("c1", "ask_user", ask_input())]),
        ModelResponse::text("好的，按你的选择继续"),
    ]);
    let agent = Arc::new(agent_with(model, ApprovalPolicy::OnRequest, Some(svc.clone())));

    // 后台任务：等提问出现后以「甲」作答（run_turn 阻塞在执行段挂起点）。
    let answer_task = tokio::spawn({
        let svc = svc.clone();
        async move {
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                let pend = svc.pending_in_session(Some("q-s1"));
                if let Some(p) = pend.first() {
                    assert!(svc.answer(&p.request_id, vec![vec!["甲".into()]], "q-s1"));
                    return;
                }
            }
            panic!("提问 2 秒内未出现");
        }
    });

    let mut s = Session::new("q-s1");
    let out = agent.run_turn(&mut s, "问我一个问题").await.unwrap();
    answer_task.await.unwrap();
    assert_eq!(out.reason, cmx_agent_core::event::StopReason::Completed);

    // 事件序列：ToolInvoked → QuestionAsked → ToolResult → QuestionResolved(answered)
    let evs = s.log.events();
    assert_eq!(count(evs, |k| matches!(k, EventKind::ToolInvoked { .. })), 1);
    let asked = count(evs, |k| matches!(k, EventKind::QuestionAsked { .. }));
    assert_eq!(asked, 1, "QuestionAsked 必须恰好一次（内核 pre 相落）");
    let resolved = evs
        .iter()
        .find(|e| matches!(e.kind, EventKind::QuestionResolved { .. }))
        .expect("QuestionResolved 必须落日志");
    match &resolved.kind {
        EventKind::QuestionResolved { answered, by, .. } => {
            assert!(answered);
            assert_eq!(by, "user");
        }
        _ => unreachable!(),
    }
    // 答案以 tool result JSON 回灌（模型可见）
    let answered_result = evs
        .iter()
        .find(|e| matches!(&e.kind, EventKind::ToolResult { ok: true, output, .. }
            if output.get("answers").is_some()))
        .expect("答案 tool result 必须落日志");
    match &answered_result.kind {
        EventKind::ToolResult { output, .. } => {
            assert_eq!(output["answers"]["s"], json!(["甲"]));
        }
        _ => unreachable!(),
    }
    // model_context 白名单：提问事件不回灌，答案 ToolResult 回灌
    let ctx = s.model_context(vec![]);
    let tool_msgs = ctx
        .messages
        .iter()
        .filter(|m| matches!(m, cmx_agent_core::ModelMessage::Tool { .. }))
        .count();
    assert_eq!(tool_msgs, 1, "只有答案一条 tool 消息");
}

#[tokio::test]
async fn ask_user_gated_for_unattended_policies() {
    for approval in [ApprovalPolicy::Never, ApprovalPolicy::Auto] {
        let svc = Arc::new(QuestionService::interactive(None));
        let model = MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id("c1", "ask_user", ask_input())]),
            ModelResponse::text("无人值守，自行继续"),
        ]);
        let agent = agent_with(model, approval, Some(svc.clone()));
        let mut s = Session::new("q-s2");
        let out = tokio::time::timeout(Duration::from_secs(1), agent.run_turn(&mut s, "问我"))
            .await.expect("无人值守不得等待回答").unwrap();
        assert_eq!(out.reason, cmx_agent_core::event::StopReason::Completed);
        let evs = s.log.events();
        assert_eq!(
            count(evs, |k| matches!(k, EventKind::QuestionAsked { .. })),
            0,
            "{approval:?} 不得发生提问挂起"
        );
        assert!(svc.pending_in_session(None).is_empty());
        // dismissed 回灌、回合继续
        assert!(evs.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { ok: true, output, .. }
            if output.get("dismissed") == Some(&json!(true)))));
    }
}

#[tokio::test]
async fn ask_user_gated_for_subagent_turn() {
    let svc = Arc::new(QuestionService::interactive(None));
    let model = MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id("c1", "ask_user", ask_input())]),
        ModelResponse::text("子任务自行继续"),
    ]);
    let agent = agent_with(model, ApprovalPolicy::OnRequest, Some(svc));
    let mut s = Session::new("q-s3");
    // 模拟 task 子回合（task.rs 以 SUBAGENT_TURN.scope(true, …) 跑子回合）
    let out = SUBAGENT_TURN
        .scope(true, agent.run_turn(&mut s, "问我"))
        .await
        .unwrap();
    assert_eq!(out.reason, cmx_agent_core::event::StopReason::Completed);
    let evs = s.log.events();
    assert_eq!(
        count(evs, |k| matches!(k, EventKind::QuestionAsked { .. })),
        0,
        "子回合不可取消，不得发生提问挂起"
    );
}

#[tokio::test]
async fn ask_user_fails_open_with_disabled_service() {
    // 不装配服务（默认 disabled）：CLI/e2e 路径，直接 dismissed 不挂起。
    let model = MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id("c1", "ask_user", ask_input())]),
        ModelResponse::text("没有交互面，自行继续"),
    ]);
    let agent = agent_with(model, ApprovalPolicy::OnRequest, None);
    let mut s = Session::new("q-s4");
    let out = agent.run_turn(&mut s, "问我").await.unwrap();
    assert_eq!(out.reason, cmx_agent_core::event::StopReason::Completed);
    let evs = s.log.events();
    assert_eq!(count(evs, |k| matches!(k, EventKind::QuestionAsked { .. })), 0);
}

#[tokio::test]
async fn duplicate_ask_in_same_step_is_rejected() {
    let svc = Arc::new(QuestionService::interactive(None));
    let model = MockModel::new([
        ModelResponse::calls(vec![
            ToolCall::with_id("c1", "ask_user", ask_input()),
            ToolCall::with_id("c2", "ask_user", ask_input()),
        ]),
        ModelResponse::text("继续"),
    ]);
    let agent = Arc::new(agent_with(model, ApprovalPolicy::OnRequest, Some(svc.clone())));

    let answer_task = tokio::spawn({
        let svc = svc.clone();
        async move {
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if let Some(p) = svc.pending_in_session(Some("q-s5")).first() {
                    svc.answer(&p.request_id, vec![vec!["乙".into()]], "q-s5");
                    return;
                }
            }
            panic!("提问未出现");
        }
    });
    let mut s = Session::new("q-s5");
    agent.run_turn(&mut s, "并发问两个").await.unwrap();
    answer_task.await.unwrap();

    // 同会话第二个 ask_user 被 has_pending 拒绝（防并发双卡），只有一次 QuestionAsked。
    let evs = s.log.events();
    assert_eq!(count(evs, |k| matches!(k, EventKind::QuestionAsked { .. })), 1);
    assert!(evs.iter().any(|e| matches!(&e.kind, EventKind::ToolResult { ok: false, output, .. }
        if output.get("error").and_then(|e| e.as_str()).is_some_and(|e| e.contains("已有待答提问")))));
}

#[tokio::test]
async fn cancel_session_dismisses_pending_question() {
    let svc = Arc::new(QuestionService::interactive(None));
    let model = MockModel::new([ModelResponse::calls(vec![ToolCall::with_id(
        "c1", "ask_user", ask_input(),
    )])]);
    let agent = Arc::new(agent_with(model, ApprovalPolicy::OnRequest, Some(svc.clone())));
    let cancel = cmx_agent_core::TurnCancel::new();
    let mut s = Session::new("q-s6");

    let stop_task = tokio::spawn({
        let svc = svc.clone();
        let cancel = cancel.clone();
        async move {
            for _ in 0..100 {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if !svc.pending_in_session(Some("q-s6")).is_empty() {
                    // 对齐 app 层 cancel_session_turn：oneshot 唤醒（而非杀任务）
                    svc.cancel_session("q-s6");
                    cancel.cancel();
                    return;
                }
            }
            panic!("提问未出现");
        }
    });
    let out = agent
        .run_turn_observed_as_cancellable(&mut s, "问我", None, None, Some(&cancel), None)
        .await
        .unwrap();
    stop_task.await.unwrap();
    // oneshot 唤醒 → 回灌 dismissed → 循环顶见旗标 → Stopped（无孤儿 tool_call）
    assert_eq!(out.reason, cmx_agent_core::event::StopReason::Stopped);
    let evs = s.log.events();
    assert!(evs.iter().any(|e| matches!(&e.kind, EventKind::QuestionResolved { answered: false, by, .. }
        if by == "canceled")));
    // 配对完整性：每个 tool_call 都有结果
    let ctx = s.model_context(vec![]);
    let calls: usize = ctx
        .messages
        .iter()
        .map(|m| match m {
            cmx_agent_core::ModelMessage::Assistant { tool_calls, .. } => tool_calls.len(),
            _ => 0,
        })
        .sum();
    let tools = ctx
        .messages
        .iter()
        .filter(|m| matches!(m, cmx_agent_core::ModelMessage::Tool { .. }))
        .count();
    assert_eq!(calls, tools, "tool_call 与 tool result 必须严格配对");
}

#[test]
fn model_context_self_heals_orphan_tool_calls() {
    // 手造历史：Assistant 带工具调用但结果缺失（中断/崩溃遗留）→ 投影补合成 err 结果。
    let mut s = Session::new("q-s7");
    s.log
        .append(EventKind::ModelMessage {
            text: None,
            tool_calls: vec![ToolCall::with_id("orphan", "shell", json!({"cmd":"ls"}))],
        });
    s.log.append(EventKind::ToolResult {
        call_id: "answered".into(),
        ok: true,
        output: json!({"echo": "hi"}),
    });
    let ctx = s.model_context(vec![]);
    let tool_ids: Vec<&String> = ctx
        .messages
        .iter()
        .filter_map(|m| match m {
            cmx_agent_core::ModelMessage::Tool { call_id, .. } => Some(call_id),
            _ => None,
        })
        .collect();
    // orphan 被补合成结果；answered（无对应调用）保留——投影不做反向删除
    assert!(tool_ids.iter().any(|id| *id == "orphan"));
    assert!(tool_ids.iter().any(|id| *id == "answered"));
}
