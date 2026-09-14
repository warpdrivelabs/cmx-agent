//! 向用户提问（人在环的第三态）：模型在执行中遇到需要用户决策/澄清的分歧点时，调 `ask_user`
//! 工具抛出结构化问题（标签 + 问题 + 单/多选项），内核挂起等待用户在桌面壳答题卡上作答后，
//! 答案以 tool result JSON 回灌、回合继续；用户忽略/超时回灌 `dismissed:true` 让模型自行降级。
//!
//! 与审批（[`crate::agent::Approver`]）机制独立、模式同构：审批回答「允许/拒绝」二元决策，
//! 提问回答的是结构化内容——强扭进 `Approver` 的 `(bool, String)` 签名会污染审批语义。
//!
//! 职责切分（两条不变量，见方案 20260914 §4.3）：
//! - **工具不触碰 `SessionLog`**：本模块的工具对象只声明 spec（给模型的能力清单），真正的
//!   挂起由内核在 `handle_tool_calls_as` 对 `user_interactive` 工具托管执行；
//! - **事件只从内核写入会话日志**：`QuestionAsked/QuestionResolved` 由内核在 pre 相/回灌段
//!   落日志（与审批事件同构），从而天然满足「落库 + 回合 SSE + 进程总线」三通路。
//!
//! 挂起骨架对齐 `cmx-agent-app::approval::InteractiveApprover`：pending 登记 + oneshot +
//! 会话绑定校验 + cancel/revoke 清理。

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::{Approval, GuardHints, Tool, ToolCtx, ToolError, ToolResult, ToolSpec};

/// `ask_user` 工具名（内核据此不做特判之外的事；工具注册表内唯一）。
pub const QUESTION_TOOL_NAME: &str = "ask_user";

/// 一个提问选项。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AskOption {
    /// 展示文本（1~5 词，简洁）。
    pub label: String,
    /// 一句话说明该选项的影响/权衡。
    #[serde(default)]
    pub description: String,
}

/// 一个提问。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AskQuestion {
    /// snake_case 短 id，答案映射键（回灌 JSON 以此为键）。
    #[serde(default)]
    pub id: String,
    /// UI 短标签（≤12 字，答题卡左上角）。
    #[serde(default)]
    pub header: String,
    /// 完整问题文本。
    pub question: String,
    /// 选项 2~4 个；自由输入由客户端自动附加，模型不得自供"其他"选项。
    pub options: Vec<AskOption>,
    /// 多选（默认单选）。
    #[serde(default)]
    pub multiple: bool,
}

/// 服务端硬规范化上限（schema 是给模型的建议，这里是强制——防超长/海量提问直达事件流与 UI）。
const MAX_QUESTIONS: usize = 3;
const MAX_OPTIONS: usize = 4;
const MAX_QUESTION_CHARS: usize = 200;
const MAX_HEADER_CHARS: usize = 12;
const MAX_LABEL_CHARS: usize = 30;
const MAX_DESC_CHARS: usize = 100;

fn truncate_chars(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// 把模型给出的 `ask_user` 入参规范化为 [`AskQuestion`] 列表。
///
/// 硬规则：1~3 问、每问 options 非空（≤4 个截断）、字符串按上限截断、缺 `id` 补 `q1/q2/…`、
/// 重复 `id` 改写为 `{id}_{n}` 去重（答案映射是 serde Map，后写覆盖先写，不去重会静默丢答案）。
pub fn normalize_ask_input(input: &Value) -> Result<Vec<AskQuestion>, String> {
    let Some(raw_questions) = input.get("questions").and_then(|q| q.as_array()) else {
        return Err("缺少 questions 数组".into());
    };
    if raw_questions.is_empty() {
        return Err("questions 不能为空".into());
    }
    let mut out: Vec<AskQuestion> = Vec::new();
    let mut seen_ids: HashMap<String, usize> = HashMap::new();
    for (i, raw) in raw_questions.iter().take(MAX_QUESTIONS).enumerate() {
        let question = raw
            .get("question")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .unwrap_or("")
            .to_string();
        if question.is_empty() {
            return Err(format!("第 {} 问缺少 question 文本", i + 1));
        }
        let mut options: Vec<AskOption> = Vec::new();
        let raw_opts = raw
            .get("options")
            .and_then(|v| v.as_array())
            .ok_or_else(|| format!("第 {} 问缺少 options", i + 1))?;
        for opt in raw_opts.iter().take(MAX_OPTIONS) {
            let label = opt
                .get("label")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("")
                .to_string();
            if label.is_empty() {
                return Err(format!("第 {} 问存在空 label 选项", i + 1));
            }
            options.push(AskOption {
                label: truncate_chars(&label, MAX_LABEL_CHARS),
                description: truncate_chars(
                    opt.get("description").and_then(|v| v.as_str()).unwrap_or(""),
                    MAX_DESC_CHARS,
                ),
            });
        }
        if options.is_empty() {
            return Err(format!("第 {} 问 options 为空", i + 1));
        }
        let mut id = match raw.get("id").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => format!("q{}", i + 1),
        };
        // 重复 id 去重：`scene` → `scene_2`、`scene_3`…
        let n = seen_ids.entry(id.clone()).or_insert(0);
        *n += 1;
        if *n > 1 {
            id = format!("{id}_{n}");
        }
        let header = match raw.get("header").and_then(|v| v.as_str()) {
            Some(h) if !h.trim().is_empty() => truncate_chars(h.trim(), MAX_HEADER_CHARS),
            _ => "提问".to_string(),
        };
        out.push(AskQuestion {
            id,
            header,
            question: truncate_chars(&question, MAX_QUESTION_CHARS),
            options,
            multiple: raw
                .get("multiple")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        });
    }
    Ok(out)
}

/// 一次提问的结局：答案（按 question id 映射），或放弃（含放弃方：user=用户点忽略、
/// canceled=清理路径唤醒失败、timeout=兜底超时）。
#[derive(Debug, Clone, PartialEq)]
pub enum QuestionOutcome {
    Answered(serde_json::Map<String, Value>),
    Dismissed(&'static str),
}

/// 待决提问的对外快照（`ListPendingQuestions` / 前端在途恢复用）。
#[derive(Debug, Clone, Serialize)]
pub struct PendingQuestionInfo {
    pub request_id: String,
    pub session_id: String,
    pub questions: Vec<AskQuestion>,
}

struct PendingQuestion {
    session_id: String,
    questions: Vec<AskQuestion>,
    tx: tokio::sync::oneshot::Sender<QuestionOutcome>,
}

/// 提问挂起服务（进程内内存态；重启即清，与审批 pending 同语义）。
///
/// `timeout` 为兜底超时：挂起回合在 Tauri 壳钉死一个 blocking 线程、同会话后续请求在
/// session_lock 上排队，无限挂起可逐渐饿死线程池——默认 30 分钟兜底，超时按忽略处理
/// 并可配置（`None` = 无限，对齐 opencode/codex 阻塞式语义）。
pub struct QuestionService {
    enabled: bool,
    timeout: Option<Duration>,
    /// request_id → 待决提问。同一会话同时至多一个待决（内核 has_pending 防重），
    /// 但保留 Map 结构以兼容清理路径的按 id 精确摘除。
    pending: Mutex<HashMap<String, PendingQuestion>>,
}

impl Default for QuestionService {
    fn default() -> Self {
        Self::interactive(Some(Duration::from_secs(30 * 60)))
    }
}

impl QuestionService {
    /// 交互式服务（桌面双壳）：真实挂起等答，带兜底超时。
    pub fn interactive(timeout: Option<Duration>) -> Self {
        Self {
            enabled: true,
            timeout,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 降级服务（CLI / e2e / 非交互装配）：`enabled=false`，内核据此直接回灌 dismissed，
    /// 不发生挂起（fail-open，防无人值守回合被一个永远不来的回答挂死）。
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            timeout: None,
            pending: Mutex::new(HashMap::new()),
        }
    }

    /// 是否真实挂起（内核门控判据之一）。
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// 兜底超时（内核等待侧读取）。
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// 登记一个提问，返回 (request_id, 等待端)。调用方（内核）随后落 `QuestionAsked` 事件
    /// 并在工具执行段 await 等待端。
    pub fn register(
        &self,
        session_id: &str,
        questions: Vec<AskQuestion>,
    ) -> (String, tokio::sync::oneshot::Receiver<QuestionOutcome>) {
        let request_id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.pending.lock().expect("questions pending lock").insert(
            request_id.clone(),
            PendingQuestion {
                session_id: session_id.to_string(),
                questions,
                tx,
            },
        );
        (request_id, rx)
    }

    /// 该会话是否已有待决提问（同会话串行化：新的 ask_user 直接被拒，防并发双卡）。
    pub fn has_pending(&self, session_id: &str) -> bool {
        self.pending
            .lock()
            .expect("questions pending lock")
            .values()
            .any(|p| p.session_id == session_id)
    }

    /// 提交答案：按问题序组 id 映射后唤醒。返回是否命中一个待决提问。
    /// `session_hint` **必须非空**并与会话绑定一致——request_id 由模型自报、跨会话可能重号，
    /// B 会话的答案不得作用于 A 会话挂起的提问（对齐审批 `decide` 的绑定校验，且不继承其
    /// 「空 hint 放行」弱点）。
    pub fn answer(
        &self,
        request_id: &str,
        answers: Vec<Vec<String>>,
        session_hint: &str,
    ) -> bool {
        if session_hint.is_empty() {
            return false;
        }
        let mut pending = self.pending.lock().expect("questions pending lock");
        let Some(p) = pending.get(request_id) else {
            return false;
        };
        if p.session_id != session_hint {
            eprintln!(
                "[question] 提问 {request_id} 属于会话 {:?}，拒绝来自会话 {session_hint:?} 的回答",
                p.session_id
            );
            return false;
        }
        let Some(p) = pending.remove(request_id) else {
            return false;
        };
        // 长度不齐：缺的问题补空数组（模型视角 = 未答），多出的静默弃。
        let mut map = serde_json::Map::new();
        for (i, q) in p.questions.iter().enumerate() {
            let one = answers.get(i).cloned().unwrap_or_default();
            map.insert(q.id.clone(), serde_json::Value::from(one));
        }
        p.tx
            .send(QuestionOutcome::Answered(map))
            .is_ok()
    }

    /// 忽略一个提问（用户点「忽略」）。返回是否命中。
    pub fn dismiss(&self, request_id: &str, session_hint: &str) -> bool {
        if session_hint.is_empty() {
            return false;
        }
        let mut pending = self.pending.lock().expect("questions pending lock");
        let Some(p) = pending.get(request_id) else {
            return false;
        };
        if p.session_id != session_hint {
            return false;
        }
        let Some(p) = pending.remove(request_id) else {
            return false;
        };
        p.tx.send(QuestionOutcome::Dismissed("user")).is_ok()
    }

    /// 兜底超时摘除（幂等；发送端可能已被 answer 摘走，失败无害）。
    pub fn expire(&self, request_id: &str) {
        self.pending
            .lock()
            .expect("questions pending lock")
            .remove(request_id);
    }

    /// 中断某会话的全部待决提问（CancelSession / 删除会话路径）。
    pub fn cancel_session(&self, session_id: &str) {
        let ids: Vec<String> = self
            .pending
            .lock()
            .expect("questions pending lock")
            .iter()
            .filter(|(_, p)| p.session_id == session_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let Some(p) = self
                .pending
                .lock()
                .expect("questions pending lock")
                .remove(&id)
            else {
                continue;
            };
            let _ = p.tx.send(QuestionOutcome::Dismissed("canceled"));
        }
    }

    /// 全量撤销（登出/换账号）：拒绝所有待决提问——挂起不得跨登录身份沿用。
    pub fn revoke_all(&self) {
        let mut pending = self.pending.lock().expect("questions pending lock");
        for (_, p) in pending.drain() {
            let _ = p.tx.send(QuestionOutcome::Dismissed("canceled"));
        }
    }

    /// 列待决提问（在途恢复主路径：前端刷新/重开窗口后据此重渲染待答卡——
    /// 回合事件回合末才落库，GetEvents 重放拿不到在途挂起）。
    pub fn pending_in_session(&self, session_id: Option<&str>) -> Vec<PendingQuestionInfo> {
        self.pending
            .lock()
            .expect("questions pending lock")
            .iter()
            .filter(|(_, p)| session_id.is_none_or(|sid| p.session_id == sid))
            .map(|(id, p)| PendingQuestionInfo {
                request_id: id.clone(),
                session_id: p.session_id.clone(),
                questions: p.questions.clone(),
            })
            .collect()
    }
}

/// `ask_user` 工具对象：只声明 spec（进模型工具清单），执行由内核托管。
///
/// 内核在 `handle_tool_calls_as` 的 pre 相对 `user_interactive` 工具特判：规范化入参 →
/// `QuestionService::register` → 落 `QuestionAsked` → 执行段挂起等答 → 回灌段组 tool result
/// 并落 `QuestionResolved`。故 [`Tool::invoke`] 正常情况下不会到达（兜底报错以防内核特判
/// 被绕过时静默丢失调用）。
pub struct AskUserTool;

const ASK_USER_DESCRIPTION: &str = "向用户提问并等待回答。当且仅当执行中遇到真正需要用户决策/澄清的分歧点时使用——\
能用合理默认或从上下文推断就不要问。一次 1~3 个问题；每个问题给 2~4 个互斥选项（label 1~5 词，\
description 一句话说明影响）；推荐选项放首位并在 label 末尾加 (Recommended)。\
多选题把 multiple 置 true。客户端会自动提供自由文本输入，不要添加“其他”类选项。\
答案将以 tool result 回灌；若结果为 dismissed:true，说明用户忽略或超时——不要反复追问，\
自行选择合理默认继续。";

fn ask_user_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "questions": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "id": {
                            "type": "string",
                            "description": "snake_case 短 id，答案映射键（如 scene、budget）"
                        },
                        "header": {
                            "type": "string",
                            "description": "UI 短标签，不超过 12 个字（如「使用场景」「部署方式」）"
                        },
                        "question": {
                            "type": "string",
                            "description": "完整问题，单句，具体明确"
                        },
                        "options": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "label": {
                                        "type": "string",
                                        "description": "选项展示文本，1~5 词，简洁；推荐项放首位并加 (Recommended)"
                                    },
                                    "description": {
                                        "type": "string",
                                        "description": "一句话说明该选项的影响/权衡"
                                    }
                                },
                                "required": ["label", "description"]
                            },
                            "description": "2~4 个互斥选项；不要添加“其他”类选项（客户端自动提供自由输入）"
                        },
                        "multiple": {
                            "type": "boolean",
                            "description": "true = 多选（复选框）；缺省单选"
                        }
                    },
                    "required": ["question", "options"]
                },
                "description": "要问的问题列表，1~3 个；避免连续刷屏"
            }
        },
        "required": ["questions"]
    })
}

#[async_trait::async_trait]
impl Tool for AskUserTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(QUESTION_TOOL_NAME, ASK_USER_DESCRIPTION)
            .schema(ask_user_schema())
            .guard(GuardHints {
                // 提问本身只读无副作用：never + 幂等——否则 UnlessTrusted 档会对提问弹无关审批卡。
                requires_approval: Approval::Never,
                idempotent: true,
                ..GuardHints::default()
            })
            .user_interactive()
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::err(
            "ask_user is managed by the kernel (user_interactive); this fallback should not be reached",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn input(v: Value) -> Value {
        json!({ "questions": v })
    }

    #[test]
    fn normalize_fills_ids_and_defaults() {
        let qs = normalize_ask_input(&input(json!([
            {"question": "部署到哪？", "options": [
                {"label": "测试环境", "description": "先验证"},
                {"label": "生产环境", "description": "直接上线"}
            ]},
            {"id": "note", "header": "备注", "question": "还有什么要求？", "multiple": true,
             "options": [{"label": "无", "description": "没有"}]}
        ])))
        .expect("normalize ok");
        assert_eq!(qs.len(), 2);
        assert_eq!(qs[0].id, "q1");
        assert_eq!(qs[0].header, "提问");
        assert!(!qs[0].multiple);
        assert_eq!(qs[1].id, "note");
        assert_eq!(qs[1].header, "备注");
        assert!(qs[1].multiple);
    }

    #[test]
    fn normalize_rewrites_duplicate_ids() {
        let qs = normalize_ask_input(&input(json!([
            {"id": "scene", "question": "A?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]},
            {"id": "scene", "question": "B?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]}
        ])))
        .expect("normalize ok");
        assert_eq!(qs[0].id, "scene");
        assert_eq!(qs[1].id, "scene_2");
    }

    #[test]
    fn normalize_truncates_and_caps() {
        let long = "长".repeat(500);
        let many_opts: Vec<_> = (0..9)
            .map(|i| json!({"label": format!("选项{i}"), "description": ""}))
            .collect();
        let qs = normalize_ask_input(&input(json!([
            {"id": "a", "header": "这是一个超过十二个字的标签", "question": long, "options": many_opts},
            {"id": "b", "question": "B?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]},
            {"id": "c", "question": "C?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]},
            {"id": "d", "question": "D?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]}
        ])))
        .expect("normalize ok");
        assert_eq!(qs.len(), MAX_QUESTIONS);
        assert_eq!(qs[0].question.chars().count(), MAX_QUESTION_CHARS);
        assert_eq!(qs[0].header.chars().count(), MAX_HEADER_CHARS);
        assert_eq!(qs[0].options.len(), MAX_OPTIONS);
    }

    #[test]
    fn normalize_rejects_bad_input() {
        assert!(normalize_ask_input(&json!({})).is_err());
        assert!(normalize_ask_input(&input(json!([]))).is_err());
        assert!(normalize_ask_input(&input(json!([{"question": "无选项"}]))).is_err());
        assert!(normalize_ask_input(&input(json!([{
            "question": "空选项", "options": []
        }]))).is_err());
        assert!(normalize_ask_input(&input(json!([{
            "question": "空 label", "options": [{"label": " ", "description": ""}, {"label": "y", "description": ""}]
        }]))).is_err());
        assert!(normalize_ask_input(&input(json!([{
            "question": " ", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]
        }]))).is_err());
    }

    #[tokio::test]
    async fn register_answer_roundtrip_and_session_binding() {
        let svc = QuestionService::interactive(None);
        let qs = normalize_ask_input(&input(json!([
            {"id": "s", "question": "选一个", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]}
        ])))
        .expect("normalize ok");
        let (rid, rx) = svc.register("sess-a", qs);
        assert!(svc.has_pending("sess-a"));
        assert!(!svc.has_pending("sess-b"));

        // 跨会话拒绝 + 空 hint 拒绝
        assert!(!svc.answer(&rid, vec![vec!["x".into()]], "sess-b"));
        assert!(!svc.answer(&rid, vec![vec!["x".into()]], ""));
        assert!(svc.has_pending("sess-a"));

        // 正确会话命中
        assert!(svc.answer(&rid, vec![vec!["x".into(), "y".into()]], "sess-a"));
        assert!(!svc.has_pending("sess-a"));
        let outcome = rx.await.expect("answered");
        match outcome {
            QuestionOutcome::Answered(map) => {
                assert_eq!(
                    map.get("s").and_then(|v| v.as_array()).map(|a| a.len()),
                    Some(2)
                );
            }
            other => panic!("expected Answered, got {other:?}"),
        }
        // 已摘除：重复 answer 未命中
        assert!(!svc.answer(&rid, vec![vec!["x".into()]], "sess-a"));
    }

    #[tokio::test]
    async fn answer_pads_short_answers() {
        let svc = QuestionService::interactive(None);
        let qs = normalize_ask_input(&input(json!([
            {"id": "a", "question": "A?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]},
            {"id": "b", "question": "B?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]}
        ])))
        .expect("normalize ok");
        let (rid, rx) = svc.register("s", qs);
        // 只答第一问（长度不齐）：第二问补空数组
        assert!(svc.answer(&rid, vec![vec!["x".into()]], "s"));
        match rx.await.expect("answered") {
            QuestionOutcome::Answered(map) => {
                assert_eq!(map.get("b").and_then(|v| v.as_array()).map(|a| a.len()), Some(0));
            }
            other => panic!("expected Answered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dismiss_and_cancel_and_revoke() {
        let svc = QuestionService::interactive(None);
        let qs = |_: ()| {
            normalize_ask_input(&input(json!([
                {"id": "s", "question": "Q?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]}
            ])))
            .expect("normalize ok")
        };
        // 忽略：by = user
        let (rid, rx) = svc.register("s1", qs(()));
        assert!(svc.dismiss(&rid, "s1"));
        assert!(matches!(rx.await, Ok(QuestionOutcome::Dismissed("user"))));
        // 会话取消：by = canceled
        let (_rid, rx) = svc.register("s2", qs(()));
        svc.cancel_session("s2");
        assert!(matches!(rx.await, Ok(QuestionOutcome::Dismissed("canceled"))));
        // 全量撤销：by = canceled
        let (_rid, rx) = svc.register("s3", qs(()));
        svc.revoke_all();
        assert!(matches!(rx.await, Ok(QuestionOutcome::Dismissed("canceled"))));
        assert!(svc.pending_in_session(None).is_empty());
    }

    #[tokio::test]
    async fn sender_drop_maps_to_canceled_dismissal() {
        let svc = QuestionService::interactive(None);
        let qs = normalize_ask_input(&input(json!([
            {"id": "s", "question": "Q?", "options": [{"label": "x", "description": ""}, {"label": "y", "description": ""}]}
        ])))
        .expect("normalize ok");
        let (rid, rx) = svc.register("s", qs);
        // 模拟幽灵摘除（如 IM 桥热重载 drop 回合后 answer 打到死对）：
        svc.expire(&rid);
        // 之后 answer 未命中（tx 已被摘）
        assert!(!svc.answer(&rid, vec![vec!["x".into()]], "s"));
        drop(rx);
    }

    #[test]
    fn disabled_service_reports_disabled() {
        let svc = QuestionService::disabled();
        assert!(!svc.enabled());
        assert!(svc.timeout().is_none());
        assert!(svc.pending_in_session(None).is_empty());
    }

    #[test]
    fn ask_user_tool_spec_is_readonly_and_interactive() {
        let spec = AskUserTool.spec();
        assert_eq!(spec.name, QUESTION_TOOL_NAME);
        assert!(spec.user_interactive);
        assert_eq!(spec.guard.requires_approval, Approval::Never);
        assert!(spec.guard.idempotent);
        // schema 必含 questions 数组定义
        assert!(spec.input_schema.get("properties").is_some());
    }
}
