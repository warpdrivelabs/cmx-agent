//! `update_plan` —— 任务计划/待办跟踪（对齐 Codex update_plan）。让模型把多步任务显式列成清单，
//! 逐步推进（pending → in_progress → completed）。纯逻辑、无副作用；结果回灌供模型与前端展示。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

pub struct UpdatePlanTool;

const VALID: &[&str] = &["pending", "in_progress", "completed"];

fn icon(status: &str) -> &'static str {
    match status {
        "completed" => "✔",
        "in_progress" => "▸",
        _ => "○",
    }
}

#[async_trait]
impl Tool for UpdatePlanTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "update_plan",
            "登记/更新任务计划（步骤 + 状态 pending/in_progress/completed）；多步任务先列计划再执行",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "step": { "type": "string" },
                            "status": { "type": "string", "enum": ["pending","in_progress","completed"] }
                        },
                        "required": ["step"]
                    }
                },
                "note": { "type": "string" }
            },
            "required": ["steps"]
        }))
        .guard(GuardHints {
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(steps) = input.get("steps").and_then(|v| v.as_array()) else {
            return Ok(ToolResult::err("update_plan: 'steps' 数组必填"));
        };
        if steps.is_empty() {
            return Ok(ToolResult::err("update_plan: steps 不能为空"));
        }
        let mut norm = Vec::new();
        let mut in_progress = 0;
        let mut rendered = Vec::new();
        for (i, s) in steps.iter().enumerate() {
            let Some(text) = s.get("step").and_then(|v| v.as_str()) else {
                return Ok(ToolResult::err(format!("update_plan: 第 {} 步缺少 step 文本", i + 1)));
            };
            let status = s.get("status").and_then(|v| v.as_str()).unwrap_or("pending");
            if !VALID.contains(&status) {
                return Ok(ToolResult::err(format!(
                    "update_plan: 非法 status '{status}'（应为 {VALID:?}）"
                )));
            }
            if status == "in_progress" {
                in_progress += 1;
            }
            rendered.push(format!("{} {}", icon(status), text));
            norm.push(json!({"step": text, "status": status}));
        }
        // Codex 约定：至多一个 in_progress（软校验，仅提示不拒绝）
        let warn = if in_progress > 1 {
            Some(format!("建议同一时刻至多一个 in_progress（当前 {in_progress} 个）"))
        } else {
            None
        };
        let completed = norm.iter().filter(|s| s["status"] == "completed").count();
        Ok(ToolResult::ok(json!({
            "steps": norm,
            "total": norm.len(),
            "completed": completed,
            "checklist": rendered.join("\n"),
            "note": input.get("note").and_then(|v| v.as_str()),
            "warning": warn,
        })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx(roots: &[PathBuf]) -> ToolCtx<'_> {
        ToolCtx { workspace_roots: roots, session_id: "test" }
    }

    #[tokio::test]
    async fn records_plan_with_progress() {
        let roots: Vec<PathBuf> = vec![];
        let r = UpdatePlanTool
            .invoke(
                json!({"steps":[
                    {"step":"读需求","status":"completed"},
                    {"step":"写代码","status":"in_progress"},
                    {"step":"跑测试","status":"pending"}
                ]}),
                &ctx(&roots),
            )
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["total"], 3);
        assert_eq!(r.output["completed"], 1);
        assert!(r.output["checklist"].as_str().unwrap().contains("▸ 写代码"));
        assert!(r.output["warning"].is_null());
    }

    #[tokio::test]
    async fn rejects_bad_status() {
        let roots: Vec<PathBuf> = vec![];
        let r = UpdatePlanTool
            .invoke(json!({"steps":[{"step":"x","status":"done"}]}), &ctx(&roots))
            .await
            .unwrap();
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn warns_multiple_in_progress() {
        let roots: Vec<PathBuf> = vec![];
        let r = UpdatePlanTool
            .invoke(
                json!({"steps":[
                    {"step":"a","status":"in_progress"},
                    {"step":"b","status":"in_progress"}
                ]}),
                &ctx(&roots),
            )
            .await
            .unwrap();
        assert!(r.ok);
        assert!(!r.output["warning"].is_null());
    }
}
