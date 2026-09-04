//! `clock` —— 返回当前时间（ISO8601）。可注入固定时钟 → 确定性测试（对齐 flowengine 的可注入时钟做法）。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

/// 时钟工具。`fixed=Some(..)` 时永远返回该时刻（测试用）。
pub struct ClockTool {
    fixed: Option<DateTime<Utc>>,
}

impl ClockTool {
    pub fn system() -> Self {
        Self { fixed: None }
    }

    pub fn fixed(t: DateTime<Utc>) -> Self {
        Self { fixed: Some(t) }
    }
}

#[async_trait]
impl Tool for ClockTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("clock", "返回当前 UTC 时间（ISO8601）").guard(GuardHints {
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let now = self.fixed.unwrap_or_else(Utc::now);
        Ok(ToolResult::ok(json!({ "now": now.to_rfc3339() })))
    }
}
