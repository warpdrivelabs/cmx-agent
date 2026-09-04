//! 不变量测试：**Model-visible means logged**。
//!
//! 核心断言：模型在任一 step 看到的每条消息（User/Assistant/Tool），都能在 append-only 会话日志里
//! 找到对应的、序号更早的源事件。这是审计可回放与信任的地基——若模型能看见却没记，就断言失败。

use std::sync::Arc;

use cmx_agent_core::event::{EventKind, SessionEvent};
use cmx_agent_core::{
    Agent, AutoApprover, GuardPipeline, MockModel, ModelMessage, ModelResponse, Policy, Session,
    ToolCall,
};
use cmx_agent_tools::default_registry;

/// 用一个会捕获上下文的 Mock 跑一个含工具调用的多 step 回合，然后逐一核对。
#[tokio::test]
async fn every_model_visible_message_has_a_logged_source() {
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "add",
            serde_json::json!({"a":7,"b":8}),
        )])
        .with_text("let me add"),
        ModelResponse::calls(vec![ToolCall::with_id(
            "c2",
            "echo",
            serde_json::json!({"text":"fifteen"}),
        )]),
        ModelResponse::text("all done"),
    ]));
    let agent = Agent::builder()
        .model(model.clone())
        .tools(default_registry())
        .guards(GuardPipeline::new())
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("inv");
    agent
        .run_turn(&mut s, "please add 7 and 8 then echo")
        .await
        .unwrap();

    let log = s.log.events();

    // 对模型的每一次调用、看到的每条消息，验证日志中存在其来源事件
    for i in 0..model.calls_seen() {
        let ctx = model.nth_context(i).unwrap();
        for msg in &ctx.messages {
            assert!(
                message_has_source(msg, log),
                "model-visible message {msg:?} (call #{i}) has NO logged source — invariant violated"
            );
        }
    }
}

/// 日志中是否存在某条模型可见消息的来源事件。
fn message_has_source(msg: &ModelMessage, log: &[SessionEvent]) -> bool {
    log.iter().any(|e| match (msg, &e.kind) {
        (ModelMessage::User { text }, EventKind::UserMessage { text: t }) => text == t,
        (
            ModelMessage::Assistant { text, tool_calls },
            EventKind::ModelMessage {
                text: lt,
                tool_calls: ltc,
            },
        ) => text == lt && tool_calls == ltc,
        (
            ModelMessage::Tool { call_id, output },
            EventKind::ToolResult {
                call_id: lc,
                output: lo,
                ..
            },
        ) => call_id == lc && output == lo,
        _ => false,
    })
}

/// 反向：日志里"模型可见"类事件的条数，应与最后一次上下文的消息条数一致（不多记、不漏记）。
#[tokio::test]
async fn context_size_matches_logged_visible_events() {
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "add",
            serde_json::json!({"a":1,"b":1}),
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
    let mut s = Session::new("inv2");
    agent.run_turn(&mut s, "add 1 1").await.unwrap();

    // 最后一次上下文（收尾 step）能看到：user + assistant(step1) + tool_result。
    let last = model.last_context().unwrap();
    let visible_in_log = s.log.count(|k| {
        matches!(
            k,
            EventKind::UserMessage { .. }
                | EventKind::ModelMessage { .. }
                | EventKind::ToolResult { .. }
        )
    });
    // 收尾 step 的模型输出发生在这次 complete() **之后**，故日志比该上下文多 1 条 ModelMessage。
    assert_eq!(
        last.messages.len() + 1,
        visible_in_log,
        "visible-event count must equal context size + the trailing model output"
    );
}

/// append-only：seq 单调递增、连续，时间戳不减。
#[tokio::test]
async fn log_is_append_only_monotonic() {
    let agent = Agent::builder()
        .model(Arc::new(MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id(
                "c1",
                "clock",
                serde_json::json!({}),
            )]),
            ModelResponse::text("ok"),
        ])))
        .tools(default_registry())
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("append");
    agent.run_turn(&mut s, "time?").await.unwrap();

    let evs = s.log.events();
    assert!(!evs.is_empty());
    for (i, e) in evs.iter().enumerate() {
        assert_eq!(e.seq, (i as u64) + 1, "seq must be 1-based contiguous");
        if i > 0 {
            assert!(e.ts >= evs[i - 1].ts, "timestamps must be non-decreasing");
        }
    }
}

/// 首事件必为 TurnStarted，尾事件必为 TurnEnded（回合边界完整）。
#[tokio::test]
async fn turn_is_bracketed_by_start_and_end() {
    let agent = Agent::builder()
        .model(Arc::new(MockModel::saying("hi")))
        .tools(default_registry())
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("bracket");
    agent.run_turn(&mut s, "yo").await.unwrap();
    let evs = s.log.events();
    assert!(matches!(
        evs.first().unwrap().kind,
        EventKind::TurnStarted { turn: 1, .. }
    ));
    assert!(matches!(
        evs.last().unwrap().kind,
        EventKind::TurnEnded { turn: 1, .. }
    ));
}

/// 会话事件可完整序列化再反序列化（回放/持久化前提）。
#[tokio::test]
async fn events_roundtrip_through_json() {
    let agent = Agent::builder()
        .model(Arc::new(MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id(
                "c1",
                "add",
                serde_json::json!({"a":2,"b":2}),
            )]),
            ModelResponse::text("four"),
        ])))
        .tools(default_registry())
        .approver(Arc::new(AutoApprover::approve()))
        .policy(Policy::default())
        .build()
        .unwrap();
    let mut s = Session::new("roundtrip");
    agent.run_turn(&mut s, "2+2").await.unwrap();

    for e in s.log.events() {
        let json = serde_json::to_string(e).expect("serialize");
        let back: SessionEvent = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(&back, e, "event must survive JSON roundtrip identically");
    }
}
