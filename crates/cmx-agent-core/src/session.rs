//! 会话 [`Session`]：持有 append-only 日志，并**只从日志派生**给模型的上下文——这就是不变量
//! "Model-visible means logged" 的结构性保证：模型看到的每条消息都能在日志里找到出处。

use crate::event::{EventKind, SessionLog};
use crate::model::{ModelContext, ModelMessage};
use crate::tool::ToolSpec;

/// 一个会话（M0：单会话、内存态。后续可持久化 / 多会话 / 子会话隔离）。
#[derive(Debug)]
pub struct Session {
    pub id: String,
    pub log: SessionLog,
    system: Option<String>,
}

impl Session {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            log: SessionLog::new(),
            system: None,
        }
    }

    /// 设置系统指令（AGENTS.md 式项目指令的 M0 落点）。
    pub fn with_system(mut self, system: impl Into<String>) -> Self {
        self.system = Some(system.into());
        self
    }

    pub fn system(&self) -> Option<&str> {
        self.system.as_deref()
    }

    /// 下一个回合号：历史 `TurnEnded` 事件计数 + 1。从日志派生 → 持久化恢复后能续上正确回合号。
    pub fn next_turn_no(&self) -> u64 {
        let ended = self.log.count(|k| matches!(k, EventKind::TurnEnded { .. }));
        ended as u64 + 1
    }

    /// 用一串已有事件重建会话（持久化恢复）。事件按 `seq` 排序后灌入日志，seq/时间戳保持原值。
    pub fn from_events(
        id: impl Into<String>,
        system: Option<String>,
        events: impl IntoIterator<Item = crate::event::SessionEvent>,
    ) -> Self {
        let mut log = SessionLog::new();
        let mut evs: Vec<_> = events.into_iter().collect();
        evs.sort_by_key(|e| e.seq);
        for e in evs {
            log.push_restored(e);
        }
        Self {
            id: id.into(),
            log,
            system,
        }
    }

    /// 从 append-only 日志重建给模型的上下文（+ 当前工具清单）。
    ///
    /// 遍历事件，把"模型可见"的三类事件投影为消息：
    /// - `UserMessage` → User
    /// - `ModelMessage` → Assistant（文本 + 工具调用）
    /// - `ToolResult`   → Tool（回灌）
    ///
    /// 其余事件（守卫裁决、审批、提问、注记、turn 边界）是审计/控制流元数据，不进模型上下文。
    ///
    /// **配对自愈**：对有 `tool_calls` 却始终没有配对 `ToolResult` 的调用（回合被中断/崩溃于
    /// 执行段的历史遗留），在投影尾部补合成错误结果——主流模型 API 严格校验 assistant 的每个
    /// tool_call 必须跟随 tool 结果，缺配对会 400 并把会话对模型砖死。中断只缺尾部调用，补在
    /// 结尾顺序正确。
    ///
    /// **压缩投影（压缩方案 §4.2.2）**：最后一条 [`EventKind::Compacted`] 之前（`up_to_seq`
    /// 含）的可见事件不再投影，改为在消息头部注入摘要 User 消息；所有
    /// [`EventKind::ToolOutputsPruned`] 的 call_id 并集命中的 `ToolResult` 投影为占位 JSON
    /// （原 output 不动——append-only 不变量保持）。
    pub fn model_context(&self, tools: Vec<ToolSpec>) -> ModelContext {
        // 扫描压缩标记：最后压缩点 + 摘要 + 清理并集
        let mut cutoff: u64 = 0;
        let mut summary: Option<&String> = None;
        let mut pruned: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for ev in self.log.iter() {
            match &ev.kind {
                EventKind::Compacted { up_to_seq, summary: sum, .. } => {
                    cutoff = *up_to_seq;
                    summary = Some(sum);
                }
                EventKind::ToolOutputsPruned { call_ids } => {
                    for id in call_ids {
                        pruned.insert(id.as_str());
                    }
                }
                _ => {}
            }
        }

        let mut messages = Vec::new();
        if let Some(s) = summary {
            messages.push(ModelMessage::User {
                text: format!(
                    "[会话前段已被压缩，以下是截至当时的结构化摘要；其后为近期原文。]\n\
                     <compacted-summary>\n{s}\n</compacted-summary>"
                ),
            });
        }

        let mut open_calls: Vec<String> = Vec::new();
        for ev in self.log.iter() {
            if ev.seq <= cutoff {
                continue; // 已被摘要覆盖的段落不投影（含失败回合的 UserMessage——救援重试不重复）
            }
            match &ev.kind {
                EventKind::UserMessage { text } => {
                    messages.push(ModelMessage::User { text: text.clone() });
                }
                EventKind::ModelMessage { text, tool_calls } => {
                    for c in tool_calls {
                        open_calls.push(c.id.clone());
                    }
                    messages.push(ModelMessage::Assistant {
                        text: text.clone(),
                        tool_calls: tool_calls.clone(),
                    });
                }
                EventKind::ToolResult {
                    call_id, output, ..
                } => {
                    open_calls.retain(|id| id != call_id);
                    if pruned.contains(call_id.as_str()) {
                        // 清理占位：原字符数投影时现算（原事件不动）
                        let n = match output {
                            serde_json::Value::String(s) => s.chars().count(),
                            other => other.to_string().chars().count(),
                        };
                        messages.push(ModelMessage::Tool {
                            call_id: call_id.clone(),
                            output: serde_json::json!({
                                "pruned": true,
                                "note": format!("较早的工具输出已清理（原约 {n} 字符）")
                            }),
                        });
                    } else {
                        messages.push(ModelMessage::Tool {
                            call_id: call_id.clone(),
                            output: output.clone(),
                        });
                    }
                }
                _ => {}
            }
        }
        for id in open_calls {
            messages.push(ModelMessage::Tool {
                call_id: id,
                output: serde_json::json!({
                    "error": "interrupted before tool result",
                    "interrupted": true
                }),
            });
        }
        ModelContext {
            system: self.system.clone(),
            messages,
            tools,
        }
    }
}
