//! `add` —— 数值相加：`{a, b} -> {sum}`。演示确定性计算 + 输入校验（缺参返回错误结果而非 panic）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

pub struct AddTool;

#[async_trait]
impl Tool for AddTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("add", "返回 a + b")
            .schema(json!({
                "type": "object",
                "properties": { "a": { "type": "number" }, "b": { "type": "number" } },
                "required": ["a", "b"]
            }))
            .guard(GuardHints {
                idempotent: true,
                ..Default::default()
            })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let a = input.get("a").and_then(|v| v.as_f64());
        let b = input.get("b").and_then(|v| v.as_f64());
        match (a, b) {
            (Some(a), Some(b)) => Ok(ToolResult::ok(json!({ "sum": a + b }))),
            _ => Ok(ToolResult::err("add: 'a' and 'b' must both be numbers")),
        }
    }
}
