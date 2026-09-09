//! 演示模型 [`DemoModel`]：一个**关键词路由**的 `ModelSeam`，用于 M1/M2 无真实大模型时的桌面演示。
//!
//! 它不是真模型——它读最后一条用户消息，按关键词决定：调哪个工具、还是纯文本回话。让「同核多壳 +
//! 真实 cmx 连接器」在窗口里肉眼可见地跑通（输入「列出流程定义」真的打 :8091 flow 引擎）。
//!
//! 真实大模型接入时，用一个基于 HTTP 的 `ModelSeam` 实现替换本类型即可，壳与前端不变。

use async_trait::async_trait;
use cmx_agent_core::ToolCall;
use cmx_agent_core::model::{ModelContext, ModelError, ModelMessage, ModelResponse, ModelSeam};

/// 关键词路由演示模型。两步交互：第一步据用户输入产工具调用（或纯文本）；看到工具结果后收尾。
pub struct DemoModel;

impl DemoModel {
    /// 取最后一条用户消息文本。
    fn last_user(ctx: &ModelContext) -> String {
        ctx.messages
            .iter()
            .rev()
            .find_map(|m| match m {
                ModelMessage::User { text } => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// 本回合是否已经有工具结果了（有 → 该收尾）。
    fn has_tool_result(ctx: &ModelContext) -> bool {
        ctx.messages
            .iter()
            .any(|m| matches!(m, ModelMessage::Tool { .. }))
    }

    /// 上一条 assistant 是否请求过工具（配合 has_tool_result 判断阶段）。
    fn tool_result_text(ctx: &ModelContext) -> Option<String> {
        ctx.messages.iter().rev().find_map(|m| match m {
            ModelMessage::Tool { output, .. } => Some(output.to_string()),
            _ => None,
        })
    }
}

#[async_trait]
impl ModelSeam for DemoModel {
    async fn complete(&self, ctx: &ModelContext) -> Result<ModelResponse, ModelError> {
        // 阶段二：已有工具结果 → 用一句话把结果说清并收尾。
        if Self::has_tool_result(ctx) {
            let raw = Self::tool_result_text(ctx).unwrap_or_default();
            let summary = summarize_tool_result(&raw);
            return Ok(ModelResponse::text(summary));
        }

        // 阶段一：据用户输入决定调哪个工具。
        let q = Self::last_user(ctx);
        let call = route(&q);
        match call {
            Some((id, name, input, preface)) => Ok(ModelResponse::calls(vec![ToolCall::with_id(
                id, name, input,
            )])
            .with_text(preface)),
            None => Ok(ModelResponse::text(format!(
                "我是 TrueMate（cmx 企业桌面智能体，演示模型）。你说的是「{q}」。\n\
                 试试：「列出所有流程定义」调 cmx-flow，「列出对象类型」调 cmx-ontology，「算 2 加 3」用内置工具。\n\
                 （真实大模型将在后续接入，届时可自然对话。）"
            ))),
        }
    }
}

/// 关键词 → (call_id, 工具名, 入参, 前置文本)。
fn route(q: &str) -> Option<(&'static str, &'static str, serde_json::Value, &'static str)> {
    let has = |kws: &[&str]| kws.iter().any(|k| q.contains(k));

    if has(&["流程定义", "流程", "审批流", "工作流", "definition"]) {
        Some((
            "call-flow",
            "flow_list_definitions",
            serde_json::json!({}),
            "好的，我去问 cmx-flow 流程引擎……",
        ))
    } else if has(&["对象类型", "对象", "本体", "实体", "object"]) {
        Some((
            "call-onto",
            "onto_list_object_types",
            serde_json::json!({}),
            "好的，我去问 cmx-ontology 本体平台……",
        ))
    } else if has(&["报表", "财报", "report"]) {
        Some((
            "call-report",
            "report_list_reports",
            serde_json::json!({}),
            "好的，我去问 cmx-report 报表平台……",
        ))
    } else if has(&["加", "相加", "求和", "+", "add"]) {
        // 从文本里粗略抽两个数字；抽不到就用 2、3。
        let nums: Vec<f64> = q
            .split(|c: char| !c.is_ascii_digit() && c != '.')
            .filter_map(|s| s.parse::<f64>().ok())
            .collect();
        let a = nums.first().copied().unwrap_or(2.0);
        let b = nums.get(1).copied().unwrap_or(3.0);
        Some((
            "call-add",
            "add",
            serde_json::json!({ "a": a, "b": b }),
            "我来算一下……",
        ))
    } else if has(&["几点", "时间", "现在", "clock", "time"]) {
        Some((
            "call-clock",
            "clock",
            serde_json::json!({}),
            "让我看看现在几点……",
        ))
    } else {
        None
    }
}

/// 把工具结果 JSON 摘要成一句自然语言（演示用；真实模型会自己组织语言）。
fn summarize_tool_result(raw: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) else {
        return format!("工具返回：{raw}");
    };
    // 连接器返回带 service/count 字段
    if let Some(svc) = v.get("service").and_then(|s| s.as_str()) {
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return format!("调用 {svc} 失败：{err}");
        }
        let count = v.get("count").and_then(|c| c.as_u64());
        // flow definitions
        if let Some(defs) = v.get("definitions").and_then(|d| d.as_array()) {
            let names: Vec<String> = defs
                .iter()
                .filter_map(|d| {
                    d.get("name")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            return format!(
                "✅ cmx-flow 流程引擎里有 {} 个流程定义：{}。",
                count.unwrap_or(names.len() as u64),
                if names.is_empty() {
                    "（无）".into()
                } else {
                    names.join("、")
                }
            );
        }
        // onto object types
        if let Some(ots) = v.get("objectTypes").and_then(|d| d.as_array()) {
            let names: Vec<String> = ots
                .iter()
                .filter_map(|o| {
                    o.get("displayName")
                        .and_then(|n| n.as_str())
                        .map(|s| s.to_string())
                })
                .collect();
            return format!(
                "✅ cmx-ontology 本体平台里有 {} 个对象类型：{}。",
                count.unwrap_or(names.len() as u64),
                if names.is_empty() {
                    "（无）".into()
                } else {
                    names.join("、")
                }
            );
        }
        return format!("✅ {svc} 返回了 {} 条数据。", count.unwrap_or(0));
    }
    // add / clock 等内置工具
    if let Some(sum) = v.get("sum") {
        return format!("✅ 计算结果：{sum}。");
    }
    if let Some(now) = v.get("now").and_then(|n| n.as_str()) {
        return format!("✅ 现在是 {now}。");
    }
    format!("✅ 完成。工具返回：{raw}")
}
