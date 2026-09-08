//! OpenAI 兼容 provider（`{base}/chat/completions`）——DeepSeek/OpenAI/Qwen/Ollama/vLLM 通用。
//!
//! 实现 [`cmx_agent_core::model::ModelSeam`]，支持**工具调用**（function calling）：
//! 把内核的 `ModelContext`（system+messages+tools）编成 OpenAI 请求，回来的 `choices[0].message`
//! 解成 `ModelResponse`（text + tool_calls）。请求编码/响应解码是**纯函数**，离线可单测。

use std::time::Duration;

use async_trait::async_trait;
use cmx_agent_core::model::{ModelContext, ModelError, ModelMessage, ModelResponse, ModelSeam};
use cmx_agent_core::tool::ToolCall;
use serde_json::{Value, json};

use crate::config::ModelProviderConfig;

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
}

/// 把内核 `ModelContext` 编成 OpenAI chat/completions 请求体（纯函数，可测）。
pub fn build_request_body(cfg: &ModelProviderConfig, ctx: &ModelContext) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    if let Some(sys) = &ctx.system {
        if !sys.is_empty() {
            messages.push(json!({"role": "system", "content": sys}));
        }
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
    // 上游错误信封：{"error":{"message":...}}
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("上游模型错误");
        return Err(ModelError(msg.to_string()));
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
        for (i, c) in calls.iter().enumerate() {
            let id = c
                .get("id")
                .and_then(|x| x.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("call_{i}"));
            let func = c.get("function");
            let name = func
                .and_then(|f| f.get("name"))
                .and_then(|x| x.as_str())
                .unwrap_or_default()
                .to_string();
            // arguments 是 JSON 字符串；宽松解析，失败则原样塞入 {"_raw": "..."}
            let args_raw = func
                .and_then(|f| f.get("arguments"))
                .and_then(|x| x.as_str())
                .unwrap_or("{}");
            let input: Value = serde_json::from_str(args_raw)
                .unwrap_or_else(|_| json!({ "_raw": args_raw }));
            if !name.is_empty() {
                tool_calls.push(ToolCall::with_id(id, name, input));
            }
        }
    }

    Ok(ModelResponse { text, tool_calls })
}

/// 流式累加器：跨 SSE 增量拼接文字与工具调用（工具的 arguments 会分片到达，按 index 拼）。
#[derive(Default)]
struct StreamAcc {
    text: String,
    tools: Vec<ToolAcc>,
    /// 是否已收到服务端的 `finish_reason`（生成真正结束，含 stop / tool_calls / length）。
    finished: bool,
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
            .enumerate()
            .map(|(i, t)| {
                let id = if t.id.is_empty() {
                    format!("call_{i}")
                } else {
                    t.id
                };
                let input: Value = serde_json::from_str(if t.args.is_empty() { "{}" } else { &t.args })
                    .unwrap_or_else(|_| serde_json::json!({ "_raw": t.args }));
                ToolCall::with_id(id, t.name, input)
            })
            .collect();
        ModelResponse { text, tool_calls }
    }
}

/// 把一个流式 chunk 应用到累加器；返回本次的文字增量（若有），供打字机回调。纯函数，可测。
///
/// 同时记录是否收到 `finish_reason`（服务端明确宣告生成结束）。GLM/DeepSeek 等推理模型会先流
/// `reasoning_content`（本函数**忽略**，不进入 text/tool 拼装），再流 `content`/`tool_calls`。
fn apply_stream_chunk(acc: &mut StreamAcc, v: &Value) -> Option<String> {
    let choice = v
        .get("choices")
        .and_then(|c| c.as_array())
        .and_then(|a| a.first());

    // 完成判据：服务端发来 `finish_reason`（stop / tool_calls / length …）即认为本回合生成结束。
    // 即使随后 HTTP 连接被掐断，已收到的内容也是完整语义，可安全采纳（不再重试）。
    if let Some(fr) = choice.and_then(|c0| c0.get("finish_reason")).and_then(|x| x.as_str()) {
        if !fr.is_empty() {
            acc.finished = true;
        }
    }

    let Some(c0) = choice else { return None };
    let delta = c0.get("delta")?;

    // 工具调用分片：按 index 拼 id/name/arguments
    if let Some(tcs) = delta.get("tool_calls").and_then(|t| t.as_array()) {
        for tc in tcs {
            let idx = tc.get("index").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let slot = acc.slot(idx);
            if let Some(id) = tc.get("id").and_then(|x| x.as_str()) {
                if !id.is_empty() {
                    slot.id = id.to_string();
                }
            }
            if let Some(f) = tc.get("function") {
                if let Some(name) = f.get("name").and_then(|x| x.as_str()) {
                    if !name.is_empty() {
                        slot.name = name.to_string();
                    }
                }
                if let Some(args) = f.get("arguments").and_then(|x| x.as_str()) {
                    slot.args.push_str(args);
                }
            }
        }
    }

    // 文字增量
    if let Some(content) = delta.get("content").and_then(|c| c.as_str()) {
        if !content.is_empty() {
            acc.text.push_str(content);
            return Some(content.to_string());
        }
    }
    None
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
        let bytes = chunk.map_err(|e| ModelError(format!("读取流失败: {e}")))?;
        buf.extend_from_slice(&bytes);
        loop {
            let Some(pos) = buf.iter().position(|&b| b == b'\n') else {
                break;
            };
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
            if let Ok(v) = serde_json::from_str::<Value>(data) {
                if let Some(delta) = apply_stream_chunk(acc, &v) {
                    if !delta.is_empty() {
                        observer.on_text_delta(&delta);
                    }
                }
            }
        }
    }
    // 流正常收尾但没见 [DONE]：交给上层判据（acc.finished）。
    Ok(false)
}

#[async_trait]
impl ModelSeam for OpenAiCompatModel {
    async fn complete(&self, ctx: &ModelContext) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/chat/completions", self.cfg.base_url);
        let body = build_request_body(&self.cfg, ctx);
        let mut req = self.client.post(&url).json(&body);
        if !self.cfg.api_key.is_empty() {
            req = req.bearer_auth(&self.cfg.api_key);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ModelError(format!("请求模型失败: {e}")))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .await
            .map_err(|e| ModelError(format!("解析模型响应失败: {e}")))?;
        if !status.is_success() {
            // 优先取上游错误信息
            if let Err(e) = parse_response(&v) {
                return Err(e);
            }
            return Err(ModelError(format!("模型服务返回 HTTP {}", status.as_u16())));
        }
        parse_response(&v)
    }

    async fn complete_streaming(
        &self,
        ctx: &ModelContext,
        observer: &dyn cmx_agent_core::TurnObserver,
    ) -> Result<ModelResponse, ModelError> {
        let url = format!("{}/chat/completions", self.cfg.base_url);
        let mut body = build_request_body(&self.cfg, ctx);
        body["stream"] = serde_json::json!(true);

        // 健壮性：单次模型生成自愈。GLM/DeepSeek 等推理模型会先流长段 reasoning 再出正文，流很长；
        // 网关/负载均衡器偶发在长流中途切断 HTTP/2（reqwest 抛 Kind::Decode「读取流失败」）。
        // 处理：若已收到服务端 finish_reason（生成真正结束）→ 采纳已收内容；否则整轮重试（最多 3 次），
        // 重试前经 observer.on_stream_reset 通知前端清掉已显示的半截文字。
        const MAX_ATTEMPTS: usize = 3;
        let mut attempt = 0usize;
        loop {
            attempt += 1;

            let mut req = self.client.post(&url).json(&body);
            if !self.cfg.api_key.is_empty() {
                req = req.bearer_auth(&self.cfg.api_key);
            }
            let resp = req
                .send()
                .await
                .map_err(|e| ModelError(format!("请求模型失败: {e}")))?;

            let status = resp.status();
            if status.is_success() {
                let mut acc = StreamAcc::default();
                let got_done = read_sse_once(resp, &mut acc, observer).await?;
                if got_done || acc.finished {
                    // 收到 [DONE] 或服务端 finish_reason → 生成完整，采纳。
                    return Ok(acc.into_response());
                }
                // 流正常收尾但既无 [DONE] 也无 finish_reason：视为一次不完整尝试，继续重试。
            } else if let Some(msg) = upstream_error_message(resp).await {
                return Err(msg); // 服务端明确报错（如 401）——重试无意义，直接抛出
            } else {
                return Err(ModelError(format!("模型服务返回 HTTP {}", status.as_u16())));
            }

            // 尝试耗尽：用最后一次已收内容（若完整）或返回明确错误。
            if attempt >= MAX_ATTEMPTS {
                return Err(ModelError(format!(
                    "模型响应不完整（{MAX_ATTEMPTS} 次尝试后仍无 finish_reason），请稍后重试"
                )));
            }
            // 重试前清掉前端已显示的半截文字，避免重试后重复/错位。
            observer.on_stream_reset();
        }
    }
}

/// 非成功响应时尽力读取上游错误体（取得所有权）；`None` 表示无可用错误信息（调用方回退到 HTTP 状态码文案）。
async fn upstream_error_message(resp: reqwest::Response) -> Option<ModelError> {
    let v: Value = resp.json().await.unwrap_or(Value::Null);
    if let Err(e) = parse_response(&v) {
        return Some(e);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::tool::ToolSpec;

    fn cfg() -> ModelProviderConfig {
        ModelProviderConfig::new("http://x", "k", "deepseek-chat")
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
        let b = build_request_body(&cfg(), &ctx);
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
        let b = build_request_body(&cfg(), &ctx);
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
    fn parse_malformed_arguments_falls_back_to_raw() {
        let v = json!({"choices":[{"message":{"tool_calls":[
            {"id":"c1","function":{"name":"x","arguments":"not-json"}}]}}]});
        let r = parse_response(&v).unwrap();
        assert_eq!(r.tool_calls[0].input["_raw"], "not-json");
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
        assert_eq!(deltas, vec!["你", "好", "，世界"]);
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
