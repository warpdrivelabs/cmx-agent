//! `echo` —— 最简工具：原样返回 `text`。无鉴权、无审批、幂等。用于内核冒烟。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("echo", "原样返回输入的 text 字段")
            .schema(json!({
                "type": "object",
                "properties": { "text": { "type": "string" } },
                "required": ["text"]
            }))
            .guard(GuardHints {
                idempotent: true,
                ..Default::default()
            })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let text = input.get("text").and_then(|v| v.as_str()).unwrap_or("");
        Ok(ToolResult::ok(json!({ "text": text })))
    }
}
