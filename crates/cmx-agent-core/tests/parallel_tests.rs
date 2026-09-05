//! 真并行验证：一步内的多个工具调用**并发执行**——总耗时≈最慢者，而非逐个累加。
//!
//! 用一个「睡眠工具」把耗时具象化：一步发起 N 个 `sleep(MS)`。
//! - 串行（旧实现）：总耗时 ≈ N×MS。
//! - 并发（`join_all`）：总耗时 ≈ MS（所有 sleep 计时器同时武装、同时触发）。
//!
//! 注意：`sleep` 是计时器型（不阻塞线程），故即便在 `#[tokio::test]` 默认的 current-thread
//! 运行时上，并发也成立——这正说明「真并行」对 I/O / 计时型工具（如网络型子智能体）的价值。

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use cmx_agent_core::event::{EventKind, StopReason};
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{
    Agent, GuardPipeline, MockModel, ModelResponse, Policy, SandboxMode, Session, Tool, ToolCall,
    ToolCtx, ToolError, ToolRegistry, ToolResult, ToolSpec,
};
use serde_json::{Value, json};

/// 睡眠工具：`invoke` 挂起 `ms` 毫秒后返回。幂等（无写副作用），可安全并发。
struct SleepTool {
    ms: u64,
}

#[async_trait]
impl Tool for SleepTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("sleep", "睡眠指定毫秒（测试并发用）")
            .schema(json!({"type":"object","properties":{"ms":{"type":"integer"}}}))
            .guard(GuardHints {
                idempotent: true,
                ..Default::default()
            })
    }
    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        tokio::time::sleep(Duration::from_millis(self.ms)).await;
        Ok(ToolResult::ok(json!({ "slept_ms": self.ms })))
    }
}

#[tokio::test]
async fn multiple_tool_calls_run_concurrently() {
    const MS: u64 = 200;
    const N: usize = 4;

    // 一步内发起 N 个 sleep(MS)。脚本耗尽后 fallback=text("done") 让父回合收尾。
    let calls: Vec<ToolCall> = (0..N)
        .map(|i| ToolCall::with_id(format!("c{i}"), "sleep", json!({ "ms": MS })))
        .collect();
    let model = MockModel::new([ModelResponse::calls(calls)]);

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(SleepTool { ms: MS }));
    let agent = Agent::builder()
        .model(Arc::new(model))
        .tools(reg)
        .guards(GuardPipeline::new())
        .policy(Policy {
            sandbox: SandboxMode::WorkspaceWrite,
            ..Default::default()
        })
        .build()
        .unwrap();

    let mut s = Session::new("par");
    let t0 = Instant::now();
    let out = agent.run_turn(&mut s, "并发睡眠").await.unwrap();
    let elapsed = t0.elapsed();

    assert_eq!(out.reason, StopReason::Completed);

    // N 个 sleep 结果都应落库且成功。
    let ok_sleeps = s
        .log
        .iter()
        .filter(|e| matches!(&e.kind, EventKind::ToolResult { ok, .. } if *ok))
        .count();
    assert_eq!(ok_sleeps, N, "{N} 个 sleep 都应成功");

    // 并发证据：串行需 ≥ N×MS = 800ms；并发 ≈ MS = 200ms。取 500ms 为界——
    // 既能证伪「串行」（800ms），又留足调度余量避免抖动误报。
    assert!(
        elapsed < Duration::from_millis(500),
        "一步内 {N} 个工具调用应并发执行：实际耗时 {elapsed:?}，串行将需 ~{}ms",
        MS * N as u64
    );
}
