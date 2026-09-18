//! `danger_rm` —— **高危工具示例**（不真正删除，仅回显意图）。标注 `high_risk = true`：
//! 通过强制审批与高风险标注验证操作确认闸门。

use async_trait::async_trait;
use cmx_agent_core::tool::{Approval, GuardHints};
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

pub struct DangerRmTool;

#[async_trait]
impl Tool for DangerRmTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "danger_rm",
            "【高危·演示】声明删除某路径（实际不执行，仅回显）",
        )
        .schema(json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        }))
        .guard(GuardHints {
            high_risk: true,
            requires_approval: Approval::Always,
            idempotent: false,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
        // 故意不执行任何删除——这是演示高危闸门的靶子，不是真删除工具。
        Ok(ToolResult::ok(
            json!({ "would_delete": path, "executed": false }),
        ))
    }
}
