//! 计划模式退出批准（方案 20260914 §7.4，阶段二）。模型在计划模式下完成调研后调 `exit_plan`
//! 提交结构化计划，内核经 [`crate::question::QuestionService`] 弹答题卡问「是否批准并开始实现」；
//! 用户批准 → 本回合 [`super::agent::TURN_PLAN_MODE`] 置 false（同回合下一批工具调用立即放行写）
//! + 计划文本落盘；忽略/超时/继续调整 → 保持计划模式。
//!
//! 实现形态对齐 ask_user 实装（907f972）：**内核托管** `user_interactive` 工具——本模块只声明
//! spec 与判定逻辑，挂起/事件/回灌由 `agent.rs` 托管路径处理；`Tool::invoke` 正常不可达。
//! 注意：user_interactive 工具**先过守卫管道再进内核特判**——`exit_plan` 必须在计划模式
//! 白名单（`guard::PLAN_READ_TOOLS`）内，否则计划模式里连退出批准一并被拦死。

use std::path::PathBuf;

use serde_json::Value;

use crate::question::{AskOption, AskQuestion, QuestionOutcome};
use crate::tool::{Approval, GuardHints, Tool, ToolCtx, ToolError, ToolResult, ToolSpec};

/// `exit_plan` 工具名（内核托管特判键；工具注册表内唯一）。
pub const EXIT_PLAN_TOOL_NAME: &str = "exit_plan";

/// 批准选项 label（首位推荐）。判定依据：答案首项 == 此 label 且不含 `user_note:` 项。
pub const APPROVE_LABEL: &str = "批准并开始实现 (Recommended)";
/// 继续调整选项 label。
pub const CONTINUE_LABEL: &str = "继续调整计划";

/// 计划文本硬上限（防超长直落事件流与磁盘）。
const MAX_PLAN_CHARS: usize = 200_000;

/// 固定审批一问（对齐 ask_user 卡片形态；单选）。
pub fn approval_question() -> Vec<AskQuestion> {
    vec![AskQuestion {
        id: "plan_approval".into(),
        header: "计划审批".into(),
        question: "计划已就绪，是否批准并开始实现？".into(),
        options: vec![
            AskOption {
                label: APPROVE_LABEL.into(),
                description: "退出计划模式，授权按计划实施（可写文件）".into(),
            },
            AskOption {
                label: CONTINUE_LABEL.into(),
                description: "保持计划模式，继续补充调研、调整计划".into(),
            },
        ],
        multiple: false,
    }]
}

/// 从模型入参提取计划文本（trim；空报错；超长截断）。
pub fn normalize_plan_input(input: &Value) -> Result<String, String> {
    let plan = input
        .get("plan")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .unwrap_or("");
    if plan.is_empty() {
        return Err("缺少 plan 字段（提交完整的结构化计划文本）".into());
    }
    Ok(plan.chars().take(MAX_PLAN_CHARS).collect())
}

/// 答案判定（§7.4 定稿规则）：`plan_approval` 首项 == 批准 label **且不含 `user_note:` 项**；
/// 选批准又附言 → 按「继续调整」处理附言带回（红队 P2-3 + N8）。
pub fn is_approval_answer(map: &serde_json::Map<String, Value>) -> bool {
    let Some(arr) = map.get("plan_approval").and_then(|v| v.as_array()) else {
        return false;
    };
    if arr.is_empty() {
        return false;
    }
    let first = arr[0].as_str().unwrap_or("");
    let has_note = arr
        .iter()
        .any(|v| v.as_str().map(|s| s.starts_with("user_note:")).unwrap_or(false));
    first == APPROVE_LABEL && !has_note
}

/// 从答案里抽附言（「批准+附言」按继续调整处理时带回）。
fn extract_note(map: &serde_json::Map<String, Value>) -> Option<String> {
    map.get("plan_approval")?
        .as_array()?
        .iter()
        .find_map(|v| {
            let s = v.as_str()?;
            s.strip_prefix("user_note:").map(str::trim).map(String::from)
        })
        .filter(|n| !n.is_empty())
}

/// 组 exit_plan 的 tool result（回灌给模型）：
/// - 批准：`{approved:true}` + 引导开工（计划文本已由内核落盘）；
/// - 继续调整（含批准+附言）：`{approved:false}` + 用户附言；
/// - 忽略/超时（dismissed）：`{approved:false, dismissed:true}` —— 不开工、继续等指示。
pub fn compose_result(
    outcome: &QuestionOutcome,
    saved_path: Option<&PathBuf>,
) -> ToolResult {
    match outcome {
        QuestionOutcome::Answered(map) => {
            if is_approval_answer(map) {
                let saved = saved_path
                    .map(|p| p.display().to_string())
                    .unwrap_or_default();
                ToolResult::ok(serde_json::json!({
                    "approved": true,
                    "plan_saved": saved_path.is_some(),
                    "plan_path": saved,
                    "output": format!(
                        "计划已批准{}。现在开始按计划实现：先按计划第一步动手，逐步推进并在每步后自我校验。",
                        saved_path.map(|p| format!("，计划存于 {}", p.display())).unwrap_or_default()
                    ),
                }))
            } else {
                let note = extract_note(map)
                    .map(|n| format!("用户附言：「{n}」。"))
                    .unwrap_or_else(|| "用户选择继续调整计划。".into());
                ToolResult::ok(serde_json::json!({
                    "approved": false,
                    "output": format!("{note}请按用户意见调整后再次调用 exit_plan 提交；计划模式保持开启（写操作仍被拒绝）。"),
                }))
            }
        }
        QuestionOutcome::Dismissed(by) => ToolResult::ok(serde_json::json!({
            "approved": false,
            "dismissed": true,
            "output": format!(
                "计划审批未被回应（{by}）。请继续调整计划或停下等待用户指示；\
                 计划模式保持开启，不要开始修改任何文件。"
            ),
        })),
    }
}

/// 计划文本落盘 `<roots[0]>/.cmx/plans/<unix_secs>.md`。失败不阻塞（None），计划仍在会话内。
pub fn save_plan(plan: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let root = roots.first()?;
    let dir = root.join(".cmx").join("plans");
    std::fs::create_dir_all(&dir).ok()?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = dir.join(format!("{ts}.md"));
    std::fs::write(&path, plan).ok()?;
    Some(path)
}

/// `exit_plan` 工具对象：只声明 spec（进模型工具清单），执行由内核托管（同 ask_user）。
pub struct ExitPlanTool;

const EXIT_PLAN_DESCRIPTION: &str = "提交当前计划并请求用户批准（**仅计划模式下可用**）。在计划模式完成充分调研后调用：\
plan 传完整的结构化计划文本（markdown，含目标 / 步骤 / 涉及文件 / 风险 / 验证方式）。\
用户批准后本回合即可写文件、按计划实施；未批准则继续调整。非计划模式下调用会报错。";

fn exit_plan_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "plan": {
                "type": "string",
                "description": "完整的结构化计划文本（markdown）：目标 / 步骤 / 涉及文件 / 风险 / 验证方式"
            }
        },
        "required": ["plan"]
    })
}

#[async_trait::async_trait]
impl Tool for ExitPlanTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(EXIT_PLAN_TOOL_NAME, EXIT_PLAN_DESCRIPTION)
            .schema(exit_plan_schema())
            .guard(GuardHints {
                // 只读无副作用（写盘由内核在批准后做，与工具执行解耦）：never + 幂等。
                requires_approval: Approval::Never,
                idempotent: true,
                ..GuardHints::default()
            })
            .user_interactive()
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::err(
            "exit_plan is managed by the kernel (user_interactive); this fallback should not be reached",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn answered(labels: &[&str]) -> serde_json::Map<String, Value> {
        let mut m = serde_json::Map::new();
        m.insert(
            "plan_approval".into(),
            json!(labels.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
        );
        m
    }

    #[test]
    fn approval_answer_rules() {
        assert!(is_approval_answer(&answered(&[APPROVE_LABEL])));
        assert!(!is_approval_answer(&answered(&[CONTINUE_LABEL])));
        // 批准 + 附言 → 不算批准
        assert!(!is_approval_answer(&answered(&[APPROVE_LABEL, "user_note: 先补风险"])));
        // 空答案不算
        assert!(!is_approval_answer(&answered(&[])));
        // 缺键不算
        assert!(!is_approval_answer(&serde_json::Map::new()));
    }

    #[test]
    fn compose_result_branches() {
        let approved = compose_result(
            &QuestionOutcome::Answered(answered(&[APPROVE_LABEL])),
            None,
        );
        assert_eq!(approved.output["approved"], json!(true));
        assert!(approved.output["output"].as_str().unwrap().contains("按计划实现"));

        let note = compose_result(
            &QuestionOutcome::Answered(answered(&[APPROVE_LABEL, "user_note: 先补风险清单"])),
            None,
        );
        assert_eq!(note.output["approved"], json!(false));
        assert!(note.output["output"].as_str().unwrap().contains("先补风险清单"));

        let cont = compose_result(
            &QuestionOutcome::Answered(answered(&[CONTINUE_LABEL])),
            None,
        );
        assert_eq!(cont.output["approved"], json!(false));

        let dismissed = compose_result(&QuestionOutcome::Dismissed("timeout"), None);
        assert_eq!(dismissed.output["dismissed"], json!(true));
        assert!(dismissed.output["output"].as_str().unwrap().contains("timeout"));
    }

    #[test]
    fn normalize_and_save() {
        assert!(normalize_plan_input(&json!({})).is_err());
        assert!(normalize_plan_input(&json!({"plan": "  "})).is_err());
        let p = normalize_plan_input(&json!({"plan": " # 计划\n1. x"})).unwrap();
        assert!(p.starts_with("# 计划"));

        let tmp = std::env::temp_dir().join(format!("cmx-exit-plan-test-{}", std::process::id()));
        let roots = [tmp.clone()];
        let saved = save_plan("hello", &roots).expect("saved");
        assert!(saved.starts_with(tmp.join(".cmx").join("plans")));
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "hello");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn exit_plan_tool_spec_is_readonly_and_interactive() {
        let spec = ExitPlanTool.spec();
        assert_eq!(spec.name, EXIT_PLAN_TOOL_NAME);
        assert!(spec.user_interactive);
        assert_eq!(spec.guard.requires_approval, Approval::Never);
        assert!(spec.guard.idempotent);
    }
}
