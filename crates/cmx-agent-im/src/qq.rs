//! QQ（官方机器人开放平台）IM provider：[`ImProvider`] 的 **WebSocket 长连接** 实现。
//!
//! 与飞书 Stream 同构——agent 主动连 QQ 网关 websocket 拉消息，**无需公网回调 URL**；
//! 且协议纯 JSON 文本帧，无 protobuf、无 ack 帧。范式仍是「出站连接拉取」，桥与内核零改动。
//!
//! 协议（对照官方 v2 文档与 botpy）：
//! - **拿凭证**：`POST https://api.bot.qq.com/app/getAppAccessToken`（JSON `appId`/`clientSecret`）
//!   → `{access_token, expires_in}`（约 7200s；缓存到过期前 60s）。
//! - **拿网关**：`GET {base}/gateway`，头 `Authorization: QQBot {access_token}` → `data.url`（wss）。
//! - **握手**：建连 → 收 op 10 Hello（`d.heartbeat_interval` 毫秒）→ 发 op 2 Identify
//!   （`token = "QQBot {access_token}"`、`intents`、`shard=[0,1]`）→ 收 op 0 READY。
//! - **心跳**：周期发 op 1（`d` = 收到的最新 `s`，首连 null），服务端回 op 11。
//! - **收消息**：op 0 Dispatch（`t`/`s`/`d`），处理 `GROUP_AT_MESSAGE_CREATE`（群聊，须 @机器人）
//!   与 `C2C_MESSAGE_CREATE`（单聊）；其余事件忽略。
//! - **发消息（仅被动回复）**：`POST {base}/v2/groups/{group_openid}/messages` 或
//!   `/v2/users/{user_openid}/messages`，body `{content, msg_type:0, msg_id, msg_seq}`。
//!   **官方限制**：群聊 msg_id 有效期 5 分钟、最多回 5 条；单聊 60 分钟、最多回 4 条——
//!   provider 内按 chat 缓存最近一条入站 `msg_id` 作回复凭证（[`ReplyTicket`]），超窗/超次报错。
//! - **热重载**：[`QqProvider::stop`] 发停止信号 + 自增代际，Stream task 在所有阶段检查
//!   代际过期即退且不重连——防「旧 task 卡在建连窗口、stop 后仍占连接」的乒乓竞态
//!   （同飞书，见 `feishu.rs`；QQ 同 app 新旧连接并存轻则互踢、重则事件双投）。
//!
//! 配置（env）：`CMX_AGENT_IM_QQ_APP_ID` 与 `CMX_AGENT_IM_QQ_APP_SECRET`（必需），
//! 以及可选 `CMX_AGENT_IM_QQ_BASE`（默认正式 `https://api.sgroup.qq.com`；沙箱填
//! `https://sandbox.api.sgroup.qq.com`）。
//!
//! 白名单/绑定由桥统一处理，本 provider 不掺和。

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::{Mutex, watch};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async, tungstenite::Message};

use crate::{ImProvider, InboundMsg};

/// ws 读写半边（`connect_async` 的产物 split 后的形态；起别名免泛型噪声）。
type WsStream = SplitStream<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>>;
type WsSink = SplitSink<WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>, Message>;

// ── 可调参数（常量集中，便于审阅）─────────────────────────────────────────
/// QQ 开放平台 API 基址（发消息 /gateway）。沙箱用 `CMX_AGENT_IM_QQ_BASE` 覆盖。
const QQ_API_BASE: &str = "https://api.sgroup.qq.com";
/// access_token 端点（固定 api.bot.qq.com，官方统一鉴权域；不随 API base 走沙箱）。
const QQ_TOKEN_URL: &str = "https://api.bot.qq.com/app/getAppAccessToken";
/// access_token 默认 TTL（官方约 7200s，响应缺 expires_in 时兜底）。
const TOKEN_DEFAULT_TTL: u64 = 7200;
/// 订阅事件：GROUP_AND_C2C_EVENT（群聊 @ 消息 + 单聊消息），官方 intents 位 1<<25。
const INTENTS_GROUP_AND_C2C: i64 = 1 << 25;
/// Hello 缺 heartbeat_interval 时的兜底心跳周期。
const DEFAULT_HEARTBEAT: Duration = Duration::from_secs(30);
/// 断开后重连退避。
const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);
/// 回复文本分块上限（QQ 文本消息 content 官方上限远小于 Telegram，取保守值）。
pub const QQ_MAX_CHUNK: usize = 500;

// 被动回复官方限制：群聊 5 分钟 / 最多 5 条；单聊 60 分钟 / 最多 4 条（窗口留 10% 安全边）。
const GROUP_REPLY_WINDOW: Duration = Duration::from_secs(270);
const GROUP_REPLY_LIMIT: u32 = 5;
const C2C_REPLY_WINDOW: Duration = Duration::from_secs(3300);
const C2C_REPLY_LIMIT: u32 = 4;

/// 读 env，trim 后非空才返回（None 表未配置）。
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|v| {
        let v = v.trim().to_string();
        (!v.is_empty()).then_some(v)
    })
}

/// 会话形态：决定发消息走 `/v2/groups` 还是 `/v2/users`（两种 openid 外观相同，无法从字面区分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatKind {
    Group,
    C2C,
}

/// 被动回复凭证：最近一条入站消息的 `msg_id` + 已用回复次数 + 收到时刻。
/// QQ 官方机器人不支持主动发消息——所有回复必须挂在一条入站 msg_id 上。
#[derive(Debug, Clone)]
struct ReplyTicket {
    msg_id: String,
    /// 已用回复次数（= 下一条回复的 msg_seq - 1）。
    used: u32,
    at: Instant,
    kind: ChatKind,
}

/// 被动回复门禁（纯函数，可单测）：超窗 / 超次返回人类可读原因。
fn reply_gate(elapsed: Duration, used: u32, kind: ChatKind) -> Result<(), String> {
    let (window, limit) = match kind {
        ChatKind::Group => (GROUP_REPLY_WINDOW, GROUP_REPLY_LIMIT),
        ChatKind::C2C => (C2C_REPLY_WINDOW, C2C_REPLY_LIMIT),
    };
    if elapsed > window {
        return Err(match kind {
            ChatKind::Group => "已超出被动回复有效期（QQ 群聊收消息后 5 分钟内可回复）".into(),
            ChatKind::C2C => "已超出被动回复有效期（QQ 单聊收消息后 60 分钟内可回复）".into(),
        });
    }
    if used >= limit {
        return Err(match kind {
            ChatKind::Group => "该消息的回复次数已用完（QQ 群聊每条消息最多回 5 条）".into(),
            ChatKind::C2C => "该消息的回复次数已用完（QQ 单聊每条消息最多回 4 条）".into(),
        });
    }
    Ok(())
}

/// QQ provider。常驻后台 task 持有 websocket；`poll` 从内部队列取已收消息。
///
/// 内部状态全 `Arc`，`start()` 克隆自身句柄入后台 task（与飞书同款，避免状态分叉）。
pub struct QqProvider {
    inner: Arc<QqInner>,
}

/// QQ provider 共享状态（Arc 包裹，后台 task 与 poll/send 共用）。
struct QqInner {
    app_id: String,
    app_secret: String,
    base: String,
    client: reqwest::Client,
    /// Stream task → poll 的入站队列。
    inbox: Mutex<VecDeque<InboundMsg>>,
    /// 自增 update_id（桥按 drain 推进游标，与飞书同款）。
    seq: AtomicI64,
    /// access_token 缓存：(token, 过期时刻)。
    token: Mutex<Option<(String, Instant)>>,
    /// chat_id → 会话形态（首条入站消息登记）。
    chats: Mutex<HashMap<String, ChatKind>>,
    /// chat_id → 最近回复凭证（每条新入站消息覆盖）。
    replies: Mutex<HashMap<String, ReplyTicket>>,
    /// 已启动 Stream task 的去重标记（`start()` 仅首次 spawn）。
    started: AtomicBool,
    /// 热重载停止信号：`stop()` 置 true，Stream 循环 `select!` 收到即退出。
    stop_tx: watch::Sender<bool>,
    stop_rx: watch::Receiver<bool>,
    /// Stream 代际：`stop()` 自增。task 任何阶段发现领的代际已过期 → 立即退出且不重连。
    generation: AtomicI64,
}

impl QqProvider {
    pub fn new(app_id: impl Into<String>, app_secret: impl Into<String>, base: Option<String>) -> Self {
        let (stop_tx, stop_rx) = watch::channel(false);
        Self {
            inner: Arc::new(QqInner {
                app_id: app_id.into(),
                app_secret: app_secret.into(),
                base: base.unwrap_or_else(|| QQ_API_BASE.into()),
                client: reqwest::Client::builder()
                    .timeout(Duration::from_secs(30))
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new()),
                inbox: Mutex::new(VecDeque::new()),
                seq: AtomicI64::new(0),
                token: Mutex::new(None),
                chats: Mutex::new(HashMap::new()),
                replies: Mutex::new(HashMap::new()),
                started: AtomicBool::new(false),
                stop_tx,
                stop_rx,
                generation: AtomicI64::new(0),
            }),
        }
    }

    /// 从 env 读取（`CMX_AGENT_IM_QQ_APP_ID` + `_APP_SECRET` 必需，`_BASE` 可选）。
    pub fn from_env() -> Option<Self> {
        let app_id = env_nonempty("CMX_AGENT_IM_QQ_APP_ID")?;
        let app_secret = env_nonempty("CMX_AGENT_IM_QQ_APP_SECRET")?;
        Some(Self::new(app_id, app_secret, env_nonempty("CMX_AGENT_IM_QQ_BASE")))
    }

    /// 测试注入：往入站队列塞一条消息（生产路径由 Stream task 调用）。
    pub async fn inject(&self, chat_id: &str, text: &str) {
        self.inject_with_sender(chat_id, text, "").await;
    }

    /// 测试注入（带 sender）：模拟某 openid 给机器人发消息，驱动桥层测试（与飞书同款）。
    /// 生成合成 msg_id（登记回复凭证，桥层回复走 send → 网络 POST 失败被桥忽略，与飞书测试同口径）。
    pub async fn inject_with_sender(&self, chat_id: &str, text: &str, sender: &str) {
        let next = self.inner.seq.load(Ordering::SeqCst) + 1; // enqueue 将分配的 id（测试单线程确定）
        let msg_id = format!("__test_msg_{next}");
        self.enqueue(chat_id, text, sender, msg_id, ChatKind::C2C).await;
    }

    /// 入队一条消息并登记会话形态 / 回复凭证。
    async fn enqueue(
        &self,
        chat_id: &str,
        text: &str,
        sender: &str,
        msg_id: String,
        kind: ChatKind,
    ) {
        let id = self.inner.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner.chats.lock().await.insert(chat_id.into(), kind);
        if !msg_id.is_empty() {
            // 新消息覆盖旧凭证：一条入站消息就是一个新的回复预算（官方按 msg_id 计次）。
            self.inner.replies.lock().await.insert(
                chat_id.into(),
                ReplyTicket { msg_id, used: 0, at: Instant::now(), kind },
            );
        }
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

    /// 换取/刷新 access_token（缓存到过期前 60s）。`force`=忽略缓存重取（401 自愈用）。
    async fn access_token(&self, force: bool) -> Result<String, String> {
        {
            let t = self.inner.token.lock().await;
            if !force
                && let Some((tok, exp)) = t.as_ref()
                && *exp > Instant::now() + Duration::from_secs(60)
            {
                return Ok(tok.clone());
            }
        }
        let resp = self
            .inner
            .client
            .post(QQ_TOKEN_URL)
            .json(&json!({ "appId": self.inner.app_id, "clientSecret": self.inner.app_secret }))
            .send()
            .await
            .map_err(|e| format!("getAppAccessToken 请求失败：{e}"))?;
        let v: Value = resp.json().await.map_err(|e| format!("getAppAccessToken 解析失败：{e}"))?;
        let tok = v
            .get("access_token")
            .and_then(|x| x.as_str())
            .ok_or_else(|| format!("getAppAccessToken 缺 access_token：{v}"))?
            .to_string();
        // expires_in 官方为数字，历史接口曾回字符串——两种都容。
        let expire = v
            .get("expires_in")
            .and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
            .unwrap_or(TOKEN_DEFAULT_TTL);
        *self.inner.token.lock().await = Some((tok.clone(), Instant::now() + Duration::from_secs(expire)));
        Ok(tok)
    }

    /// 拉取 websocket 网关地址（`GET /gateway`，`Authorization: QQBot {token}`）。
    async fn fetch_gateway(&self) -> Result<String, String> {
        let token = self.access_token(false).await?;
        let resp = self
            .inner
            .client
            .get(format!("{}/gateway", self.inner.base.trim_end_matches('/')))
            .header("Authorization", format!("QQBot {token}"))
            .send()
            .await
            .map_err(|e| format!("gateway 请求失败：{e}"))?;
        let v: Value = resp.json().await.map_err(|e| format!("gateway 解析失败：{e}"))?;
        let url = v
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or_else(|| format!("gateway 响应缺 url：{v}"))?
            .to_string();
        Ok(url)
    }

    /// Stream 主循环：建连 → 握手 → 收帧/心跳；出错由外层重连。
    /// 热重载：stop 置位或代际过期 → 返回 `Err(STOPPED)`（外层据此退出不重连）。
    /// gateway/建连两个不响应 select 的 HTTP/TLS 阶段结束后也检查代际（与飞书同款）。
    async fn run_stream_once(&self, my_gen: i64) -> Result<(), String> {
        let url = self.fetch_gateway().await?;
        if self.generation_stale(my_gen) {
            return Err(STOPPED.into());
        }
        let (ws, _resp) = connect_async(&url).await.map_err(|e| format!("ws 建连失败：{e}"))?;
        if self.generation_stale(my_gen) {
            // 建连成功但代际已过期：立刻关闭，绝不占用连接。
            drop(ws);
            tracing::info!("qq stream 建连后发现代际过期，立即退出（热重载）");
            return Err(STOPPED.into());
        }
        let (mut sink, mut stream) = ws.split();
        let period = self.handshake(&mut sink, &mut stream, my_gen).await?;
        tracing::info!("qq stream 已连接（intents=GROUP_AND_C2C）");

        // ── 收发主循环：心跳 + 事件分发（对齐飞书 select 结构）──
        let mut beat = tokio::time::interval(period);
        beat.tick().await; // 跳过首次立即触发
        let mut last_s: Option<i64> = None; // 心跳须带最新 s（官方语义）
        let mut stop = self.inner.stop_rx.clone();
        loop {
            if self.generation_stale(my_gen) {
                tracing::info!("qq stream 代际过期，断开连接（热重载）");
                return Err(STOPPED.into());
            }
            tokio::select! {
                _ = stop.changed(), if *stop.borrow() || stop.has_changed().unwrap_or(false) => {
                    if *stop.borrow() {
                        tracing::info!("qq stream 收到停止信号，断开连接");
                        return Err(STOPPED.into());
                    }
                }
                msg = stream.next() => match msg {
                    Some(Ok(Message::Text(s))) => {
                        self.handle_frame(&s, &mut last_s).await?;
                    }
                    Some(Ok(Message::Binary(b))) => {
                        let text = String::from_utf8(b)
                            .map_err(|e| format!("ws 帧非 UTF-8：{e}"))?;
                        self.handle_frame(&text, &mut last_s).await?;
                    }
                    Some(Ok(Message::Ping(p))) => { let _ = sink.send(Message::Pong(p)).await; }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return Err(format!("ws 读失败：{e}")),
                    None => return Err("ws 连接关闭".into()),
                },
                _ = beat.tick() => {
                    let _ = sink.send(Message::Text(json!({ "op": 1, "d": last_s }).to_string())).await;
                }
            }
        }
    }

    /// 握手：收 op 10 Hello（取心跳周期）→ 发 op 2 Identify → 等 op 0 READY。
    /// 顺序 await（无 select），返回心跳周期。op 9 = 凭证/订阅错，交外层退避重试。
    async fn handshake(
        &self,
        sink: &mut WsSink,
        stream: &mut WsStream,
        my_gen: i64,
    ) -> Result<Duration, String> {
        let hello = next_text(stream).await?;
        let v: Value = serde_json::from_str(&hello).map_err(|e| format!("Hello 解析失败：{e}"))?;
        if v.get("op").and_then(|o| o.as_i64()) != Some(10) {
            return Err(format!("首帧非 Hello(op10)：{v}"));
        }
        let period = v
            .pointer("/d/heartbeat_interval")
            .and_then(|x| x.as_u64())
            .map(|ms| Duration::from_millis(ms.max(1000)))
            .unwrap_or(DEFAULT_HEARTBEAT);
        let token = self.access_token(false).await?;
        let identify = json!({
            "op": 2,
            "d": {
                "token": format!("QQBot {token}"),
                "intents": INTENTS_GROUP_AND_C2C,
                "shard": [0, 1],
            }
        });
        sink.send(Message::Text(identify.to_string()))
            .await
            .map_err(|e| format!("发送 Identify 失败：{e}"))?;
        loop {
            if self.generation_stale(my_gen) {
                return Err(STOPPED.into());
            }
            let text = next_text(stream).await?;
            let Some(v) = parse_json(&text) else { continue };
            match v.get("op").and_then(|o| o.as_i64()) {
                Some(0) if v.get("t").and_then(|t| t.as_str()) == Some("READY") => {
                    return Ok(period);
                }
                // op 9 Invalid Session：identify 参数错（多为凭证失效）。
                Some(9) => return Err("Invalid Session（op9，检查凭证/事件订阅）".into()),
                _ => {} // READY 前的其它帧忽略
            }
        }
    }

    /// 处理一帧已收到的事件文本（主循环逐帧调用）。
    /// op 0 消息事件 → 入队；op 7/9 → 断线交外层重连；其余忽略。
    async fn handle_frame(&self, text: &str, last_s: &mut Option<i64>) -> Result<(), String> {
        let Some(v) = parse_json(text) else { return Ok(()) };
        if let Some(s) = v.get("s").and_then(|x| x.as_i64()) {
            *last_s = Some(s);
        }
        match v.get("op").and_then(|o| o.as_i64()) {
            // 心跳 ACK / 服务端重复 Hello：忽略
            Some(11) | Some(10) => Ok(()),
            // 服务端要求重连 / 会话失效：按错误走外层重连（fresh Identify）
            Some(7) => Err("服务端要求重连(op7)".into()),
            Some(9) => Err("Invalid Session（op9，检查凭证/事件订阅）".into()),
            Some(0) => {
                let event_type = v.get("t").and_then(|t| t.as_str()).unwrap_or("");
                if matches!(event_type, "GROUP_AT_MESSAGE_CREATE" | "C2C_MESSAGE_CREATE")
                    && let Some(m) = parse_qq_event(text)
                {
                    self.enqueue(&m.chat_id, &m.text, &m.sender, m.msg_id, m.kind).await;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// 重连外壳：失败/断开 → 退避重试，永不放弃（对齐飞书与桥 `run()` 的退避语义）。
    /// 热重载：stop 置位 → 退出（含退避 sleep 期间被通知也立即走）。
    async fn run_stream_loop(self: Arc<Self>) {
        let mut stop = self.inner.stop_rx.clone();
        let my_gen = self.inner.generation.load(Ordering::SeqCst); // 领取代际
        loop {
            tokio::select! {
                _ = stop.changed() => {
                    if *stop.borrow() {
                        tracing::info!("qq stream task 退出（热重载）");
                        return;
                    }
                }
                r = self.run_stream_once(my_gen) => {
                    if matches!(r, Err(ref e) if e == STOPPED) {
                        tracing::info!("qq stream task 退出（热重载）");
                        return;
                    }
                    if let Err(e) = r {
                        tracing::warn!("qq stream 断开：{e}，{}s 后重连", RECONNECT_BACKOFF.as_secs());
                    }
                    // 退避期间也响应 stop（select 包住 sleep）。
                    tokio::select! {
                        _ = stop.changed() => { if *stop.borrow() { return; } }
                        _ = tokio::time::sleep(RECONNECT_BACKOFF) => {}
                    }
                }
            }
        }
    }

    /// 热重载：发停止信号并自增代际——所有 Stream task（含正卡在建连中的）在下一个
    /// 检查点退出且不再重连，QQ 连接让位给新桥。
    pub fn stop(&self) {
        self.inner.generation.fetch_add(1, Ordering::SeqCst);
        let _ = self.inner.stop_tx.send(true);
    }

    /// 代际是否已过期（本 task 领的号不再是最新）。
    fn generation_stale(&self, my_gen: i64) -> bool {
        self.inner.generation.load(Ordering::SeqCst) != my_gen
    }
}

/// `run_stream_once` 返回该串 = 因 stop/代际过期退出（外层不再重连，正常退出 task）。
const STOPPED: &str = "__stopped__";

/// 收下一帧数据并转文本（QQ 协议为 JSON 文本帧；容错 Binary）。只借 stream——
/// 与主循环 select 的 sink 使用不冲突。ws 层 Ping/Pong 由 tungstenite 自动排队应答。
async fn next_text(stream: &mut WsStream) -> Result<String, String> {
    loop {
        match stream.next().await {
            Some(Ok(Message::Text(s))) => return Ok(s),
            Some(Ok(Message::Binary(b))) => {
                return String::from_utf8(b).map_err(|e| format!("ws 帧非 UTF-8：{e}"));
            }
            Some(Ok(_)) => {}
            Some(Err(e)) => return Err(format!("ws 读失败：{e}")),
            None => return Err("ws 连接关闭".into()),
        }
    }
}

/// 解析 JSON 文本（解析失败返回 None——非 JSON 帧不致命，忽略）。
fn parse_json(text: &str) -> Option<Value> {
    serde_json::from_str(text).ok()
}

impl QqInner {
    /// CAS 守门：仅首次调用返回 true（已启动则 false，避免重复 spawn）。
    fn start_or_reject(&self) -> bool {
        self.started
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }
}

#[async_trait]
impl ImProvider for QqProvider {
    async fn poll(&self, offset: i64) -> Result<(Vec<InboundMsg>, i64), String> {
        let mut q = self.inner.inbox.lock().await;
        // 只取 update_id > offset 的（与飞书同款游标排水）。
        let mut out = Vec::new();
        let mut next = offset;
        while let Some(m) = q.front() {
            if m.update_id <= offset {
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
        let ticket = {
            let map = self.inner.replies.lock().await;
            map.get(chat_id).cloned()
        };
        let Some(t) = ticket else {
            // 没有凭证 = 该会话还没给机器人发过消息（QQ 官方机器人不支持主动发消息）。
            return Err("QQ 官方机器人只能被动回复：请先在 QQ 里给机器人发一条消息".into());
        };
        reply_gate(t.at.elapsed(), t.used, t.kind)?;
        let path = match t.kind {
            ChatKind::Group => format!("/v2/groups/{chat_id}/messages"),
            ChatKind::C2C => format!("/v2/users/{chat_id}/messages"),
        };
        let body = build_send_body(text, &t.msg_id, t.used + 1);
        // 401 → 强制刷新 token 重试一次（access_token 过期自愈）。
        let token = self.access_token(false).await?;
        let resp = self
            .inner
            .client
            .post(format!("{}{path}", self.inner.base.trim_end_matches('/')))
            .header("Authorization", format!("QQBot {token}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("sendMessage 请求失败：{e}"))?;
        let resp = if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            let fresh = self.access_token(true).await?;
            self.inner
                .client
                .post(format!("{}{path}", self.inner.base.trim_end_matches('/')))
                .header("Authorization", format!("QQBot {fresh}"))
                .json(&body)
                .send()
                .await
                .map_err(|e| format!("sendMessage 重试失败：{e}"))?
        } else {
            resp
        };
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("sendMessage HTTP {status}: {body}"));
        }
        // 发送成功才推进计数（失败不消耗 msg_seq 预算）。
        if let Some(t) = self.inner.replies.lock().await.get_mut(chat_id) {
            t.used += 1;
        }
        Ok(())
    }

    /// 启动常驻 Stream 后台 task。幂等：重复调用只 spawn 一次（`started` CAS 守门）。
    async fn start(&self) -> Result<(), String> {
        if self.inner.start_or_reject() {
            let me = Arc::clone(&self.inner);
            tokio::spawn(async move {
                let p = QqProvider { inner: me };
                Arc::new(p).run_stream_loop().await;
            });
        }
        Ok(())
    }

    /// 文本消息分块上限（QQ 官方 content 上限远小于 Telegram 4096）。
    fn max_chunk(&self) -> usize {
        QQ_MAX_CHUNK
    }

    /// 热重载：trait 入口（壳侧经 `dyn ImProvider` 调用）——委托固有实现发自增代际 + 停止信号。
    fn stop(&self) {
        QqProvider::stop(self);
    }
}

// ── 纯函数：消息解析 / 构造（可单测，无网络）──────────────────────────────

/// 从 op 0 Dispatch payload 解析一条文本消息。
/// - `GROUP_AT_MESSAGE_CREATE`：chat_id = `d.group_openid`，sender = `d.author.id`
///   （兜底 `author.member_openid`），群聊须 @ 机器人（content 剥 @ 段）。
/// - `C2C_MESSAGE_CREATE`：chat_id = sender = 用户 openid（`d.author.user_openid` 兜底
///   `author.id` / `d.user_openid`）。
///
/// 仅文本、非空；其余事件返回 None。字段名按官方文档，取值全程防御式（缺失按空兜底）。
pub fn parse_qq_event(payload: &str) -> Option<QqInbound> {
    let v: Value = serde_json::from_str(payload).ok()?;
    if v.get("op").and_then(|o| o.as_i64()) != Some(0) {
        return None;
    }
    let event_type = v.get("t").and_then(|t| t.as_str())?;
    let d = v.get("d")?;
    let (kind, chat_id, sender) = match event_type {
        "GROUP_AT_MESSAGE_CREATE" => {
            let chat_id = d.get("group_openid").and_then(|c| c.as_str())?.to_string();
            let sender = d
                .pointer("/author/id")
                .or_else(|| d.pointer("/author/member_openid"))
                .or_else(|| d.pointer("/member_openid"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            (ChatKind::Group, chat_id, sender)
        }
        "C2C_MESSAGE_CREATE" => {
            let sender = d
                .pointer("/author/user_openid")
                .or_else(|| d.pointer("/author/id"))
                .or_else(|| d.get("user_openid"))
                .and_then(|s| s.as_str())
                .unwrap_or("")
                .to_string();
            (ChatKind::C2C, sender.clone(), sender)
        }
        _ => return None, // 其余事件（READY/RESUMED/FRIEND_ADD…）不驱动回合
    };
    let raw = d.get("content").and_then(|c| c.as_str())?;
    let text = strip_at_prefix(raw);
    if text.is_empty() {
        return None;
    }
    let msg_id = d.get("id").and_then(|i| i.as_str()).unwrap_or("").to_string();
    Some(QqInbound { chat_id, sender, msg_id, text, kind })
}

/// 一条解析后的入站消息（`parse_qq_event` 产物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QqInbound {
    pub chat_id: String,
    pub sender: String,
    /// 消息 id（被动回复凭证；缺失为空串——无凭证时只能收不能回）。
    pub msg_id: String,
    pub text: String,
    pub kind: ChatKind,
}

/// 剥 @ 前缀（纯函数）：去首尾空白；`<@…>` 段整体剥掉；`@xxx` 形式剥首个空白分隔词。
/// 群聊须 @ 机器人，content 可能带 mention 前缀——剥净后才是用户正文。
pub fn strip_at_prefix(content: &str) -> String {
    let mut s = content.trim();
    while let Some(rest) = s.strip_prefix("<@") {
        match rest.find('>') {
            Some(i) => s = rest[i + 1..].trim_start(),
            None => break,
        }
    }
    if s.starts_with('@') {
        if let Some(sp) = s.find(char::is_whitespace) {
            s = s[sp..].trim_start();
        } else {
            s = "";
        }
    }
    s.to_string()
}

/// 构造发消息 body（`msg_type=0` 文本 + 被动回复凭证）。`msg_seq` 从 1 起单调递增。
pub fn build_send_body(text: &str, msg_id: &str, msg_seq: u32) -> Value {
    json!({
        "content": text,
        "msg_type": 0,
        "msg_id": msg_id,
        "msg_seq": msg_seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GROUP_EVENT: &str = r#"{"id":"evt1","op":0,"s":42,"t":"GROUP_AT_MESSAGE_CREATE","d":{"author":{"id":"mem_openid","member_openid":"mem_openid"},"content":"<@!BOT> 帮我查订单","group_openid":"grp_openid","id":"msgid_1","timestamp":"2026-09-09"}}"#;
    const C2C_EVENT: &str = r#"{"op":0,"s":43,"t":"C2C_MESSAGE_CREATE","d":{"author":{"id":"usr_openid","user_openid":"usr_openid"},"content":"你好","id":"msgid_2","timestamp":"2026-09-09"}}"#;

    #[test]
    fn parse_group_event() {
        let m = parse_qq_event(GROUP_EVENT).expect("应解析出群消息");
        assert_eq!(m.chat_id, "grp_openid");
        assert_eq!(m.sender, "mem_openid");
        assert_eq!(m.msg_id, "msgid_1");
        assert_eq!(m.text, "帮我查订单");
        assert_eq!(m.kind, ChatKind::Group);
    }

    #[test]
    fn parse_c2c_event() {
        let m = parse_qq_event(C2C_EVENT).expect("应解析出单聊消息");
        assert_eq!(m.chat_id, "usr_openid");
        assert_eq!(m.sender, "usr_openid");
        assert_eq!(m.text, "你好");
        assert_eq!(m.kind, ChatKind::C2C);
    }

    #[test]
    fn parse_skips_other_events_and_empty() {
        let ready = r#"{"op":0,"s":1,"t":"READY","d":{"session_id":"x"}}"#;
        assert!(parse_qq_event(ready).is_none());
        let op11 = r#"{"op":11}"#;
        assert!(parse_qq_event(op11).is_none());
        let empty = r#"{"op":0,"s":2,"t":"C2C_MESSAGE_CREATE","d":{"author":{"id":"u"},"content":"   ","id":"m"}}"#;
        assert!(parse_qq_event(empty).is_none());
        assert!(parse_qq_event("not json").is_none());
    }

    #[test]
    fn strip_at_variants() {
        assert_eq!(strip_at_prefix("  你好 "), "你好");
        assert_eq!(strip_at_prefix("<@!ABC>命令"), "命令");
        assert_eq!(strip_at_prefix("@机器人 查一下"), "查一下");
        assert_eq!(strip_at_prefix("@机器人"), "");
        assert_eq!(strip_at_prefix("查一下"), "查一下");
    }

    #[test]
    fn send_body_shape() {
        let b = build_send_body("hi", "msgid_1", 2);
        assert_eq!(b["content"], "hi");
        assert_eq!(b["msg_type"], 0);
        assert_eq!(b["msg_id"], "msgid_1");
        assert_eq!(b["msg_seq"], 2);
    }

    #[test]
    fn reply_gate_windows_and_limits() {
        // 群聊：窗口内 + 未超次 → Ok
        assert!(reply_gate(Duration::from_secs(60), 0, ChatKind::Group).is_ok());
        // 群聊：超窗
        assert!(reply_gate(GROUP_REPLY_WINDOW + Duration::from_secs(1), 0, ChatKind::Group).is_err());
        // 群聊：超次（5 条上限）
        assert!(reply_gate(Duration::from_secs(60), GROUP_REPLY_LIMIT, ChatKind::Group).is_err());
        assert!(reply_gate(Duration::from_secs(60), GROUP_REPLY_LIMIT - 1, ChatKind::Group).is_ok());
        // 单聊：60 分钟窗（留安全边 3300s）/ 4 条上限
        assert!(reply_gate(Duration::from_secs(3000), 0, ChatKind::C2C).is_ok());
        assert!(reply_gate(C2C_REPLY_WINDOW + Duration::from_secs(1), 0, ChatKind::C2C).is_err());
        assert!(reply_gate(Duration::from_secs(60), C2C_REPLY_LIMIT, ChatKind::C2C).is_err());
    }

    #[tokio::test]
    async fn poll_drains_inbox_and_advances_cursor() {
        let p = QqProvider::new("id", "secret", None);
        p.inject("u1", "第一句").await;
        p.inject("u1", "第二句").await;
        let (msgs, next) = p.poll(0).await.unwrap();
        assert_eq!(msgs.len(), 2);
        assert!(next >= 2);
        let (msgs2, next2) = p.poll(next).await.unwrap();
        assert!(msgs2.is_empty());
        assert_eq!(next2, next);
    }

    #[tokio::test]
    async fn poll_skips_messages_at_or_below_cursor() {
        let p = QqProvider::new("id", "secret", None);
        p.inject("u1", "旧1").await; // id=1
        p.inject("u1", "旧2").await; // id=2
        p.inject("u1", "新").await; // id=3
        let (msgs, next) = p.poll(2).await.unwrap();
        assert_eq!(msgs.len(), 1, "只应取 update_id>2 的");
        assert_eq!(msgs[0].text, "新");
        assert!(next >= 3);
    }

    #[tokio::test]
    async fn inject_registers_reply_ticket_and_chat_kind() {
        let p = QqProvider::new("id", "secret", None);
        p.inject_with_sender("grp1", "hi", "u1").await;
        assert_eq!(p.inner.chats.lock().await.get("grp1"), Some(&ChatKind::C2C));
        let t = p.inner.replies.lock().await.get("grp1").cloned();
        assert!(t.is_some(), "入站消息应登记被动回复凭证");
    }

    #[tokio::test]
    async fn stop_bumps_generation_and_signals() {
        let p = QqProvider::new("id", "secret", None);
        assert_eq!(p.inner.generation.load(Ordering::SeqCst), 0);
        assert!(!*p.inner.stop_rx.borrow());
        p.stop();
        assert_eq!(p.inner.generation.load(Ordering::SeqCst), 1);
        assert!(*p.inner.stop_rx.borrow());
        assert!(p.generation_stale(0));
        assert!(!p.generation_stale(1));
    }
}
