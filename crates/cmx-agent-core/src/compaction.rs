//! 压缩内核：头部序列化 / 结构化摘要提示词 / prune 选择（压缩方案 §4.2.4–§4.2.5）。
//!
//! 纯函数层：只操作事件切片与字符串，不碰模型、不碰日志——编排（何时压缩、请求怎么发）
//! 在 app 层。参考实现：opencode `packages/core/src/session/compaction.ts`（模板/合并指令/常量）
//! 与 codex `core/src/compact.rs`（摘要请求自溢出逐条丢弃）。

use std::collections::HashSet;

use crate::event::{EventKind, SessionEvent};
use crate::token::est_tokens;

/// 单个工具输出在序列化里的截断长度（opencode TOOL_OUTPUT_MAX_CHARS）。
pub const TOOL_OUTPUT_MAX_CHARS: usize = 2_000;
/// 摘要请求的输出上限。
pub const SUMMARY_MAX_TOKENS: u64 = 4_096;
/// prune：从尾往前保护最近这么多 token 的工具输出。
pub const PRUNE_PROTECT: u64 = 40_000;
/// prune：可清理量不足此数不动手（不值为一次占位改写）。
pub const PRUNE_MINIMUM: u64 = 20_000;

/// 工具输出值的文本化（String 原样，其余紧凑 JSON）。
fn output_text(output: &serde_json::Value) -> String {
    match output {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 超长截断（带 `[已截断]` 尾标，opencode 同款）。
fn truncate_chars(s: &str) -> String {
    if s.chars().count() <= TOOL_OUTPUT_MAX_CHARS {
        s.to_string()
    } else {
        let head: String = s.chars().take(TOOL_OUTPUT_MAX_CHARS).collect();
        format!("{head}\n[已截断]")
    }
}

/// 把事件切片逐条序列化为纯文本段（每事件一段；压缩方案 §4.2.4.1）。
///
/// 可见三类 + 推理参与（opencode 两代一致，利于摘要质量）；守卫裁决/审批/提问/注记等
/// 审计事件不进摘要。附件类内容当前无独立事件形态，图片等以工具输出自然进入
/// `[工具结果]`；为在途的图片粘贴方案预留 `[附件 mime: 名]` 占位格式。
pub fn serialize_entries(events: &[SessionEvent]) -> Vec<String> {
    let mut out = Vec::new();
    for ev in events {
        match &ev.kind {
            EventKind::UserMessage { text } => {
                out.push(format!("[用户]: {text}"));
            }
            EventKind::ModelMessage { text, tool_calls } => {
                if let Some(t) = text
                    && !t.is_empty()
                {
                    out.push(format!("[助手]: {t}"));
                }
                for c in tool_calls {
                    let args = c.input.to_string();
                    out.push(format!("[助手 工具调用]: {}({args})", c.name));
                }
            }
            EventKind::ToolResult { call_id: _, ok, output } => {
                let body = truncate_chars(&output_text(output));
                if *ok {
                    out.push(format!("[工具结果]: {body}"));
                } else {
                    out.push(format!("[工具错误]: {body}"));
                }
            }
            EventKind::Reasoning { text } if !text.trim().is_empty() => {
                out.push(format!("[推理]: {}", text.trim()));
            }
            _ => {}
        }
    }
    out
}

/// 自溢出防护（codex `remove_first_item` 同款）：条目总量超出预算时从最旧条目逐条丢弃，
/// 返回（保留下来的文本, 丢弃条数）。
pub fn fit_entries(entries: &[String], budget_tokens: u64) -> (String, usize) {
    let ests: Vec<u64> = entries.iter().map(|e| est_tokens(e)).collect();
    let mut total: u64 = ests.iter().sum();
    let mut drop = 0usize;
    while total > budget_tokens && drop < entries.len() {
        total -= ests[drop];
        drop += 1;
    }
    let kept = entries[drop..].join("\n\n");
    (kept, drop)
}

/// 结构化摘要提示词（中文模板 + 规则 + 可选合并指令 + 可选用户关注点）。
pub fn summary_prompt(prior: Option<&str>, conversation: &str, focus: Option<&str>) -> String {
    let mut p = String::new();
    p.push_str("以下是迄今的对话记录：\n\n<conversation>\n");
    p.push_str(conversation);
    p.push_str("\n</conversation>\n\n");
    if let Some(prior) = prior.filter(|s| !s.trim().is_empty()) {
        p.push_str("以下是更早会话的既有摘要：\n\n<prior-summary>\n");
        p.push_str(prior);
        p.push_str("\n</prior-summary>\n\n");
        p.push_str(
            "请把既有摘要与上方对话合并为一份新摘要。合并规则：\n\
             - 既有摘要里的目标、约束、用户指示、已做决策、并行工作线，即使对话未提及也要延续保留；只删「已完成且不再需要」的条目。\n\
             - 对话比摘要更新：冲突时以对话为准，写出纠正后的事实、舍弃旧说法。\n\
             - 把对话中的新进展、新决策、新约束补入对应小节。\n\
             - 已完成的工作从「进行中」迁入「已完成」。\n\
             - 受阻项已解除则更新，但仍需保留继续工作所需的细节。\n\
             - 「目标」与「下一步」随当前工作状态刷新。\n\n",
        );
    }
    p.push_str(
        "请基于以上内容为另一个助手生成一份交接摘要，使它能无缝接续工作。严格按以下 Markdown 结构输出（保留全部小节与顺序，不要输出本说明）：\n\n\
         ## 目标\n- （一两句话说明用户要完成什么；无则写（无））\n\n\
         ## 重要细节\n- （约束/偏好、已做决策及原因、关键事实/假设、继续所需的精确上下文；无则写（无））\n\n\
         ## 工作状态\n### 已完成\n- （已完成并验证的工作；无则写（无））\n\n### 进行中\n- （当前工作/半成品/调查状态；无则写（无））\n\n### 受阻\n- （阻塞项/失败命令/未知项；无则写（无））\n\n\
         ## 下一步\n1. （下一个具体动作；无则写（无））\n2. （再下一步；无则写（无））\n\n\
         ## 相关文件\n- （文件或目录路径：为何重要；无则写（无））\n\n\
         规则：\n\
         - 每节必须保留，即使为空也写「（无）」。\n\
         - 用简洁要点，不写长段落。\n\
         - 精确保留文件路径、符号、命令、错误串、URL、标识符。\n\
         - 绝不提及摘要或压缩过程本身。\n",
    );
    if let Some(f) = focus.filter(|s| !s.trim().is_empty()) {
        p.push_str(&format!(
            "\n用户特别要求：压缩时注意保留与以下关注点相关的内容：{f}\n"
        ));
    }
    p
}

/// prune 选择（§4.2.5）：返回本批应清理的 tool call id。
///
/// 三守卫（opencode compaction.ts:288-305 对齐）：① 跳过最近一个回合（回合内容在边界事件
/// 之前到达——逆向遍历先见内容后见 `TurnStarted`，故最新回合内容 `turns==0`、更早的
/// `turns>=1`；总回合数 <2 时无可保护对照、什么都不剪）；② 只考虑最后压缩点（最后一条
/// `Compacted` 的 up_to_seq）之后的事件；③ 已在清理名单的不再重复登记。skill 类工具
/// （技能加载）豁免——技能体系 v2 引入 skill 工具时在此追加豁免名单。
pub fn select_prune(events: &[SessionEvent], already_pruned: &HashSet<String>) -> Vec<String> {
    // 最后压缩点
    let cutoff = events
        .iter()
        .rev()
        .find_map(|e| match &e.kind {
            EventKind::Compacted { up_to_seq, .. } => Some(*up_to_seq),
            _ => None,
        })
        .unwrap_or(0);

    let mut turns = 0usize;
    let mut protected: u64 = 0;
    let mut candidates: Vec<(String, u64)> = Vec::new();
    for ev in events.iter().rev() {
        if ev.seq <= cutoff {
            break; // 守卫②
        }
        match &ev.kind {
            EventKind::TurnStarted { .. } => turns += 1,
            EventKind::ToolResult { call_id, ok: true, output } if turns >= 1 => {
                if already_pruned.contains(call_id) {
                    continue; // 守卫③
                }
                let t = est_tokens(&output_text(output));
                if protected + t <= PRUNE_PROTECT {
                    protected += t;
                } else {
                    candidates.push((call_id.clone(), t));
                }
            }
            _ => {}
        }
    }
    // 守卫①（总回合数≥2 才有「最新回合之外」可言）；可清理量不足不值为之
    let reclaimable: u64 = candidates.iter().map(|(_, t)| t).sum();
    if turns < 2 || reclaimable < PRUNE_MINIMUM {
        return Vec::new();
    }
    candidates.into_iter().map(|(id, _)| id).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{SessionLog, StopReason};
    use serde_json::json;

    fn log_with(events: impl FnOnce(&mut SessionLog)) -> Vec<SessionEvent> {
        let mut log = SessionLog::new();
        events(&mut log);
        log.events().to_vec()
    }

    #[test]
    fn serialize_covers_visible_kinds() {
        let evs = log_with(|log| {
            log.append(EventKind::UserMessage { text: "帮我查一下".into() });
            log.append(EventKind::Reasoning { text: "先想一步".into() });
            log.append(EventKind::ModelMessage {
                text: Some("好的".into()),
                tool_calls: vec![crate::tool::ToolCall::with_id("c1", "grep", json!({"q":"x"}))],
            });
            log.append(EventKind::ToolResult { call_id: "c1".into(), ok: true, output: json!("a\nb") });
            log.append(EventKind::Note { text: "审计不进".into() });
        });
        let entries = serialize_entries(&evs);
        let joined = entries.join("\n");
        assert!(joined.contains("[用户]: 帮我查一下"));
        assert!(joined.contains("[推理]: 先想一步"));
        assert!(joined.contains("[助手]: 好的"));
        assert!(joined.contains("[助手 工具调用]: grep({\"q\":\"x\"})"));
        assert!(joined.contains("[工具结果]: a\nb"));
        assert!(!joined.contains("审计不进"));
    }

    #[test]
    fn truncate_long_output() {
        let long = "x".repeat(3000);
        let t = truncate_chars(&long);
        assert!(t.chars().count() == TOOL_OUTPUT_MAX_CHARS + "\n[已截断]".chars().count());
    }

    #[test]
    fn fit_entries_drops_oldest() {
        let entries: Vec<String> = (0..5).map(|i| "a".repeat(400 * (i + 1))).collect();
        // 总量 = (400+800+1200+1600+2000)/4 = 1500 token；预算 1000 → 丢最旧 3 段后剩 900
        let (kept, dropped) = fit_entries(&entries, 1000);
        assert_eq!(dropped, 3);
        assert!(est_tokens(&kept) <= 1000);
    }

    #[test]
    fn prune_respects_guards() {
        let evs = log_with(|log| {
            log.append(EventKind::TurnStarted { turn: 1, user_input: "t1".into() });
            log.append(EventKind::ToolResult {
                call_id: "old".into(),
                ok: true,
                output: json!("y".repeat(200_000)), // 5 万 token 级
            });
            log.append(EventKind::TurnEnded { turn: 1, reason: StopReason::Completed, steps: 1, usage: None });
            log.append(EventKind::TurnStarted { turn: 2, user_input: "t2".into() });
            log.append(EventKind::ToolResult {
                call_id: "recent".into(),
                ok: true,
                output: json!("z".repeat(200_000)),
            });
            log.append(EventKind::TurnEnded { turn: 2, reason: StopReason::Completed, steps: 1, usage: None });
        });
        // 最新回合内的 recent 受守卫①保护；old 可清理且量足
        let ids = select_prune(&evs, &HashSet::new());
        assert_eq!(ids, vec!["old".to_string()]);
        // 已登记过 → 守卫③ 不再产出
        let mut done = HashSet::new();
        done.insert("old".to_string());
        assert!(select_prune(&evs, &done).is_empty());
        // 单回合（无 t2）→ 守卫① 不剪
        let evs1: Vec<_> = evs.iter().take(3).cloned().collect();
        assert!(select_prune(&evs1, &HashSet::new()).is_empty());
    }

    #[test]
    fn prune_skips_before_compacted_cutoff() {
        let evs = log_with(|log| {
            log.append(EventKind::TurnStarted { turn: 1, user_input: "t1".into() });
            log.append(EventKind::ToolResult {
                call_id: "ancient".into(),
                ok: true,
                output: json!("q".repeat(200_000)),
            });
            log.append(EventKind::TurnEnded { turn: 1, reason: StopReason::Completed, steps: 1, usage: None });
            log.append(EventKind::Compacted {
                up_to_seq: 3,
                summary: "旧摘要".into(),
                reason: crate::event::CompactionReason::AutoThreshold,
            });
            log.append(EventKind::TurnStarted { turn: 2, user_input: "t2".into() });
            log.append(EventKind::TurnEnded { turn: 2, reason: StopReason::Completed, steps: 1, usage: None });
            log.append(EventKind::TurnStarted { turn: 3, user_input: "t3".into() });
            log.append(EventKind::TurnEnded { turn: 3, reason: StopReason::Completed, steps: 1, usage: None });
        });
        // ancient 在压缩点之前（守卫②），其余回合无工具输出可剪 → 空
        assert!(select_prune(&evs, &HashSet::new()).is_empty());
    }

    #[test]
    fn prompt_includes_focus_and_prior() {
        let p = summary_prompt(Some("旧摘要"), "对话", Some("保留结论"));
        assert!(p.contains("<prior-summary>") && p.contains("旧摘要"));
        assert!(p.contains("保留结论"));
        let p2 = summary_prompt(None, "对话", None);
        assert!(!p2.contains("prior-summary"));
    }
}
