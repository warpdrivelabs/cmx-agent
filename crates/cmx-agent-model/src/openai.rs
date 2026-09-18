//! OpenAI 兼容 provider（`{base}/chat/completions`）——DeepSeek/OpenAI/Qwen/Ollama/vLLM 通用。
//!
//! 实现 [`cmx_agent_core::model::ModelSeam`]，支持**工具调用**（function calling）：
//! 把内核的 `ModelContext`（system+messages+tools）编成 OpenAI 请求，回来的 `choices[0].message`
//! 解成 `ModelResponse`（text + tool_calls）。请求编码/响应解码是**纯函数**，离线可单测。

use std::time::Duration;

use async_trait::async_trait;
use cmx_agent_core::model::{ModelContext, ModelError, ModelMessage, ModelResponse, ModelSeam, ModelUsage};
use cmx_agent_core::tool::ToolCall;
use serde_json::{Value, json};

use crate::config::ModelProviderConfig;
use crate::error_friendly::friendly_model_error;

/// 错误构造统一收口：先套友好话术（七类映射，方案「报错可见性」B 项——
/// IM 桥复用同一文本，后端成型三端同时受益），原文保留在括号里。
/// 控制哨兵（`__turn_cancelled__`）不经映射原样通过。
fn err(raw: impl std::fmt::Display) -> ModelError {
    let s = raw.to_string();
    if s.starts_with("__turn_cancelled__") {
        return ModelError(s);
    }
    ModelError(friendly_model_error(&s))
}

/// OpenAI 兼容模型缝。
pub struct OpenAiCompatModel {
    cfg: ModelProviderConfig,
    client: reqwest::Client,
}

impl OpenAiCompatModel {
    pub fn new(cfg: ModelProviderConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(cfg.timeout_ms))
            .user_agent("cmx-agent-model/0.1")
            .build()
            .expect("build reqwest client");
        Self { cfg, client }
    }

    pub fn model_name(&self) -> &str {
        &self.cfg.model
    }

    /// 连通性探测（配置面板「测试连接」用，方案 20260917 §5.4）：向 `{base}/chat/completions`
    /// 发 `max_tokens=1` 的一次真实请求——同时验证 key、URL、模型名三样（比 GET /models
    /// 普适，部分网关没有 /models）。key/URL 只进内存不落盘；token 消耗 1 级。
    /// 返回耗时毫秒；错误按七类分类（`TestConnectError::kind`）并带友好话术，
    /// 分类映射与聊天回合错误共用 `friendly_model_error` 一张表。
    pub async fn ping(&self) -> Result<u64, TestConnectError> {
        use crate::error_friendly::{classify, friendly_model_error_brief};
        let map = |raw: String| TestConnectError {
            kind: classify(&raw),
            // 测试连接悬浮条是小空间：只要话术、不带「（原文：…）」（用户：不用展示原文）
            message: friendly_model_error_brief(&raw),
        };
        let url = format!("{}/chat/completions", self.cfg.base_url);
        let body = json!({
            "model": self.cfg.model,
            "messages": [{"role": "user", "content": "ping"}],
            "max_tokens": 1,
        });
        let mut req = self.client.post(&url).json(&body);
        if !self.cfg.api_key.is_empty() {
            req = req.bearer_auth(&self.cfg.api_key);
        }
        let t0 = std::time::Instant::now();
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) => return Err(map(format!("请求模型失败: {e}"))),
        };
        let status = resp.status();
        let v: Value = resp.json().await.unwrap_or(Value::Null);
        // 信封错误优先（4xx/5xx 带 message、个别网关 200 也回 error 信封）
        if let Some(m) = v.get("error").and_then(|e| e.get("message")).and_then(|x| x.as_str()) {
            return Err(map(format!("HTTP {}: {m}", status.as_u16())));
        }
        if !status.is_success() {
            return Err(map(format!("模型服务返回 HTTP {}", status.as_u16())));
        }
        Ok(t0.elapsed().as_millis() as u64)
    }
}

/// 「测试连接」探测错误：七类稳定分类值 + 友好话术（含原文）。
#[derive(Debug, Clone)]
pub struct TestConnectError {
    /// auth / network / timeout / model_not_found / rate_limit / server / unknown。
    pub kind: &'static str,
    pub message: String,
}

/// 把内核 `ModelContext` 编成 OpenAI chat/completions 请求体（纯函数，可测）。
///
/// `max_tokens`：输出上限——正常回合不传（维持旧行为），仅压缩摘要等合成请求传
/// （压缩方案 §4.2.4.3；`ModelProviderConfig` 无该配置字段，是有意收窄）。
pub fn build_request_body(cfg: &ModelProviderConfig, ctx: &ModelContext, max_tokens: Option<u64>) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &ctx.system
        && !sys.is_empty() {
            messages.push(json!({"role": "system", "content": sys}));
        }
    for m in &ctx.messages {
        match m {
            ModelMessage::User { text } => {
                messages.push(json!({"role": "user", "content": text}));
            }
            ModelMessage::Assistant { text, tool_calls } => {
                let mut msg = json!({"role": "assistant"});
                // content 允许为 null（当只有工具调用时）
                msg["content"] = match text {
                    Some(t) => json!(t),
                    None => Value::Null,
                };
                if !tool_calls.is_empty() {
                    let calls: Vec<Value> = tool_calls
                        .iter()
                        .map(|c| {
                            json!({
                                "id": c.id,
                                "type": "function",
                                "function": {
                                    "name": c.name,
                                    // OpenAI 约定 arguments 是「JSON 字符串」
                                    "arguments": c.input.to_string(),
                                }
                            })
                        })
                        .collect();
                    msg["tool_calls"] = json!(calls);
                }
                messages.push(msg);
            }
            ModelMessage::Tool { call_id, output } => {
                let content = match output {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": call_id,
                    "content": content,
                }));
            }
        }
    }

    let mut body = json!({
        "model": cfg.model,
        "messages": messages,
        "temperature": cfg.temperature,
    });
    if let Some(mt) = max_tokens {
        body["max_tokens"] = json!(mt);
    }

    if !ctx.tools.is_empty() {
        let tools: Vec<Value> = ctx
            .tools
            .iter()
            .map(|t| {
                json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    }
                })
            })
            .collect();
        body["tools"] = json!(tools);
        body["tool_choice"] = json!("auto");
    }
    body
}

/// 解析 OpenAI chat/completions 响应为 `ModelResponse`（纯函数，可测）。
pub fn parse_response(v: &Value) -> Result<ModelResponse, ModelError> {
    // 上游错误信封：{"error":{"message":...}}（HTTP 200 但业务报错的网关形态）
    if let Some(env) = v.get("error") {
        let msg = env
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("上游模型错误");
        return Err(err(msg));
    }
    let msg = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first())
        .and_then(|c0| c0.get("message"))
        .ok_or_else(|| ModelError("响应缺少 choices[0].message".to_string()))?;

    let text = msg
        .get("content")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let mut tool_calls: Vec<ToolCall> = Vec::new();
    if let Some(calls) = msg.get("tool_calls").and_then(|c| c.as_array()) {
        for c in calls.iter() {
            let id = c
                .get("id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(next_call_id);
            let func = c.get("function");
            let name = func
                .and_then(|f| f.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            // arguments 是 JSON 字符串；解析失败（典型：finish_reason=length 截断的半截参数）
            // 直接丢弃该调用——旧实现塞 {"_raw": 原串} 照跑，残参会原样进入错误消息/日志。
            let args_raw = func
                .and_then(|f| f.get("arguments"))
                .and_then(|x| x.as_str())
                .unwrap_or("{}");
            let Ok(input) = serde_json::from_str::<Value>(args_raw) else {
                eprintln!("[model] 工具 {name} 的参数不是合法 JSON（疑似截断），已丢弃");
                continue;
            };
            if !name.is_empty() {
                tool_calls.push(ToolCall::with_id(id, name, input));
            }
        }
    }

    let reasoning = msg
        .get("reasoning_content")
        .and_then(|c| c.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());

    let usage = v.get("usage").and_then(parse_usage_json);
    Ok(ModelResponse {
        text,
        reasoning,
        tool_calls,
        usage,
    })
}

/// 解析 usage 块（压缩方案 §4.1.2）。**取数必须用 `prompt_tokens`**——MLamp 实测响应里
/// 另有 `input_tokens` 字段但恒 0；`cached_input` 兼容 OpenAI 新版（prompt_tokens_details.
/// cached_tokens）与 DeepSeek 风格（prompt_cache_hit_tokens）两种网关形态。缺 prompt_tokens
/// 返回 None（不覆盖已收 usage）。
fn parse_usage_json(u: &Value) -> Option<ModelUsage> {
    let input = u.get("prompt_tokens").and_then(|x| x.as_u64())?;
    let output = u
        .get("completion_tokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let cached_input = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|x| x.as_u64())
        .or_else(|| u.get("prompt_cache_hit_tokens").and_then(|x| x.as_u64()));
    Some(ModelUsage {
        input,
        output,
        cached_input,
    })
}

/// 合成 tool_call id 的进程级序号：网关省略 id 时跨会话也绝不重号（旧实现按响应内
/// index 从 0 起名，两会话并发时各自的 `call_0` 在审批 pending 表里互相覆盖）。
fn next_call_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static CALL_SEQ: AtomicU64 = AtomicU64::new(0);
    format!("call_{}", CALL_SEQ.fetch_add(1, Ordering::Relaxed))
}

/// 流式累加器：跨 SSE 增量拼接文字与工具调用（工具的 arguments 会分片到达，按 index 拼）。
#[derive(Default)]
struct StreamAcc {
    text: String,
    reasoning: String,
    tools: Vec<ToolAcc>,
    /// 是否已收到服务端的 `finish_reason`（生成真正结束，含 stop / tool_calls / length）。
    finished: bool,
    /// 任意帧出现顶层 `usage` 即缓存（后到覆盖：OpenAI 约定最后一个 usage-only 帧是完整值）。
    usage: Option<ModelUsage>,
}
#[derive(Default)]
struct ToolAcc {
    id: String,
    name: String,
    args: String,
}
impl StreamAcc {
    fn slot(&mut self, idx: usize) -> &mut ToolAcc {
        while self.tools.len() <= idx {
            self.tools.push(ToolAcc::default());
        }
        &mut self.tools[idx]
    }
    fn into_response(self) -> ModelResponse {
        let text = if self.text.is_empty() {
            None
        } else {
            Some(self.text)
        };
        let tool_calls = self
            .tools
            .into_iter()
            .filter(|t| !t.name.is_empty())
            .filter_map(|t| {
                let id = if t.id.is_empty() {
                    next_call_id()
                } else {
                    t.id
                };
                // 参数解析失败（疑似 length 截断的半截参数）直接丢弃，不再以 {"_raw":…} 照跑。
                match serde_json::from_str::<Value>(if t.args.is_empty() { "{}" } else { &t.args }) {
                    Ok(input) => Some(ToolCall::with_id(id, t.name, input)),
                    Err(_) => {
                        eprintln!("[model] 工具 {} 的参数不是合法 JSON（疑似截断），已丢弃", t.name);
                        None
                    }
                }
            })
            .collect();
        let reasoning = if self.reasoning.is_empty() {
            None
        } else {
            Some(self.reasoning)
        };
        ModelResponse {
            text,
            reasoning,
            tool_calls,
            usage: self.usage,
        }
    }
}

/// 流式增量类型：正文与思考过程分开回调，避免推理内容污染回复气泡。
#[derive(Debug, Clone, PartialEq, Eq)]
enum StreamDelta {
    Text(String),
    Reasoning(String),
}

/// 把一个流式 chunk 应用到累加器；返回本次的文字增量（若有），供打字机回调。纯函数，可测。
///
/// 同时记录是否收到 `finish_reason`（服务端明确宣告生成结束）。GLM/DeepSeek 等推理模型会先流
/// `reasoning_content`（本函数**忽略**，不进入 text/tool 拼装），再流 `content`/`tool_calls`。
fn apply_stream_chunk(acc: &mut StreamAcc, v: &Value) -> Option<StreamDelta> {
    // usage 捕获必须在 `choice?` 早退**之前**：独立 usage 帧 `choices:[]`，走不到 choice
    // 分支（MLamp 实测两种形态并存——独立帧 / 与 finish_reason 同帧，逐帧覆盖取最后）。
    if let Some(u) = v.get("usage")
        && let Some(u) = parse_usage_json(u)
    {
        acc.usage = Some(u);
    }

    let choice = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());

    // 完成判据：服务端发来 `finish_reason`（stop / tool_calls / length …）即认为本回合生成结束。
    // 即使随后 HTTP 连接被掐断，已收到的内容也是完整语义，可安全采纳（不再重试）。
    if let Some(fr) = choice.and_then(|c0| c0.get("finish_reason")).and_then(|x| x.as_str())
        && !fr.is_empty() {
            acc.finished = true;
        }

    let c0 = choice?;
    let delta = c0.get("delta")?;

    // 工具调用分片：按 index 拼 id/name/arguments
    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tcs {
            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let slot = acc.slot(idx);
            if let Some(id) = tc.get("id").and_then(|x| x.as_str())
                && !id.is_empty() {
                    slot.id = id.to_string();
                }
            if let Some(f) = tc.get("function") {
                if let Some(name) = f.get("name").and_then(|x| x.as_str())
                    && !name.is_empty() {
                        slot.name = name.to_string();
                    }
                if let Some(args) = f.get("arguments").and_then(|x| x.as_str()) {
                    slot.args.push_str(args);
                }
            }
        }
    }

    // 思考过程增量 + 文字增量：两者各自累计。部分网关（如 GLM 推理模型）会把两类并进
    // 同一帧 delta——不能因先命中 reasoning 就 early-return 丢掉同帧正文。返回值只能带
    // 一种增量：同帧并存时返回正文（思考卡丢一帧增量，但数据一个字都不丢）。
    let mut reasoning_delta = None;
    if let Some(content) = delta
        .get("reasoning_content")
        .and_then(|c| c.as_str())
        && !content.is_empty()
    {
        acc.reasoning.push_str(content);
        reasoning_delta = Some(content.to_string());
    }

    // 文字增量
    if let Some(content) = delta.get("content").and_then(|c| c.as_str())
        && !content.is_empty() {
            acc.text.push_str(content);
            return Some(StreamDelta::Text(content.to_string()));
        }
    reasoning_delta.map(StreamDelta::Reasoning)
}

/// 单次 SSE 读取尝试：逐行消费响应体，把增量灌进 `acc`。
/// 返回 `true` = 收到 `[DONE]`；`false` = 到达正常体结尾但没有 `[DONE]`（此时须看 `acc.finished` 决定采纳/重试）。
async fn read_sse_once(
    resp: reqwest::Response,
    acc: &mut StreamAcc,
    observer: &dyn cmx_agent_core::TurnObserver,
) -> Result<bool, ModelError> {
    use futures_util::StreamExt;

    // 逐块读取 SSE：按 '\n' 分行（0x0A 不会出现在 UTF-8 多字节序列中，可安全切字节）。
    let mut buf: Vec<u8> = Vec::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let bytes = chunk.map_err(|e| err(format!("读取流失败: {e}")))?;
        if observer.is_cancelled() {
            return Err(ModelError("__turn_cancelled__".to_string()));
        }
        buf.extend_from_slice(&bytes);
        while let Some(pos) = buf.iter().position(|&b| b == b'\n') {
            let line_bytes: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line_bytes);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else {
                continue;
            };
            let data = data.trim();
            if data == "[DONE]" {
                return Ok(true);
            }
            if let Ok(v) = serde_json::from_str::<Value>(data)
                && let Some(delta) = apply_stream_chunk(acc, &v)
            {
                match delta {
                    StreamDelta::Text(text) if !text.is_empty() => observer.on_text_delta(&text),
                    StreamDelta::Reasoning(text) if !text.is_empty() => {
                        observer.on_reasoning_delta(&text)
                    }
                    _ => {}
                }
                    }
        }
    }
    // 流正常收尾但没见 [DONE]：交给上层判据（acc.finished）。
    Ok(false)
}


impl OpenAiCompatModel {
    /// 非流式 chat/completions 一次往返（complete / complete_bounded 共用体）。
    async fn post_chat(
        &self,
        ctx: &ModelContext,
        max_tokens: Option<u64>,
    ) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/chat/completions", self.cfg.base_url);
        let body = build_request_body(&self.cfg, ctx, max_tokens);
        let mut req = self.client.post(&url).json(&body);
        if !self.cfg.api_key.is_empty() {
            req = req.bearer_auth(&self.cfg.api_key);
        }
        let resp = req.send().await.map_err(|e| err(format!("请求模型失败: {e}")))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| err(format!("解析模型响应失败: {e}")))?;
        if !status.is_success() {
            // 优先取上游错误信息（带状态码标注供七类分类），无信封回退状态码文案
            return Err(match v.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()) {
                Some(m) => err(format!("HTTP {}: {m}", status.as_u16())),
                None => err(format!("模型服务返回 HTTP {}", status.as_u16())),
            });
        }
        parse_response(&v)
    }
}

#[async_trait]
impl ModelSeam for OpenAiCompatModel {
    async fn complete(&self, ctx: &ModelContext) -> Result<ModelResponse, ModelError> {
        self.post_chat(ctx, None).await
    }

    /// 摘要等合成请求：请求体附 `max_tokens`（压缩方案 §4.2.4.3）。
    async fn complete_bounded(
        &self,
        ctx: &ModelContext,
        max_tokens: u64,
    ) -> Result<ModelResponse, ModelError> {
        self.post_chat(ctx, Some(max_tokens)).await
    }

    async fn complete_streaming(
        &self,
        ctx: &ModelContext,
        observer: &dyn cmx_agent_core::TurnObserver,
    ) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/chat/completions", self.cfg.base_url);
        let mut body = build_request_body(&self.cfg, ctx, None);
        body["stream"] = serde_json::json!(true);
        // 用量感知（压缩方案 §4.1.2）：请求服务端回传 usage 帧。个别网关不认该参数会 4xx，
        // 报错文本含 stream_options 时**从 body 移除该键**重试一次（须真移除——下方断流重试
        // 复用同一 body，只改局部变量后续尝试仍带坏参数）。
        body["stream_options"] = serde_json::json!({"include_usage": true});
        let mut usage_param_stripped = false;

        // 健壮性：单次模型生成自愈。GLM/DeepSeek 等推理模型会先流长段 reasoning 再出正文，流很长；
        // 网关/负载均衡器偶发在长流中途切断 HTTP/2（reqwest 抛 Kind::Decode「读取流失败」）。
        // 处理：**流中途断也纳入自愈**——若已收到服务端 finish_reason（生成真正结束，掐断只丢了
        // 收尾标记）→ 采纳已收内容；否则整轮重试（最多 3 次），重试前经 observer.on_stream_reset
        // 通知前端清掉已显示的半截文字。重试耗尽时回报最后一次真实流错误（比笼统文案更可诊断）。
        const MAX_ATTEMPTS: usize = 3;
        let mut attempt = 0usize;
        let mut last_err: Option<ModelError> = None;
        loop {
            attempt += 1;

            if observer.is_cancelled() {
                return Err(ModelError("__turn_cancelled__".to_string()));
            }

            let mut req = self.client.post(&url).json(&body);
            if !self.cfg.api_key.is_empty() {
                req = req.bearer_auth(&self.cfg.api_key);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| err(format!("请求模型失败: {e}")))?;

            let status = resp.status();
            if status.is_success() {
                let mut acc = StreamAcc::default();
                let got_done = match read_sse_once(resp, &mut acc, observer).await {
                    Ok(done) => Some(done),
                    Err(e) => {
                        if acc.finished {
                            // 服务端已宣告生成结束：掐断只影响收尾标记，已收内容语义完整，采纳。
                            return Ok(acc.into_response());
                        }
                        if observer.is_cancelled() {
                            return Err(ModelError("__turn_cancelled__".to_string()));
                        }
                        last_err = Some(e); // 中途断且未完成 → 记下错误，走整轮重试
                        None
                    }
                };
                if got_done == Some(true) || acc.finished {
                    // 收到 [DONE] 或服务端 finish_reason → 生成完整，采纳。
                    return Ok(acc.into_response());
                }
                // 流正常收尾但既无 [DONE] 也无 finish_reason：视为一次不完整尝试，继续重试。
            } else if let Some(msg) = upstream_error_message(resp).await {
                // stream_options 不被网关认领 → 剥参重试一次（计一次尝试，防不认领网关死循环）
                if !usage_param_stripped && msg.0.to_ascii_lowercase().contains("stream_options") {
                    usage_param_stripped = true;
                    if let Some(obj) = body.as_object_mut() {
                        obj.remove("stream_options");
                    }
                    if attempt >= MAX_ATTEMPTS {
                        return Err(msg);
                    }
                    continue;
                }
                return Err(msg); // 服务端明确报错（如 401）——重试无意义，直接抛出
            } else {
                return Err(err(format!("模型服务返回 HTTP {}", status.as_u16())));
            }

            // 尝试耗尽：优先回报最后一次真实流错误（友好化已在构造点完成）；
            // 否则说明只是"不完整"而非断流。
            if attempt >= MAX_ATTEMPTS {
                return Err(last_err.unwrap_or_else(|| {
                    err(format!(
                        "模型响应中断，已自动重试 {MAX_ATTEMPTS} 次仍无完整响应，请稍后再试"
                    ))
                }));
            }
            // 重试前清掉前端已显示的半截文字，避免重试后重复/错位。
            observer.on_stream_reset();
            observer.on_reasoning_reset();
        }
    }
}

/// 非成功响应时尽力读取上游错误体（取得所有权）；带状态码标注返回（供七类分类认准
/// auth/404/429 等）。无信封 message 时返回 None，调用方回退 HTTP 状态码文案。
async fn upstream_error_message(resp: reqwest::Response) -> Option<ModelError> {
    let status = resp.status();
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    let m = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|x| x.as_str())?;
    Some(err(format!("HTTP {}: {m}", status.as_u16())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::TurnObserver;
    use cmx_agent_core::tool::ToolSpec;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cfg() -> ModelProviderConfig {
        ModelProviderConfig::new("http://x", "k", "deepseek-chat")
    }

    /// 计数观察者：记录文字增量与流重置次数。
    struct CountingObserver {
        resets: AtomicUsize,
    }
    impl TurnObserver for CountingObserver {
        fn on_text_delta(&self, _d: &str) {}
        fn on_stream_reset(&self) {
            self.resets.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// 本地假 SSE 服务器：第 1 个连接声明 Content-Length 但只发半截就断流（复现网关掐流，
    /// reqwest 抛 Kind::Decode「读取流失败」）；第 2 个连接回完整 SSE（含 [DONE]）。
    /// 回归验证 complete_streaming 的自愈重试：断流不再直接抛错，重试后拿到完整文本。
    #[tokio::test]
    async fn mid_stream_abort_is_retried_not_fatal() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            // —— 连接 1：半截响应 + 断流 ——
            let (mut s1, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = s1.read(&mut buf).await; // 消费请求（本地一回包内含头+体）
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\n\r\n";
            let body = "data: {\"choices\":[{\"delta\":{\"content\":\"半截\"},\"finish_reason\":null}]}\n\n";
            s1.write_all(head.as_bytes()).await.unwrap();
            s1.write_all(body.as_bytes()).await.unwrap();
            let _ = s1.flush().await;
            drop(s1); // 未达 Content-Length 即断 → reqwest 解码错误（复现掐流）

            // —— 连接 2：完整 SSE ——
            let (mut s2, _) = listener.accept().await.unwrap();
            let _ = s2.read(&mut buf).await;
            let resp = concat!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"完整回复\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
                "data: [DONE]\n\n",
            );
            s2.write_all(resp.as_bytes()).await.unwrap();
            let _ = s2.flush().await;
            drop(s2);
        });

        let model = OpenAiCompatModel::new(ModelProviderConfig::new(
            format!("http://{addr}"),
            "k",
            "test-model",
        ));
        let ctx = ModelContext {
            system: None,
            messages: vec![ModelMessage::User { text: "hi".into() }],
            tools: vec![],
        };
        let observer = CountingObserver { resets: AtomicUsize::new(0) };
        let r = model.complete_streaming(&ctx, &observer).await;
        server.await.unwrap();
        assert!(r.is_ok(), "断流一次后应自愈重试成功: {r:?}");
        let resp = r.unwrap();
        assert_eq!(resp.text.as_deref(), Some("完整回复"), "重试后应拿到第二次的完整文本");
        assert_eq!(observer.resets.load(Ordering::SeqCst), 1, "重试前应通知前端清屏一次");
    }

    /// 断流但服务端**已发 finish_reason**：掐断只丢了收尾标记，已收内容应被直接采纳（不重试）。
    #[tokio::test]
    async fn abort_after_finish_reason_adopts_partial() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut s, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 8192];
            let _ = s.read(&mut buf).await;
            let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: 1000\r\n\r\n";
            let body = concat!(
                "data: {\"choices\":[{\"delta\":{\"content\":\"语义完整\"},\"finish_reason\":null}]}\n\n",
                "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            );
            s.write_all(head.as_bytes()).await.unwrap();
            s.write_all(body.as_bytes()).await.unwrap();
            let _ = s.flush().await;
            drop(s); // finish_reason 已到但流被掐
        });

        let model = OpenAiCompatModel::new(ModelProviderConfig::new(
            format!("http://{addr}"),
            "k",
            "test-model",
        ));
        let ctx = ModelContext {
            system: None,
            messages: vec![ModelMessage::User { text: "hi".into() }],
            tools: vec![],
        };
        let observer = CountingObserver { resets: AtomicUsize::new(0) };
        let r = model.complete_streaming(&ctx, &observer).await;
        server.await.unwrap();
        assert!(r.is_ok(), "已收 finish_reason 的断流应采纳: {r:?}");
        assert_eq!(r.unwrap().text.as_deref(), Some("语义完整"));
        assert_eq!(observer.resets.load(Ordering::SeqCst), 0, "采纳场景不应触发重试清屏");
    }

    #[test]
    fn request_includes_system_user_and_tools() {
        let ctx = ModelContext {
            system: Some("你是助手".into()),
            messages: vec![ModelMessage::User { text: "算 2+3".into() }],
            tools: vec![
                ToolSpec::new("add", "相加")
                    .schema(json!({"type":"object","properties":{"a":{"type":"number"}}})),
            ],
        };
        let b = build_request_body(&cfg(), &ctx, None);
        assert_eq!(b["model"], "deepseek-chat");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][1]["role"], "user");
        assert_eq!(b["messages"][1]["content"], "算 2+3");
        assert_eq!(b["tools"][0]["function"]["name"], "add");
        assert_eq!(b["tool_choice"], "auto");
    }

    #[test]
    fn request_encodes_assistant_toolcall_and_tool_result() {
        let ctx = ModelContext {
            system: None,
            messages: vec![
                ModelMessage::Assistant {
                    text: None,
                    tool_calls: vec![ToolCall::with_id("c1", "add", json!({"a":2,"b":3}))],
                },
                ModelMessage::Tool {
                    call_id: "c1".into(),
                    output: json!({"sum": 5}),
                },
            ],
            tools: vec![],
        };
        let b = build_request_body(&cfg(), &ctx, None);
        let asst = &b["messages"][0];
        assert_eq!(asst["role"], "assistant");
        assert!(asst["content"].is_null());
        assert_eq!(asst["tool_calls"][0]["id"], "c1");
        assert_eq!(asst["tool_calls"][0]["function"]["name"], "add");
        // arguments 必须是 JSON 字符串
        assert!(asst["tool_calls"][0]["function"]["arguments"].is_string());
        let toolmsg = &b["messages"][1];
        assert_eq!(toolmsg["role"], "tool");
        assert_eq!(toolmsg["tool_call_id"], "c1");
        // 无 tools 时不带 tools 字段
        assert!(b.get("tools").is_none());
    }

    #[test]
    fn parse_plain_text() {
        let v = json!({"choices":[{"message":{"role":"assistant","content":"你好"}}]});
        let r = parse_response(&v).unwrap();
        assert_eq!(r.text.as_deref(), Some("你好"));
        assert!(r.tool_calls.is_empty());
    }

    #[test]
    fn parse_tool_call() {
        let v = json!({"choices":[{"message":{"role":"assistant","content":null,
            "tool_calls":[{"id":"c1","type":"function","function":{"name":"add","arguments":"{\"a\":2,\"b\":3}"}}]}}]});
        let r = parse_response(&v).unwrap();
        assert!(r.text.is_none());
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "add");
        assert_eq!(r.tool_calls[0].input["a"], 2);
        assert_eq!(r.tool_calls[0].input["b"], 3);
    }

    #[test]
    fn parse_malformed_arguments_drops_tool_call() {
        let v = json!({"choices":[{"message":{"tool_calls":[
            {"id":"c1","function":{"name":"x","arguments":"not-json"}}]}}]});
        let r = parse_response(&v).unwrap();
        assert!(
            r.tool_calls.is_empty(),
            "坏 JSON 参数的调用应被丢弃（不再以 _raw 照跑——残参会进错误消息/日志）"
        );
    }

    #[test]
    fn parse_upstream_error() {
        let v = json!({"error":{"message":"invalid api key"}});
        let e = parse_response(&v).unwrap_err();
        assert!(e.0.contains("invalid api key"));
    }

    #[test]
    fn stream_accumulates_text_deltas() {
        let mut acc = StreamAcc::default();
        let mut deltas = vec![];
        for c in ["你", "好", "，世界"] {
            let v = json!({"choices":[{"delta":{"content":c}}]});
            if let Some(d) = apply_stream_chunk(&mut acc, &v) {
                deltas.push(d);
            }
        }
        assert_eq!(
            deltas,
            vec![
                StreamDelta::Text("你".into()),
                StreamDelta::Text("好".into()),
                StreamDelta::Text("，世界".into())
            ]
        );
        let r = acc.into_response();
        assert_eq!(r.text.as_deref(), Some("你好，世界"));
        assert!(r.tool_calls.is_empty());
    }

    #[test]
    fn stream_accumulates_fragmented_tool_call() {
        let mut acc = StreamAcc::default();
        // 工具调用分片：id/name 首帧，arguments 分多帧拼
        let frames = vec![
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"add","arguments":""}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"a\":2,"}}]}}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"\"b\":3}"}}]}}]}),
        ];
        for f in &frames {
            let d = apply_stream_chunk(&mut acc, f);
            assert!(d.is_none(), "工具分片不应产出文字增量");
        }
        let r = acc.into_response();
        assert!(r.text.is_none());
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "add");
        assert_eq!(r.tool_calls[0].id, "call_1");
        assert_eq!(r.tool_calls[0].input["a"], 2);
        assert_eq!(r.tool_calls[0].input["b"], 3);
    }

    #[test]
    fn stream_marks_finished_on_finish_reason() {
        // 生成结束判据：服务端发来 finish_reason（哪怕迟于 [DONE] 前、或与 usage 同帧）。
        let mut acc = StreamAcc::default();
        apply_stream_chunk(&mut acc, &json!({"choices":[{"delta":{"content":"你好"}}]}));
        assert!(!acc.finished, "仅有内容增量时不应标记完成");
        apply_stream_chunk(
            &mut acc,
            &json!({"choices":[{"delta":{},"finish_reason":"stop"}]}),
        );
        assert!(acc.finished, "收到 finish_reason 应标记完成");
        let r = acc.into_response();
        assert_eq!(r.text.as_deref(), Some("你好"));
    }

    #[test]
    fn stream_ignores_reasoning_content_before_tool_calls() {
        // GPT 推理模型先流 reasoning_content 再给出工具调用——reasoning 不得污染 text/tool 累加。
        let mut acc = StreamAcc::default();
        apply_stream_chunk(&mut acc, &json!({"choices":[{"delta":{"reasoning_content":"让我想想"}}]}));
        apply_stream_chunk(
            &mut acc,
            &json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"add","arguments":"{\"a\":2,\"b\":3}"}}]}}]}),
        );
        let r = acc.into_response();
        assert!(r.text.is_none(), "reasoning 不应进入 text");
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].input["a"], 2);
    }

    #[test]
    fn stream_interrupted_before_done_still_adopts_when_finished() {
        // 健壮性核心：流在 [DONE] 前被掐断，但只要收到过 finish_reason，已收内容视为完整、可采纳。
        let mut acc = StreamAcc::default();
        apply_stream_chunk(&mut acc, &json!({"choices":[{"delta":{"content":"结果"}}]}));
        apply_stream_chunk(
            &mut acc,
            &json!({"choices":[{"delta":{"content":"是5"},"finish_reason":"stop"}]}),
        );
        // 模拟外层丢弃该次流（无 [DONE]）；判据只看 acc.finished。
        assert!(acc.finished);
        let r = acc.into_response();
        assert_eq!(r.text.as_deref(), Some("结果是5"));
    }

    /// 联网集成测试（`cargo test -p cmx-agent-model live_gateway_streaming -- --ignored` 用真实网关验证流式重试）。
    #[tokio::test]
    #[ignore = "需要真实 MLamp 网关凭据，仅供手动验证"]
    async fn live_gateway_streaming() {
        use cmx_agent_core::TurnObserver;

        // 与 model.json 相同的网关凭据；此测试默认不跑，需要时显式指定（不入库敏感信息）。
        let base_url = std::env::var("CMX_LIVE_BASE_URL").unwrap_or_else(|_| "https://llmgw-bz.mlamp.cn/v1".into());
        let api_key = std::env::var("CMX_LIVE_API_KEY").unwrap_or_default();
        let model = std::env::var("CMX_LIVE_MODEL").unwrap_or_else(|_| "mlamp/glm-5.2".into());
        let cfg = ModelProviderConfig::new(base_url, api_key, model);
        let seam = OpenAiCompatModel::new(cfg);

        struct NoopObserver;
        impl TurnObserver for NoopObserver {
            fn on_text_delta(&self, _d: &str) {}
        }

        let ctx = ModelContext {
            system: Some("你是助手，可调用工具。".into()),
            messages: vec![
                cmx_agent_core::model::ModelMessage::User { text: "帮我把 2 和 3 相加".into() },
            ],
            tools: vec![
                cmx_agent_core::tool::ToolSpec::new("add", "相加")
                    .schema(json!({"type":"object","properties":{"a":{"type":"number"},"b":{"type":"number"}},"required":["a","b"]})),
            ],
        };
        let r = seam.complete_streaming(&ctx, &NoopObserver).await;
        assert!(r.is_ok(), "流式应成功: {r:?}");
        let r = r.unwrap();
        // 要么有文字、要么有工具调用（glm-5.2 会先 reasoning 再工具调用）。
        assert!(r.text.is_some() || !r.tool_calls.is_empty(), "应产出文字或工具调用");
    }
}
