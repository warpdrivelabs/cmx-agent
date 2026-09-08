//! 飞书（Feishu/Lark）IM provider：[`ImProvider`] 的 **Stream 长连接** 实现。
//!
//! 与 Telegram 长轮询不同，飞书 Stream 是 agent 主动连飞书 websocket 拉消息——**无需公网回调 URL、
//! 无需内网穿透、零 AES/SHA crypto 依赖**（`unsafe_code = "forbid"` 保持）。范式仍是「出站连接拉取」，
//! 与 [`TelegramProvider`] 同构，桥与内核零改动。
//!
//! 协议（对照 lark oapi-sdk-go `ws/` 包）：
//! - **拿连接地址**：`POST {base}/callback/ws/endpoint`，body `{"AppID","AppSecret"}` → `data.URL`（wss）。
//! - **帧**：protobuf `Frame`（`pbbp2.proto`）。`method=0` 控制（ping/pong）；`method=1` 数据（event）。
//!   headers 含 `type=event|ping|pong`、`message_id`、`seq` 等。
//! - **心跳**：客户端发 ping 控制帧，服务端回 pong（约 30s）。
//! - **收消息**：`type=event` 的数据帧，payload 为 v2 事件 JSON（`im.message.receive_v1`），
//!   取 `event.message.{chat_id, content}` → content 解出 `text`。回 ack 数据帧（`{"code":200}`）。
//! - **发消息**：`POST {base}/open-apis/im/v1/messages?receive_id_type=chat_id`，Bearer tenant_access_token
//!   （token 由 `auth/v3/tenant_access_token/internal` 换取并缓存）。
//!
//! 配置（env）：`CMX_AGENT_IM_FEISHU_APP_ID` + `CMX_AGENT_IM_FEISHU_APP_SECRET`（必需）+ 可选
//! `CMX_AGENT_IM_FEISHU_BASE`（默认 `https://open.feishu.cn`，海外用 `https://open.larksuite.com`）。
//! 白名单 `CMX_AGENT_IM_ALLOW` 填飞书 `chat_id`（如 `oc_xxx`）——由 CLI/桥统一处理，本 provider 不掺和。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};

use crate::{ImProvider, InboundMsg};

// ── 可调参数（常量集中，便于审阅/后续抽配置）──────────────────────────────
/// 飞书国内开放平台默认基址。海外用 `https://open.larksuite.com`（`CMX_AGENT_IM_FEISHU_BASE` 覆盖）。
const FEISHU_BASE_FEISHU: &str = "https://open.feishu.cn";
/// tenant_access_token 默认 TTL（飞书返回 expire，缺失时兜底 2h）。
const TOKEN_DEFAULT_TTL: u64 = 7200;
/// 客户端发 ping 间隔（飞书服务端下发 ClientConfig.PingInterval，本实现用固定值保活）。
const PING_INTERVAL: Duration = Duration::from_secs(30);
/// 断开后重连退避。
const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// 读 env，trim 后非空才返回（None 表未配置）。
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|v| {
        let v = v.trim().to_string();
        (!v.is_empty()).then_some(v)
    })
}

/// 飞书 Stream provider。常驻后台 task 持有 websocket；`poll` 从内部队列取已收消息。
///
/// 内部状态全 `Arc`，故 `start()` 克隆自身句柄入后台 task，不重建内层 provider（避免字段未来
/// 增加/忘拷导致状态分叉）。
pub struct FeishuProvider {
    inner: Arc<FeishuInner>,
}

/// 飞书 provider 共享状态（Arc 包裹，后台 task 与 poll 共用）。
struct FeishuInner {
    app_id: String,
    app_secret: String,
    base: String,
    client: reqwest::Client,
    /// Stream task → poll 的入站队列。
    inbox: Mutex<VecDeque<InboundMsg>>,
    /// 自增 update_id（飞书事件包无稳定 seq，本地自增即可，桥按 drain 去重）。
    seq: AtomicI64,
    /// tenant_access_token 缓存：(token, 过期时刻)。
    token: Mutex<Option<(String, Instant)>>,
    /// 已启动 Stream task 的去重标记（`start()` 仅首次 spawn）。
    started: AtomicBool,
}

impl FeishuProvider {
    pub fn new(app_id: impl Into<String>, app_secret: impl Into<String>, base: Option<String>) -> Self {
        Self {
            inner: Arc::new(FeishuInner {
                app_id: app_id.into(),
                app_secret: app_secret.into(),
                base: base.unwrap_or_else(|| FEISHU_BASE_FEISHU.into()),
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new()),
                inbox: Mutex::new(VecDeque::new()),
                seq: AtomicI64::new(0),
                token: Mutex::new(None),
                started: AtomicBool::new(false),
            }),
        }
    }

    /// 从 env 读取（`CMX_AGENT_IM_FEISHU_APP_ID` + `_APP_SECRET` 必需，`_BASE` 可选）。
    pub fn from_env() -> Option<Self> {
        let app_id = env_nonempty("CMX_AGENT_IM_FEISHU_APP_ID")?;
        let app_secret = env_nonempty("CMX_AGENT_IM_FEISHU_APP_SECRET")?;
        Some(Self::new(app_id, app_secret, std::env::var("CMX_AGENT_IM_FEISHU_BASE").ok()))
    }

    /// 测试注入：往入站队列塞一条消息（生产路径由 Stream task 调用）。
    pub async fn inject(&self, chat_id: &str, text: &str) {
        self.inject_with_sender(chat_id, text, "").await;
    }

    /// 测试注入（带 sender）：模拟飞书某 open_id 发消息，驱动绑定/鉴权路径测试。
    pub async fn inject_with_sender(&self, chat_id: &str, text: &str, sender: &str) {
        let id = self.inner.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner
            .inbox
            .lock()
            .await
            .push_back(InboundMsg {
                chat_id: chat_id.into(),
                text: text.into(),
                update_id: id,
                sender: sender.into(),
            });
    }

    /// 换取/刷新 tenant_access_token（缓存到过期前 60s）。
    async fn tenant_token(&self) -> Result<String, String> {
        {
            let t = self.inner.token.lock().await;
            if let Some((tok, exp)) = t.as_ref()
                && *exp > Instant::now() + Duration::from_secs(60)
            {
                return Ok(tok.clone());
            }
        }
        let resp = self
            .inner
            .client
            .post(format!("{}/open-apis/auth/v3/tenant_access_token/internal", self.inner.base.trim_end_matches('/')))
            .json(&json!({ "app_id": self.inner.app_id, "app_secret": self.inner.app_secret }))
            .send()
            .await
            .map_err(|e| format!("tenant_access_token 请求失败：{e}"))?;
        let v: Value = resp.json().await.map_err(|e| format!("tenant_access_token 解析失败：{e}"))?;
        let tok = v
            .get("tenant_access_token")
            .and_then(|x| x.as_str())
            .ok_or_else(|| format!("tenant_access_token 缺失：{v}"))?
            .to_string();
        let expire = v.get("expire").and_then(|x| x.as_u64()).unwrap_or(TOKEN_DEFAULT_TTL);
        *self.inner.token.lock().await = Some((tok.clone(), Instant::now() + Duration::from_secs(expire)));
        Ok(tok)
    }

    /// 拉取 websocket 连接地址。
    async fn fetch_endpoint(&self) -> Result<String, String> {
        let resp = self
            .inner
            .client
            .post(format!("{}/callback/ws/endpoint", self.inner.base.trim_end_matches('/')))
            .header("locale", "zh")
            .json(&json!({ "AppID": self.inner.app_id, "AppSecret": self.inner.app_secret }))
            .send()
            .await
            .map_err(|e| format!("endpoint 请求失败：{e}"))?;
        let v: Value = resp.json().await.map_err(|e| format!("endpoint 解析失败：{e}"))?;
        let code = v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
        if code != 0 {
            return Err(format!(
                "endpoint code={code} msg={}",
                v.get("msg").and_then(|m| m.as_str()).unwrap_or("")
            ));
        }
        let url = v
            .get("data")
            .and_then(|d| d.get("URL"))
            .and_then(|u| u.as_str())
            .ok_or("endpoint 响应缺 data.URL")?;
        Ok(url.to_string())
    }

    /// Stream 主循环：建连 → 收帧（ping/pong/event）→ 入队 + ack；出错由外层重连。
    async fn run_stream_once(&self) -> Result<(), String> {
        let url = self.fetch_endpoint().await?;
        let (ws, _resp) = connect_async(&url).await.map_err(|e| format!("ws 建连失败：{e}"))?;
        let (mut sink, mut stream) = ws.split();
        let mut ping = tokio::time::interval(PING_INTERVAL);
        ping.tick().await; // 跳过首次立即触发
        tracing::info!("feishu stream 已连接");
        loop {
            tokio::select! {
                msg = stream.next() => match msg {
                    Some(Ok(Message::Binary(b))) => {
                        let Some(frame) = decode_frame(&b) else { continue };
                        match frame.method {
                            0 => {
                                // 控制：ping→回 pong；pong→忽略（可能带 ClientConfig，暂不应用）
                                if get_header(&frame.headers, "type") == "ping" {
                                    let _ = sink.send(Message::Binary(make_pong_frame(frame.service))).await;
                                }
                            }
                            1 => {
                                let t = get_header(&frame.headers, "type");
                                if t == "event" {
                                    if let Ok(s) = std::str::from_utf8(&frame.payload)
                                        && let Some((chat_id, text, open_id)) = parse_receive_event(s)
                                    {
                                        let id = self.inner.seq.fetch_add(1, Ordering::SeqCst) + 1;
                                        self.inner
                                            .inbox
                                            .lock()
                                            .await
                                            .push_back(InboundMsg {
                                                chat_id,
                                                text,
                                                update_id: id,
                                                sender: open_id,
                                            });
                                    }
                                    // ack：回数据帧，payload {"code":200}，透传原 headers
                                    let _ = sink.send(Message::Binary(make_ack_frame(&frame))).await;
                                } else if t == "callback" {
                                    let _ = sink.send(Message::Binary(make_ack_frame(&frame))).await;
                                }
                            }
                            _ => {}
                        }
                    }
                    Some(Ok(Message::Ping(p))) => { let _ = sink.send(Message::Pong(p)).await; }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(format!("ws 读失败：{e}")),
                    None => return Err("ws 连接关闭".into()),
                },
                _ = ping.tick() => {
                    let _ = sink.send(Message::Binary(make_ping_frame(0))).await;
                }
            }
        }
    }

    /// 重连外壳：失败/断开 → 退避重试，永不放弃（对齐桥 `run()` 的退避语义）。
    async fn run_stream_loop(self: Arc<Self>) {
        loop {
            if let Err(e) = self.run_stream_once().await {
                tracing::warn!("feishu stream 断开：{e}，{}s 后重连", RECONNECT_BACKOFF.as_secs());
            }
            tokio::time::sleep(RECONNECT_BACKOFF).await;
        }
    }
}

#[async_trait]
impl ImProvider for FeishuProvider {
    async fn poll(&self, offset: i64) -> Result<(Vec<InboundMsg>, i64), String> {
        let mut q = self.inner.inbox.lock().await;
        // 只取 update_id > offset 的；保留未到游标的（队列本应单调，保留以求稳）。
        let mut out = Vec::new();
        let mut next = offset;
        while let Some(m) = q.front() {
            if m.update_id <= offset {
                // 已过游标：丢弃（防历史残留重复驱动）。
                q.pop_front();
                continue;
            }
            let m = q.pop_front().expect("front 已确认存在");
            next = next.max(m.update_id);
            out.push(m);
        }
        Ok((out, next))
    }

    async fn send(&self, chat_id: &str, text: &str) -> Result<(), String> {
        let token = self.tenant_token().await?;
        let resp = self
            .inner
            .client
            .post(format!(
                "{}/open-apis/im/v1/messages?receive_id_type=chat_id",
                self.inner.base.trim_end_matches('/')
            ))
            .bearer_auth(&token)
            .json(&build_send_body(chat_id, text))
            .send()
            .await
            .map_err(|e| format!("sendMessage 请求失败：{e}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("sendMessage HTTP {status}: {body}"));
        }
        Ok(())
    }

    /// 启动常驻 Stream 后台 task（飞书独有；长轮询 provider 用 trait 默认 no-op）。
    /// 幂等：重复调用只 spawn 一次（`started` CAS 守门）。
    async fn start(&self) -> Result<(), String> {
        if self.inner.start_or_reject() {
            // Clone 现有 Arc<FeishuInner> 入后台 task——不重建内层，状态天然共享。
            let me = Arc::clone(&self.inner);
            tokio::spawn(async move {
                let p = FeishuProvider { inner: me };
                Arc::new(p).run_stream_loop().await;
            });
        }
        Ok(())
    }
}

impl FeishuInner {
    /// CAS 守门：仅首次调用返回 true（已启动则 false，避免重复 spawn）。
    fn start_or_reject(&self) -> bool {
        self.started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

// ── 纯函数：消息解析 / 构造（可单测，无网络）──────────────────────────────

/// 从 v2 事件 payload 解析 `(chat_id, text, open_id)`。仅处理 `im.message.receive_v1` 文本消息。
/// open_id 取自 `event.sender.sender_id.open_id`，用于 IM 用户 ↔ 门户用户绑定（按 sender 查绑定身份）。
pub fn parse_receive_event(payload: &str) -> Option<(String, String, String)> {
    let v: Value = serde_json::from_str(payload).ok()?;
    let header = v.get("header")?;
    if header.get("event_type").and_then(|t| t.as_str()) != Some("im.message.receive_v1") {
        return None;
    }
    let event = v.get("event")?;
    let message = event.get("message")?;
    let chat_id = message.get("chat_id").and_then(|c| c.as_str())?.to_string();
    if message.get("message_type").and_then(|t| t.as_str()) != Some("text") {
        return None; // 一期仅文本
    }
    let content = message.get("content").and_then(|c| c.as_str())?;
    let text = serde_json::from_str::<Value>(content)
        .ok()?
        .get("text")
        .and_then(|t| t.as_str())?
        .to_string();
    if text.is_empty() {
        return None;
    }
    // sender open_id：event.sender.sender_id.open_id（缺失则空串，绑定按未绑定处理）。
    let open_id = event
        .get("sender")
        .and_then(|s| s.get("sender_id"))
        .and_then(|id| id.get("open_id"))
        .and_then(|o| o.as_str())
        .unwrap_or("")
        .to_string();
    Some((chat_id, text, open_id))
}

/// 构造发消息 body（`receive_id` / `msg_type` / `content`）。
pub fn build_send_body(chat_id: &str, text: &str) -> Value {
    json!({
        "receive_id": chat_id,
        "msg_type": "text",
        "content": serde_json::to_string(&json!({ "text": text })).unwrap_or_default(),
    })
}

fn get_header(headers: &[(String, String)], key: &str) -> String {
    headers
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}

// ── pbbp2 Frame protobuf 编解码（手写，免 prost 依赖）─────────────────────
//
// 线格式（对照 lark oapi-sdk-go `ws/pbbp2.pb.go`）：
//   Frame: 1=SeqID varint | 2=LogID varint | 3=service varint | 4=method varint
//          | 5=headers (repeated Header) | 6=payload_encoding string | 7=payload_type string
//          | 8=payload bytes | 9=LogIDNew string
//   Header: 1=key string | 2=value string
// tag = (field<<3)|wire（varint=0, length-delimited=2）。

#[derive(Debug, Clone, Default)]
struct Frame {
    seq_id: u64,
    log_id: u64,
    service: i32,
    method: i32,
    headers: Vec<(String, String)>,
    payload_encoding: String,
    payload_type: String,
    payload: Vec<u8>,
    log_id_new: String,
}

fn enc_varint(buf: &mut Vec<u8>, mut v: u64) {
    while v >= 0x80 {
        buf.push((v as u8) | 0x80);
        v >>= 7;
    }
    buf.push(v as u8);
}

fn enc_str(buf: &mut Vec<u8>, tag: u8, s: &str) {
    buf.push(tag);
    enc_varint(buf, s.len() as u64);
    buf.extend_from_slice(s.as_bytes());
}

fn enc_bytes(buf: &mut Vec<u8>, tag: u8, b: &[u8]) {
    buf.push(tag);
    enc_varint(buf, b.len() as u64);
    buf.extend_from_slice(b);
}

fn enc_varint_field(buf: &mut Vec<u8>, tag: u8, v: u64) {
    buf.push(tag);
    enc_varint(buf, v);
}

fn encode_header(h: &(String, String)) -> Vec<u8> {
    let mut b = Vec::new();
    enc_str(&mut b, 0xa, &h.0);
    enc_str(&mut b, 0x12, &h.1);
    b
}

fn encode_frame(f: &Frame) -> Vec<u8> {
    let mut b = Vec::new();
    enc_varint_field(&mut b, 0x8, f.seq_id);
    enc_varint_field(&mut b, 0x10, f.log_id);
    enc_varint_field(&mut b, 0x18, f.service as u64);
    enc_varint_field(&mut b, 0x20, f.method as u64);
    for h in &f.headers {
        enc_bytes(&mut b, 0x2a, &encode_header(h));
    }
    if !f.payload_encoding.is_empty() {
        enc_str(&mut b, 0x32, &f.payload_encoding);
    }
    if !f.payload_type.is_empty() {
        enc_str(&mut b, 0x3a, &f.payload_type);
    }
    if !f.payload.is_empty() {
        enc_bytes(&mut b, 0x42, &f.payload);
    }
    if !f.log_id_new.is_empty() {
        enc_str(&mut b, 0x4a, &f.log_id_new);
    }
    b
}

fn dec_varint(buf: &[u8], i: &mut usize) -> Option<u64> {
    let mut result = 0u64;
    let mut shift = 0u32;
    loop {
        if *i >= buf.len() {
            return None;
        }
        let b = buf[*i];
        *i += 1;
        result |= ((b & 0x7f) as u64) << shift;
        if b & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 63 {
            return None;
        }
    }
    Some(result)
}

fn dec_str(buf: &[u8], i: &mut usize) -> Option<String> {
    let len = dec_varint(buf, i)? as usize;
    let end = i.checked_add(len)?;
    if end > buf.len() {
        return None;
    }
    let s = std::str::from_utf8(&buf[*i..end]).ok()?.to_string();
    *i = end;
    Some(s)
}

fn dec_bytes(buf: &[u8], i: &mut usize) -> Option<Vec<u8>> {
    let len = dec_varint(buf, i)? as usize;
    let end = i.checked_add(len)?;
    if end > buf.len() {
        return None;
    }
    let b = buf[*i..end].to_vec();
    *i = end;
    Some(b)
}

fn dec_header(buf: &[u8]) -> Option<(String, String)> {
    let mut i = 0;
    let mut key = String::new();
    let mut val = String::new();
    while i < buf.len() {
        let tag = dec_varint(buf, &mut i)?;
        let field = (tag >> 3) as u32;
        let wire = (tag & 0x7) as u32;
        match (field, wire) {
            (1, 2) => key = dec_str(buf, &mut i)?,
            (2, 2) => val = dec_str(buf, &mut i)?,
            _ => skip_field(buf, &mut i, wire)?,
        }
    }
    Some((key, val))
}

fn skip_field(buf: &[u8], i: &mut usize, wire: u32) -> Option<()> {
    match wire {
        0 => {
            dec_varint(buf, i)?;
        }
        2 => {
            dec_bytes(buf, i)?;
        }
        1 => {
            if *i + 8 > buf.len() {
                return None;
            }
            *i += 8;
        }
        5 => {
            if *i + 4 > buf.len() {
                return None;
            }
            *i += 4;
        }
        _ => return None,
    }
    Some(())
}

fn decode_frame(buf: &[u8]) -> Option<Frame> {
    let mut f = Frame::default();
    let mut i = 0;
    while i < buf.len() {
        let tag = dec_varint(buf, &mut i)?;
        let field = (tag >> 3) as u32;
        let wire = (tag & 0x7) as u32;
        match (field, wire) {
            (1, 0) => f.seq_id = dec_varint(buf, &mut i)?,
            (2, 0) => f.log_id = dec_varint(buf, &mut i)?,
            (3, 0) => f.service = dec_varint(buf, &mut i)? as i32,
            (4, 0) => f.method = dec_varint(buf, &mut i)? as i32,
            (5, 2) => {
                let hb = dec_bytes(buf, &mut i)?;
                f.headers.push(dec_header(&hb)?);
            }
            (6, 2) => f.payload_encoding = dec_str(buf, &mut i)?,
            (7, 2) => f.payload_type = dec_str(buf, &mut i)?,
            (8, 2) => f.payload = dec_bytes(buf, &mut i)?,
            (9, 2) => f.log_id_new = dec_str(buf, &mut i)?,
            _ => skip_field(buf, &mut i, wire)?,
        }
    }
    Some(f)
}

fn make_ping_frame(service: i32) -> Vec<u8> {
    encode_frame(&Frame {
        method: 0,
        service,
        headers: vec![("type".into(), "ping".into())],
        ..Default::default()
    })
}

fn make_pong_frame(service: i32) -> Vec<u8> {
    encode_frame(&Frame {
        method: 0,
        service,
        headers: vec![("type".into(), "pong".into())],
        ..Default::default()
    })
}

fn make_ack_frame(orig: &Frame) -> Vec<u8> {
    let payload = serde_json::to_vec(&json!({ "code": 200 })).unwrap_or_default();
    encode_frame(&Frame {
        seq_id: orig.seq_id,
        log_id: orig.log_id,
        service: orig.service,
        method: 1,
        headers: orig.headers.clone(),
        payload,
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_text_receive_event() {
        let payload = r#"{"schema":"2.0","header":{"event_id":"e1","event_type":"im.message.receive_v1","create_time":"1"},"event":{"sender":{"sender_id":{"open_id":"o1"}},"message":{"message_id":"om_1","chat_id":"oc_abc","chat_type":"p2p","message_type":"text","content":"{\"text\":\"你好\"}"}}}"#;
        let (chat, text, open_id) = parse_receive_event(payload).expect("应解析出文本消息");
        assert_eq!(chat, "oc_abc");
        assert_eq!(text, "你好");
        assert_eq!(open_id, "o1");
    }

    #[test]
    fn parse_open_id_missing_is_empty() {
        // sender.sender_id.open_id 缺失：open_id 返空串（不阻断解析；绑定按未绑定处理）。
        let payload = r#"{"header":{"event_type":"im.message.receive_v1"},"event":{"message":{"chat_id":"oc_x","message_type":"text","content":"{\"text\":\"hi\"}"}}}"#;
        let (_, _, open_id) = parse_receive_event(payload).expect("应解析出文本消息");
        assert_eq!(open_id, "");
    }

    #[test]
    fn parse_skips_non_text_and_other_events() {
        let img = r#"{"header":{"event_type":"im.message.receive_v1"},"event":{"message":{"chat_id":"oc_x","message_type":"image","content":"{}"}}}"#;
        assert!(parse_receive_event(img).is_none());
        let other = r#"{"header":{"event_type":"im.message.message_read_v1"},"event":{"reader":{"chat_id":"oc_x"}}}"#;
        assert!(parse_receive_event(other).is_none());
        assert!(parse_receive_event("not json").is_none());
        // 空 text
        let empty = r#"{"header":{"event_type":"im.message.receive_v1"},"event":{"message":{"chat_id":"oc_x","message_type":"text","content":"{\"text\":\"\"}"}}}"#;
        assert!(parse_receive_event(empty).is_none());
    }

    #[test]
    fn send_body_shape() {
        let b = build_send_body("oc_1", "hi");
        assert_eq!(b["receive_id"], "oc_1");
        assert_eq!(b["msg_type"], "text");
        // content 是 JSON 字符串
        let c: Value = serde_json::from_str(b["content"].as_str().unwrap()).unwrap();
        assert_eq!(c["text"], "hi");
    }

    #[test]
    fn frame_roundtrip_event() {
        let orig = Frame {
            seq_id: 42,
            log_id: 7,
            service: 1,
            method: 1,
            headers: vec![("type".into(), "event".into()), ("message_id".into(), "om_9".into())],
            payload: br#"{"x":1}"#.to_vec(),
            ..Default::default()
        };
        let bytes = encode_frame(&orig);
        let dec = decode_frame(&bytes).expect("应解码");
        assert_eq!(dec.seq_id, 42);
        assert_eq!(dec.log_id, 7);
        assert_eq!(dec.service, 1);
        assert_eq!(dec.method, 1);
        assert_eq!(dec.headers.len(), 2);
        assert_eq!(dec.headers[0], ("type".into(), "event".into()));
        assert_eq!(dec.headers[1], ("message_id".into(), "om_9".into()));
        assert_eq!(dec.payload, br#"{"x":1}"#.to_vec());
    }

    #[test]
    fn ping_pong_ack_frames_decode() {
        let ping = make_ping_frame(1);
        let f = decode_frame(&ping).unwrap();
        assert_eq!(f.method, 0);
        assert_eq!(get_header(&f.headers, "type"), "ping");

        let pong = make_pong_frame(1);
        let f = decode_frame(&pong).unwrap();
        assert_eq!(f.method, 0);
        assert_eq!(get_header(&f.headers, "type"), "pong");

        let orig = Frame { method: 1, headers: vec![("type".into(), "event".into())], ..Default::default() };
        let ack = make_ack_frame(&orig);
        let f = decode_frame(&ack).unwrap();
        assert_eq!(f.method, 1);
        assert_eq!(get_header(&f.headers, "type"), "event");
        let p: Value = serde_json::from_slice(&f.payload).unwrap();
        assert_eq!(p["code"], 200);
    }

    #[tokio::test]
    async fn poll_drains_inbox_and_advances_cursor() {
        let p = FeishuProvider::new("id", "secret", None);
        p.inject("oc_a", "第一句").await;
        p.inject("oc_a", "第二句").await;
        let (msgs, next) = p.poll(0).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(next >= 2);
        // 再 poll：队列空，游标不变
        let (msgs2, next2) = p.poll(next).await.unwrap();
        assert!(msgs2.is_empty());
        assert_eq!(next2, next);
    }

    #[tokio::test]
    async fn poll_skips_messages_at_or_below_cursor() {
        // 模拟崩溃重启后历史残留：游标已到 3，队列里 update_id<=3 的应被丢弃，不重复驱动。
        let p = FeishuProvider::new("id", "secret", None);
        p.inject("oc_a", "旧1").await; // id=1
        p.inject("oc_a", "旧2").await; // id=2
        p.inject("oc_a", "新").await; // id=3
        let (msgs, next) = p.poll(2).await.unwrap();
        assert_eq!(msgs.len(), 1, "只应取 update_id>2 的");
        assert_eq!(msgs[0].text, "新");
        assert!(next >= 3);
    }
}
