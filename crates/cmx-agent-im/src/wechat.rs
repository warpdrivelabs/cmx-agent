//! 微信 ClawBot（iLink 协议）IM provider：[`ImProvider`] 的 **HTTP 长轮询** 实现。
//!
//! 与 Telegram 同形态——`getupdates` 服务端挂起 ≤35s 的出站长轮询，**无需公网回调、
//! 无 WebSocket、无 protobuf**。ClawBot 是微信团队官方的 OpenClaw 微信插件，底层
//! iLink 协议（`ilinkai.weixin.qq.com`），合法零封号；本文件自实现 bot 侧协议，
//! 不依赖 OpenClaw/Node（协议对照 npm `@tencent-weixin/openclaw-weixin` v2.1.1，
//! 社区整理文档 github.com/nightsailer/wechat-clawbot `docs/ilink-protocol.md`）。
//!
//! 协议要点：
//! - **登录**：`GET /ilink/bot/get_bot_qrcode?bot_type=3` → data-url PNG 二维码；
//!   轮询 `GET /ilink/bot/get_qrcode_status?qrcode=…`（`scaned_but_redirect` 切
//!   `redirect_host`；`expired` 自动重取 ≤3 次）→ `confirmed` 得 `bot_token` /
//!   `ilink_bot_id` / `baseurl`（此后基址以 `baseurl` 为准，IDC 调度）。
//!   登录入口 = CLI 子命令 `cmx-agent im-login`，凭证落盘 im.json 的 `wechat` 字段。
//! - **收消息**：`POST /ilink/bot/getupdates`，body `{"get_updates_buf": 游标}`——
//!   字符串游标由服务端下发、客户端必须原样回传。i64 游标装不下，本 provider 把它
//!   收在内部（[`WechatInner::cursor`]），不占用 [`ImProvider::poll`] 的 offset 参数。
//! - **发消息**：`POST /ilink/bot/sendmessage`，`message_type=2` / `message_state=2`
//!   文本 item；回传入站带来的 `context_token` 以精确关联会话窗口。
//! - **鉴权头**：`Authorization: Bearer {bot_token}` + `AuthorizationType: ilink_bot_token`
//!   + `X-WECHAT-UIN`（随机 u32 十进制串再 base64，防重放）。
//! - **错误**：响应判定对齐官方 v2.4.8（[`classify_response`]）——`ret`/`errcode`
//!   **存在且 ≠0** 才是错误，缺失/null 均为成功（长轮询空转响应无 `ret` 字段）；
//!   -14 = token 失效（官方 2.4.6 起 STALE_TOKEN 语义）→ 暂停 1h；HTTP 401 → token 失效，
//!   暂停 60s 并提示重新扫码。两者都收在 provider 内部（暂停期 [`ImProvider::poll`]
//!   返回空轮），**不透传给桥**——桥 `run()` 只有 3s 退避，会把 -14 撞成高频重试。
//!
//! v1 边界：仅单聊文本（群聊/语音/图片等媒体消息跳过并打日志）；[`WECHAT_MAX_CHUNK`]
//! = 2000（官方单条上限未披露，保守值）；游标不持久化（重启后首轮排水历史残留，
//! 与飞书/QQ「跳过残留」同口径）。
//!
//! 配置（env）：`CMX_AGENT_IM_WECHAT_BOT_TOKEN`（必需）+ 可选
//! `CMX_AGENT_IM_WECHAT_BASE`（默认 `https://ilinkai.weixin.qq.com`）。
//! 白名单/绑定由桥统一处理，本 provider 不掺和。

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::{ImProvider, InboundMsg};

// ── 可调参数（常量集中，便于审阅）─────────────────────────────────────────
/// iLink API 基址（登录 confirmed 响应的 `baseurl` 会覆盖，IDC 调度）。
pub(crate) const WECHAT_BASE_DEFAULT: &str = "https://ilinkai.weixin.qq.com";
/// iLink channel_version（对齐官方 `@tencent-weixin/openclaw-weixin` 包版本；协议升级时
/// 改这里跟随——2026-09 真机核对为 2.4.8，此前照社区文档抄的 2.1.1 已过旧）。
pub(crate) const ILINK_CHANNEL_VERSION: &str = "2.4.8";
/// 回复文本分块上限（官方上限未披露，取 Telegram 4096 的一半留安全余量）。
const WECHAT_MAX_CHUNK: usize = 2000;
/// errcode=-14 会话超时的暂停时长（协议文档：暂停 1 小时后恢复）。
const SESSION_TIMEOUT_PAUSE: Duration = Duration::from_secs(3600);
/// 401（token 失效）后的重试暂停：期间 poll 返回空轮，避免高频撞 401 刷日志。
const RELOGIN_PAUSE: Duration = Duration::from_secs(60);
/// 长轮询连续失败退避：1-2 次等 2s，≥3 次等 30s（协议文档建议值）。
const FAIL_BACKOFF_SHORT: Duration = Duration::from_secs(2);
const FAIL_BACKOFF_LONG: Duration = Duration::from_secs(30);
/// `getupdates` 服务端挂起 ≤35s → 客户端超时须大于它（45s），其余调用 15s。
const LONGPOLL_TIMEOUT: Duration = Duration::from_secs(45);
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// message_id 去重集合上限（超限整体清空——游标正常推进时本不该重复，纯兜底）。
const SEEN_CAP: usize = 4096;
/// context_token 缓存 TTL（参考协议文档 typing_ticket 的 24h 建议值）。
const CONTEXT_TTL: Duration = Duration::from_secs(24 * 3600);
/// 扫码登录总时长上限 / 状态轮询间隔 / 二维码过期自动刷新次数上限。
const LOGIN_MAX_WAIT: Duration = Duration::from_secs(600);
const LOGIN_POLL_INTERVAL: Duration = Duration::from_secs(2);
const QR_MAX_REFRESH: u32 = 3;
/// 扫码状态查询**连续**失败容忍次数（实测腾讯侧偶发瞬断 "error sending request"——
/// 一次失败就终态会把正常登录打死；连续超限才报错，期间自动重试）。
const LOGIN_POLL_MAX_FAILS: u32 = 5;

/// 读 env，trim 后非空才返回（None 表未配置）。
fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|v| {
        let v = v.trim().to_string();
        (!v.is_empty()).then_some(v)
    })
}

/// `iLink-App-ClientVersion` 头：版本串按 `0x00MMNNPP` 编码（如 0.3.0 → 768）。
/// 解析失败各段兜底 0（协议只要求 uint32 形态）。
pub(crate) fn encode_client_version_str(v: &str) -> u32 {
    let mut it = v.split('.');
    let num = |s: Option<&str>| s.and_then(|x| x.parse::<u32>().ok()).unwrap_or(0);
    let (maj, min, pat) = (num(it.next()), num(it.next()), num(it.next()));
    (maj.min(0xFF) << 16) | (min.min(0xFF) << 8) | pat.min(0xFF)
}

/// 当前 ClientVersion 编码（进程内只算一次）。**对齐官方实现**：用 [`ILINK_CHANNEL_VERSION`]
/// （如 2.1.1 → 0x00020101 = 131329），不用 crate 版本（0.1.0 → 256 会被 iLink 核心端点
/// 按版本下限拒掉——getupdates 挑版本比扫码端点严格）。
fn client_version_u32() -> u32 {
    static V: OnceLock<u32> = OnceLock::new();
    *V.get_or_init(|| encode_client_version_str(ILINK_CHANNEL_VERSION))
}

/// 随机 u32（getrandom，缓存外依赖零新增——它已在依赖树/离线缓存中）。
/// 失败兜底时间熵：该随机数只用于 X-WECHAT-UIN 防重放头，非安全边界。
pub(crate) fn random_u32() -> u32 {
    let mut b = [0u8; 4];
    if getrandom::getrandom(&mut b).is_ok() {
        u32::from_le_bytes(b)
    } else {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0)
    }
}

/// `X-WECHAT-UIN` 头值：随机 u32 → 十进制字符串 → 对该字符串 UTF-8 字节做 base64。
pub(crate) fn random_uin() -> String {
    STANDARD.encode(random_u32().to_string())
}

/// 公共头（所有请求）：`iLink-App-Id: bot` + ClientVersion。
pub(crate) fn apply_common(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    rb.header("iLink-App-Id", "bot")
        .header("iLink-App-ClientVersion", client_version_u32())
}

/// POST 额外头：Bearer 鉴权 + AuthorizationType + X-WECHAT-UIN（每次请求新随机）。
/// token 兼容两种形态：已带 `Bearer ` 前缀直接用，否则补前缀（登录响应形态未完全固定）。
pub(crate) fn apply_auth(rb: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    let bearer = if token.starts_with("Bearer ") {
        token.to_string()
    } else {
        format!("Bearer {token}")
    };
    rb.header("AuthorizationType", "ilink_bot_token")
        .header("Authorization", bearer)
        .header("X-WECHAT-UIN", random_uin())
}

/// POST 请求体公共段 `base_info`。对齐官方 v2.4.8（`buildBaseInfo`）：`channel_version` =
/// 包版本；2.3.1 起新增 `bot_agent`（UA 风格串，官方对缺失/非法值回落 `"OpenClaw"`——照抄）。
pub(crate) fn base_info() -> Value {
    json!({ "channel_version": ILINK_CHANNEL_VERSION, "bot_agent": "OpenClaw" })
}

/// iLink 响应判定（纯函数，可单测）。**语义对齐官方 v2.4.8 `monitor.js`**：
/// `isApiError = (ret !== undefined && ret !== 0) || (errcode !== undefined && errcode !== 0)`
/// —— `ret`/`errcode` **存在且 ≠0** 才是错误，缺失/null 均为成功（长轮询空转的正常响应
/// 没有 `ret` 字段；此前把缺失兜底成 -1 当失败，真机误报 `getupdates ret=-1 msg=`）。
/// `ret` 或 `errcode` 命中 -14（2.4.6 起语义 = **token 失效**，STALE_TOKEN）单列，调用方暂停。
pub(crate) fn classify_response(v: &Value) -> ResponseVerdict {
    let ret = v.get("ret").and_then(|r| r.as_i64());
    let errcode = v.get("errcode").and_then(|c| c.as_i64());
    if ret == Some(-14) || errcode == Some(-14) {
        return ResponseVerdict::StaleToken;
    }
    // 取非零的那一个作为错误码（正常只有一个有值；都在且都非零优先 ret）。
    if let Some(code) = ret.filter(|&r| r != 0).or(errcode.filter(|&c| c != 0)) {
        let msg = v.get("errmsg").and_then(|m| m.as_str()).unwrap_or("").to_string();
        return ResponseVerdict::Failed { code, errmsg: msg };
    }
    ResponseVerdict::Ok
}

/// [`classify_response`] 的判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ResponseVerdict {
    /// 成功（ret/errcode 缺失、null 或 0）。
    Ok,
    /// ret/errcode = -14：token 已失效（官方 2.4.6 起 STALE_TOKEN 语义），调用方暂停。
    StaleToken,
    /// 其它非零码：code + errmsg（可能为空串——真机见过 errmsg 缺失的错误响应）。
    Failed { code: i64, errmsg: String },
}

// ── iLink 消息结构（防御式 serde：缺字段兜底默认，字段出入不炸）─────────────

/// WeixinMessage（getupdates 的 msgs 元素）。字段名按协议文档。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub(crate) struct WeixinMessage {
    seq: i64,
    message_id: i64,
    from_user_id: String,
    to_user_id: String,
    group_id: String,
    /// 0 NONE / 1 USER / 2 BOT——只处理 1（跳过自己发出去的回声）。
    message_type: i64,
    /// 0 NEW / 1 GENERATING / 2 FINISH（用户消息恒 NEW，暂不作门槛）。
    #[allow(dead_code)]
    message_state: i64,
    /// 会话令牌：回复时回传以精确关联会话窗口。
    context_token: String,
    item_list: Vec<MessageItem>,
}

/// MessageItem：v1 只消费 type=1 文本项，其余类型（图片/语音/文件/视频）忽略。
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
struct MessageItem {
    #[serde(rename = "type")]
    item_type: i64,
    text_item: Option<TextItem>,
}

/// TextItem：纯文本内容。
#[derive(Debug, Clone, Default, Deserialize)]
struct TextItem {
    #[serde(default)]
    text: String,
}

/// 一条通过过滤、待入队的入站消息（`convert` 产物）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedMsg {
    chat_id: String,
    sender: String,
    text: String,
    /// 去重 id：优先 message_id，退 seq；两者皆缺 → 0（不做去重）。
    dedup_id: i64,
    /// 桥侧游标透传值（seq，缺则退 dedup_id）。
    seq: i64,
    context_token: String,
}

/// 纯函数：入站过滤 + 字段映射（可单测，无网络）。
/// - 非 USER 消息（BOT 回声/NONE）→ None；
/// - 群聊（group_id 非空）→ None（v1 仅单聊，权限模型未文档化）；
/// - 无文本 item / 文本全空白 / 缺 from_user_id → None；
/// - 多个文本 item 顺序拼接。
fn convert(m: &WeixinMessage) -> Option<ParsedMsg> {
    if m.message_type != 1 {
        return None;
    }
    if !m.group_id.is_empty() {
        tracing::info!("微信群聊消息暂不处理（v1 仅单聊）group_id={}", m.group_id);
        return None;
    }
    let mut text = String::new();
    for item in &m.item_list {
        if item.item_type == 1
            && let Some(t) = &item.text_item
        {
            text.push_str(&t.text);
        }
    }
    if text.trim().is_empty() {
        return None;
    }
    if m.from_user_id.is_empty() {
        return None;
    }
    let dedup_id = if m.message_id != 0 {
        m.message_id
    } else {
        m.seq
    };
    let seq = if m.seq != 0 { m.seq } else { dedup_id };
    Some(ParsedMsg {
        chat_id: m.from_user_id.clone(),
        sender: m.from_user_id.clone(),
        text,
        dedup_id,
        seq,
        context_token: m.context_token.clone(),
    })
}

/// 构造 sendmessage body：`message_type=2`（BOT）/ `message_state=2`（FINISH）文本 item。
/// `context_token` 缺省时不带该字段（iLink v2.1+ 服务端按最近活跃会话兜底）。
pub(crate) fn build_send_body(
    to_user_id: &str,
    text: &str,
    context_token: Option<&str>,
    client_id: &str,
) -> Value {
    let mut msg = json!({
        "to_user_id": to_user_id,
        "client_id": client_id,
        "message_type": 2,
        "message_state": 2,
        "item_list": [ { "type": 1, "text_item": { "text": text } } ],
    });
    if let Some(tok) = context_token {
        msg["context_token"] = json!(tok);
    }
    json!({ "msg": msg, "base_info": base_info() })
}

/// sendmessage 的 client_id：进程内自增序号 + 随机后缀（协议只要求自定义唯一串）。
fn new_client_id() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, Ordering::SeqCst);
    format!("cmx-agent-{n}-{:08x}", random_u32())
}

/// 扫码登录成功的产物（CLI 写入 im.json `wechat` 字段的来源）。
#[derive(Debug, Clone)]
pub struct WechatLogin {
    pub bot_token: String,
    /// 机器人 id（`xxx@im.bot`，展示用）。
    pub bot_id: String,
    /// 登录用户 id（展示用）。
    pub user_id: String,
    /// 登录确认下发的 API 基址（IDC 调度后可能与默认不同）。
    pub base: String,
}

/// 微信 provider。长轮询型（[`ImProvider::start`] 走 trait 默认 no-op）——
/// 收发都在 `poll`/`send` 里同步发生，无常驻 task、无热重载需求。
pub struct WechatProvider {
    inner: Arc<WechatInner>,
}

/// 共享状态（Arc 包裹，登录流程与 poll/send 共用）。
struct WechatInner {
    /// iLink API 基址（登录 confirmed 后可被 `baseurl` 覆盖）。
    base: Mutex<String>,
    /// bot_token；空 = 未扫码登录（poll/send 直接报指引错误）。
    bot_token: Mutex<String>,
    /// 常规调用（15s 超时）。
    http: reqwest::Client,
    /// getupdates 专用（45s 超时 > 服务端 35s 挂起）。
    longpoll: reqwest::Client,
    /// `get_updates_buf` 字符串游标（None = 从未拉过，请求传空串）。
    cursor: Mutex<Option<String>>,
    /// 入站队列（getupdates → admit → 桥 poll 排水；测试 inject 也走这里）。
    inbox: Mutex<VecDeque<InboundMsg>>,
    /// message_id 去重（游标回退兜底；超 [`SEEN_CAP`] 整体清空）。
    seen: Mutex<HashSet<i64>>,
    /// chat_id → (context_token, 收到时刻)：回复回传（TTL [`CONTEXT_TTL`]）。
    contexts: Mutex<HashMap<String, (String, Instant)>>,
    /// errcode=-14 / 401 的静默期：期间 poll 返回空轮不触网。
    pause_until: Mutex<Option<Instant>>,
    /// 连续失败退避（网络/5xx）：1-2 次等 2s，≥3 次等 30s。
    backoff_until: Mutex<Option<Instant>>,
    fail_streak: AtomicU32,
    /// 首轮排水标记：第一次 getupdates 拉到的历史残留只推游标不处理。
    seeded: AtomicBool,
    /// 桥侧游标透传值（最后一条入站消息的 seq）。
    last_seq: AtomicI64,
    /// 测试注入口的自增序号。
    seq_counter: AtomicI64,
}

impl WechatProvider {
    pub fn new(bot_token: impl Into<String>, base: Option<String>) -> Self {
        Self {
            inner: Arc::new(WechatInner {
                base: Mutex::new(
                    base.filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| WECHAT_BASE_DEFAULT.into()),
                ),
                bot_token: Mutex::new(bot_token.into()),
                http: reqwest::Client::builder()
                    .timeout(HTTP_TIMEOUT)
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new()),
                longpoll: reqwest::Client::builder()
                    .timeout(LONGPOLL_TIMEOUT)
                    .build()
                    .unwrap_or_else(|_| reqwest::Client::new()),
                cursor: Mutex::new(None),
                inbox: Mutex::new(VecDeque::new()),
                seen: Mutex::new(HashSet::new()),
                contexts: Mutex::new(HashMap::new()),
                pause_until: Mutex::new(None),
                backoff_until: Mutex::new(None),
                fail_streak: AtomicU32::new(0),
                seeded: AtomicBool::new(false),
                last_seq: AtomicI64::new(0),
                seq_counter: AtomicI64::new(0),
            }),
        }
    }

    /// 从 env 读取（`CMX_AGENT_IM_WECHAT_BOT_TOKEN` 必需，`_BASE` 可选）。
    pub fn from_env() -> Option<Self> {
        let token = env_nonempty("CMX_AGENT_IM_WECHAT_BOT_TOKEN")?;
        Some(Self::new(token, env_nonempty("CMX_AGENT_IM_WECHAT_BASE")))
    }

    /// 测试注入：往入站队列塞一条消息（生产路径由 getupdates 经 admit 入队）。
    pub async fn inject(&self, chat_id: &str, text: &str) {
        self.inject_with_sender(chat_id, text, "").await;
    }

    /// 测试注入（带 sender）：模拟某微信号给机器人发消息，驱动桥层测试（与飞书/QQ 同款）。
    /// 同步登记合成 context_token（回复路径形态与生产一致；回复走网络失败被桥忽略）。
    pub async fn inject_with_sender(&self, chat_id: &str, text: &str, sender: &str) {
        let seq = self.inner.seq_counter.fetch_add(1, Ordering::SeqCst) + 1;
        self.inner
            .contexts
            .lock()
            .await
            .insert(chat_id.into(), ("__test_ctx__".into(), Instant::now()));
        self.inner.inbox.lock().await.push_back(InboundMsg {
            chat_id: chat_id.into(),
            text: text.into(),
            update_id: seq,
            sender: sender.into(),
        });
        self.inner.last_seq.fetch_max(seq, Ordering::SeqCst);
    }
}

/// 扫码状态响应的解析产物（纯函数 [`parse_qr_status`]，无副作用可单测）。
#[derive(Debug, Clone, PartialEq, Eq)]
enum QrRaw {
    Wait,
    Scaned,
    /// IDC 重定向：后续轮询切到该 host。
    Redirect(String),
    Expired,
    Confirmed {
        bot_token: String,
        bot_id: String,
        user_id: String,
        base: Option<String>,
    },
    Unknown(String),
}

/// 纯函数：扫码状态响应 → 解析产物。字段防御式取值（缺 `redirect_host` 兜底继续等）。
fn parse_qr_status(v: &Value) -> QrRaw {
    let status = v.get("status").and_then(|x| x.as_str()).unwrap_or("");
    match status {
        "wait" => QrRaw::Wait,
        "scaned" => QrRaw::Scaned,
        "scaned_but_redirect" => {
            match v.get("redirect_host").and_then(|x| x.as_str()) {
                Some(h) if !h.is_empty() => QrRaw::Redirect(h.to_string()),
                _ => QrRaw::Wait,
            }
        }
        "expired" => QrRaw::Expired,
        "confirmed" => {
            let Some(bot_token) = v.get("bot_token").and_then(|x| x.as_str()) else {
                return QrRaw::Unknown("confirmed 响应缺 bot_token".into());
            };
            let str_field =
                |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            QrRaw::Confirmed {
                bot_token: bot_token.to_string(),
                bot_id: str_field("ilink_bot_id"),
                user_id: str_field("ilink_user_id"),
                base: v
                    .get("baseurl")
                    .and_then(|x| x.as_str())
                    .map(str::trim)
                    .filter(|b| !b.is_empty())
                    .map(str::to_string),
            }
        }
        "" => QrRaw::Unknown("响应缺 status".into()),
        other => QrRaw::Unknown(other.to_string()),
    }
}

/// 取一张二维码（公共头）。⚠ 实测（2026-09-11 真机探测）：`qrcode_img_content` **不是**
/// base64 PNG（社区文档有误），而是**二维码要编码的内容字符串**（一个 `liteapp.weixin.qq.com`
/// 授权页 URL）——二维码图由客户端自行渲染（GUI 用 vendor 的 qrcode.js，CLI 生成自包含 HTML）。
async fn fetch_qr(http: &reqwest::Client, host: &str) -> Result<(String, String), String> {
    let url = format!("{host}/ilink/bot/get_bot_qrcode?bot_type=3");
    let resp = apply_common(http.get(&url))
        .send()
        .await
        .map_err(|e| format!("获取登录二维码请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("获取登录二维码 HTTP {}", resp.status()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("二维码响应解析失败：{e}"))?;
    let qrcode = v
        .get("qrcode")
        .and_then(|x| x.as_str())
        .ok_or_else(|| format!("二维码响应缺 qrcode：{v}"))?
        .to_string();
    let content = v
        .get("qrcode_img_content")
        .and_then(|x| x.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("二维码响应缺 qrcode_img_content：{v}"))?
        .to_string();
    Ok((qrcode, content))
}

/// 扫码状态轮询单次结果（GUI 分步驱动用，每 ~2s 一次）。
#[derive(Debug, Clone)]
pub enum QrEvent {
    /// 等待扫码。
    Waiting,
    /// 已扫码，待手机确认。
    Scaned,
    /// 二维码已过期并自动重取（[`WechatQrSession::qr_content`] 已更新，UI 应重渲染）。
    Refreshed,
    /// 登录成功（token/baseurl 已写回 provider；凭证由调用方落盘 im.json）。
    Confirmed(WechatLogin),
}

/// 一次扫码登录会话：`WechatProvider::qr_begin` 产出，GUI 逐次 [`Self::poll_once`]
/// 驱动（CLI 的 [`WechatProvider::login_qr_with`] 内部是同款循环）。跨调用持有二维码
/// 标识 / 当前 host（IDC 重定向可切换）/ 自动刷新计数 / 总时限。
pub struct WechatQrSession {
    inner: Arc<WechatInner>,
    qrcode: String,
    host: String,
    /// 二维码内容字符串（`qrcode_img_content`，实测为授权页 URL；图由调用方渲染）。
    qr_content: String,
    refreshes: u32,
    /// 状态查询连续失败计数（成功即清零；超 [`LOGIN_POLL_MAX_FAILS`] 才终态）。
    fail_streak: u32,
    deadline: Instant,
}

impl WechatQrSession {
    /// 当前二维码内容字符串（`Refreshed` 后已更新，UI 应重新渲染二维码）。
    pub fn qr_content(&self) -> &str {
        &self.qr_content
    }

    /// 轮询一次扫码状态。内部处理 IDC 重定向（切 host，当次返回 `Waiting`）与
    /// 二维码过期自动重取（≤[`QR_MAX_REFRESH`] 次，返回 `Refreshed`）。
    /// `Confirmed` 时 token/baseurl 已写回 provider，凭证在事件里由调用方落盘。
    ///
    /// 网络瞬断不终态：连续失败 <[`LOGIN_POLL_MAX_FAILS`] 次返回 `Waiting` 自动重试
    /// （成功即清零），超限才透传最后一次错误（实测腾讯侧会偶发瞬断）。
    pub async fn poll_once(&mut self) -> Result<QrEvent, String> {
        match self.poll_inner().await {
            Ok(ev) => {
                self.fail_streak = 0;
                Ok(ev)
            }
            Err(e) => {
                self.fail_streak += 1;
                if self.fail_streak >= LOGIN_POLL_MAX_FAILS {
                    Err(e)
                } else {
                    tracing::warn!(
                        "扫码状态查询失败（第 {}/{} 次），继续重试：{e}",
                        self.fail_streak,
                        LOGIN_POLL_MAX_FAILS
                    );
                    Ok(QrEvent::Waiting)
                }
            }
        }
    }

    /// 单次状态查询（无重试；[`Self::poll_once`] 的内层）。
    async fn poll_inner(&mut self) -> Result<QrEvent, String> {
        if Instant::now() >= self.deadline {
            return Err("扫码登录超时（10 分钟），请重新开始".into());
        }
        let surl = format!("{}/ilink/bot/get_qrcode_status?qrcode={}", self.host, self.qrcode);
        let resp = apply_common(self.inner.http.get(&surl))
            .send()
            .await
            .map_err(|e| format!("查询扫码状态请求失败：{e}"))?;
        if !resp.status().is_success() {
            return Err(format!("查询扫码状态 HTTP {}", resp.status()));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("扫码状态响应解析失败：{e}"))?;
        match parse_qr_status(&v) {
            QrRaw::Wait => Ok(QrEvent::Waiting),
            QrRaw::Scaned => Ok(QrEvent::Scaned),
            QrRaw::Redirect(h) => {
                let next = format!("https://{h}");
                if next != self.host {
                    tracing::info!("微信扫码登录：IDC 重定向 → {next}");
                    self.host = next;
                }
                Ok(QrEvent::Waiting)
            }
            QrRaw::Expired => {
                self.refreshes += 1;
                if self.refreshes > QR_MAX_REFRESH {
                    return Err("二维码多次过期，登录中止".into());
                }
                let (qrcode, content) = fetch_qr(&self.inner.http, &self.host).await?;
                self.qrcode = qrcode;
                self.qr_content = content;
                tracing::info!("微信扫码登录：二维码已过期，自动刷新（第 {} 次）", self.refreshes);
                Ok(QrEvent::Refreshed)
            }
            QrRaw::Confirmed { bot_token, bot_id, user_id, base } => {
                let base = base.unwrap_or_else(|| self.host.clone());
                // token/baseurl 写回自身：登录后同一 provider 实例即可直接收发。
                *self.inner.bot_token.lock().await = bot_token.clone();
                *self.inner.base.lock().await = base.clone();
                let login = WechatLogin { bot_token, bot_id, user_id, base };
                tracing::info!("微信扫码登录成功 bot_id={}", login.bot_id);
                Ok(QrEvent::Confirmed(login))
            }
            QrRaw::Unknown(s) => Err(format!("未知扫码状态：{s}")),
        }
    }
}

impl WechatProvider {
    /// 发起扫码登录：取二维码，返回会话（[`WechatQrSession::qr_content`] 为二维码内容，
    /// 图由调用方渲染）。后续状态用 [`WechatQrSession::poll_once`] 逐步驱动；
    /// CLI 一步到底用 [`Self::login_qr_with`]。
    pub async fn qr_begin(&self) -> Result<WechatQrSession, String> {
        let host = self.base_url().await;
        let (qrcode, content) = fetch_qr(&self.inner.http, &host).await?;
        Ok(WechatQrSession {
            inner: Arc::clone(&self.inner),
            qrcode,
            host,
            qr_content: content,
            refreshes: 0,
            fail_streak: 0,
            deadline: Instant::now() + LOGIN_MAX_WAIT,
        })
    }

    /// 扫码登录全流程（CLI 一步到底）：每次拿到新二维码内容（含过期自动刷新）时回调
    /// `on_qr`（写文件 / 打印等展示由调用方决定），轮询到 confirmed 返回凭证。
    /// GUI 分步形态见 [`Self::qr_begin`] + [`WechatQrSession::poll_once`]。
    pub async fn login_qr_with<F>(&self, mut on_qr: F) -> Result<WechatLogin, String>
    where
        F: FnMut(&str) -> Result<(), String>,
    {
        let mut sess = self.qr_begin().await?;
        on_qr(sess.qr_content())?;
        tracing::info!("微信登录二维码已就绪（用微信 ClawBot 扫码并在手机确认；约 5 分钟有效，过期自动刷新）");
        loop {
            tokio::time::sleep(LOGIN_POLL_INTERVAL).await;
            match sess.poll_once().await? {
                QrEvent::Waiting => {}
                QrEvent::Scaned => tracing::info!("微信扫码登录：已扫码，请在手机上确认"),
                QrEvent::Refreshed => {
                    on_qr(sess.qr_content())?;
                    tracing::info!("二维码已过期，已自动刷新并更新展示");
                }
                QrEvent::Confirmed(login) => return Ok(login),
            }
        }
    }
}

impl WechatProvider {
    /// 当前 API 基址。
    async fn base_url(&self) -> String {
        self.inner.base.lock().await.trim_end_matches('/').to_string()
    }

    /// 读当前 token（空 = 未登录）。
    async fn token(&self) -> String {
        self.inner.bot_token.lock().await.clone()
    }

    /// POST 一个 iLink 端点（公共头 + 鉴权头）。401 → 置 60s 静默期并返回重扫码指引；
    /// 其余状态原样返回响应交调用方判读。
    async fn post(&self, path: &str, body: &Value) -> Result<reqwest::Response, String> {
        let token = self.token().await;
        if token.trim().is_empty() {
            return Err("微信 bot 未扫码登录：先运行 `cmx-agent im-login`（凭证存 im.json）".into());
        }
        let url = format!("{}/ilink/bot/{path}", self.base_url().await);
        let rb = apply_auth(apply_common(self.inner.http.post(&url)), &token);
        let resp = rb
            .json(body)
            .send()
            .await
            .map_err(|e| format!("iLink {path} 请求失败：{e}"))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            *self.inner.pause_until.lock().await = Some(Instant::now() + RELOGIN_PAUSE);
            return Err("微信 bot_token 已失效（HTTP 401）：请重新运行 `cmx-agent im-login` 扫码".into());
        }
        Ok(resp)
    }

    /// 长轮询拉一轮：返回 (新消息, 新游标)。游标回写放调用方（poll）以便失败时保留旧值。
    async fn get_updates(&self) -> Result<(Vec<WeixinMessage>, String), String> {
        let token = self.token().await;
        if token.trim().is_empty() {
            return Err("微信 bot 未扫码登录：先运行 `cmx-agent im-login`（凭证存 im.json）".into());
        }
        let buf = self
            .inner
            .cursor
            .lock()
            .await
            .clone()
            .unwrap_or_default();
        let body = json!({ "get_updates_buf": buf, "base_info": base_info() });
        let url = format!("{}/ilink/bot/getupdates", self.base_url().await);
        let rb = apply_auth(apply_common(self.inner.longpoll.post(&url)), &token);
        let resp = rb
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("getupdates 请求失败：{e}"))?;
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            *self.inner.pause_until.lock().await = Some(Instant::now() + RELOGIN_PAUSE);
            return Err("微信 bot_token 已失效（HTTP 401）：请重新运行 `cmx-agent im-login` 扫码".into());
        }
        if !resp.status().is_success() {
            return Err(format!("getupdates HTTP {}", resp.status()));
        }
        let v: Value = resp
            .json()
            .await
            .map_err(|e| format!("getupdates 响应解析失败：{e}"))?;
        match classify_response(&v) {
            ResponseVerdict::Ok => {}
            ResponseVerdict::StaleToken => {
                *self.inner.pause_until.lock().await = Some(Instant::now() + SESSION_TIMEOUT_PAUSE);
                return Err(
                    "iLink 返回 -14（bot_token 已失效）：暂停 1 小时后自动恢复；持续出现请重新运行 `cmx-agent im-login` 扫码"
                        .into(),
                );
            }
            ResponseVerdict::Failed { code, errmsg } => {
                // 面包屑：带原始响应片段（截断）——iLink 无公开文档，包结构出入以真机为准
                // （方案 R1），这段让用户下次贴日志即可定位。
                let raw = serde_json::to_string(&v).unwrap_or_default();
                let snippet: String = raw.chars().take(200).collect();
                return Err(format!("getupdates code={code} msg={errmsg} resp={snippet}"));
            }
        }
        let msgs: Vec<WeixinMessage> = v
            .get("msgs")
            .and_then(|m| m.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|m| serde_json::from_value(m.clone()).ok())
                    .collect()
            })
            .unwrap_or_default();
        let cursor = v
            .get("get_updates_buf")
            .and_then(|c| c.as_str())
            .unwrap_or("")
            .to_string();
        Ok((msgs, cursor))
    }

    /// 去重登记：首次出现返回 true。id=0（message_id/seq 双缺）不做去重恒放行。
    async fn admit_id(&self, id: i64) -> bool {
        if id == 0 {
            return true;
        }
        let mut seen = self.inner.seen.lock().await;
        if seen.len() > SEEN_CAP {
            seen.clear();
        }
        seen.insert(id)
    }

    /// 一条通过过滤的消息入队（登记 context_token + push inbox + 推进桥游标）。
    async fn admit(&self, m: WeixinMessage) {
        let Some(p) = convert(&m) else { return };
        if !self.admit_id(p.dedup_id).await {
            return;
        }
        if !p.context_token.is_empty() {
            self.inner
                .contexts
                .lock()
                .await
                .insert(p.chat_id.clone(), (p.context_token, Instant::now()));
        }
        self.inner.last_seq.fetch_max(p.seq, Ordering::SeqCst);
        self.inner.inbox.lock().await.push_back(InboundMsg {
            chat_id: p.chat_id,
            text: p.text,
            update_id: p.seq,
            sender: p.sender,
        });
    }

    /// 排水 inbox（全部取出）。
    async fn drain(&self) -> Vec<InboundMsg> {
        let mut q = self.inner.inbox.lock().await;
        std::mem::take(&mut *q).into_iter().collect()
    }

    /// 当前桥游标（无消息时原样返回，保证单调）。
    fn last_seq(&self) -> i64 {
        self.inner.last_seq.load(Ordering::SeqCst)
    }

    /// 查会话的 context_token（过期/缺失 → None，sendmessage 不带该字段）。
    async fn context_for(&self, chat_id: &str) -> Option<String> {
        let map = self.inner.contexts.lock().await;
        match map.get(chat_id) {
            Some((tok, at)) if at.elapsed() < CONTEXT_TTL => Some(tok.clone()),
            _ => None,
        }
    }

    /// 静默期（pause/backoff）是否仍在生效。
    async fn paused(&self) -> bool {
        let now = Instant::now();
        let pause = *self.inner.pause_until.lock().await;
        let backoff = *self.inner.backoff_until.lock().await;
        pause.is_some_and(|t| now < t) || backoff.is_some_and(|t| now < t)
    }
}

/// 生成**自包含**的二维码登录 HTML（CLI 展示用；GUI 用 vendor 的 qrcode.js 直接渲染）。
/// 内嵌 `lib_js`（qrcode-generator）+ `content`（二维码内容），浏览器打开即见二维码。
/// `content` 经 JSON 序列化转义为 JS 字符串字面量；`lib_js` 用位置参数注入（其内含
/// 花括号，不能走捕获式 `{}` 插值）。
pub fn qr_login_html(content: &str, lib_js: &str) -> String {
    let lit = serde_json::to_string(content).unwrap_or_else(|_| "\"\"".into());
    format!(
        r#"<!doctype html><html lang="zh"><head><meta charset="utf-8"><title>微信登录二维码</title></head>
<body style="margin:0;display:flex;flex-direction:column;align-items:center;justify-content:center;min-height:100vh;font-family:system-ui,sans-serif;background:#fff;color:#222">
<div id="qr"></div><p style="font-size:14px">用微信（已开通 ClawBot 插件）扫码，并在手机上确认。二维码约 5 分钟有效。</p>
<script>{0}</script>
<script>
var q = qrcode(0, "M"); q.addData({1}); q.make();
document.getElementById("qr").innerHTML = q.createSvgTag({{ cellSize: 6, margin: 8 }});
</script></body></html>"#,
        lib_js, lit
    )
}

#[async_trait]
impl ImProvider for WechatProvider {
    /// 排水优先：inbox 有货直接返回（测试 inject 路径与拉取间隙到达的消息不触网）；
    /// 空了才长轮询拉一轮。静默期（-14 暂停 / 401 / 连续失败退避）内返回空轮不触网。
    async fn poll(&self, _offset: i64) -> Result<(Vec<InboundMsg>, i64), String> {
        let drained = self.drain().await;
        if !drained.is_empty() {
            return Ok((drained, self.last_seq()));
        }
        if self.paused().await {
            return Ok((Vec::new(), self.last_seq()));
        }
        match self.get_updates().await {
            Ok((msgs, new_cursor)) => {
                *self.inner.cursor.lock().await = Some(new_cursor);
                self.inner.fail_streak.store(0, Ordering::SeqCst);
                *self.inner.backoff_until.lock().await = None;
                if !self.inner.seeded.swap(true, Ordering::SeqCst) {
                    // 首轮：只推游标，历史残留不驱动回合（与飞书/QQ 同口径）。
                    tracing::debug!("微信首轮排水 {} 条历史残留（不处理）", msgs.len());
                } else {
                    for m in msgs {
                        self.admit(m).await;
                    }
                }
                let drained = self.drain().await;
                Ok((drained, self.last_seq()))
            }
            Err(e) => {
                let streak = self.inner.fail_streak.fetch_add(1, Ordering::SeqCst) + 1;
                let wait = if streak <= 2 { FAIL_BACKOFF_SHORT } else { FAIL_BACKOFF_LONG };
                *self.inner.backoff_until.lock().await = Some(Instant::now() + wait);
                Err(e)
            }
        }
    }

    /// 发文本回复：回传该会话最近一次入站的 context_token（过期/缺失则不带）。
    async fn send(&self, chat_id: &str, text: &str) -> Result<(), String> {
        let ctx = self.context_for(chat_id).await;
        let body = build_send_body(chat_id, text, ctx.as_deref(), &new_client_id());
        let resp = self.post("sendmessage", &body).await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let detail = resp.text().await.unwrap_or_default();
            return Err(format!("sendmessage HTTP {status}: {detail}"));
        }
        // 官方 2.4.6 起 sendmessage 会回 JSON（`{ret, errmsg}`）；空体/非 JSON 容忍
        //（旧版服务端无响应体）。ret/errcode 非零 = 服务端拒收（HTTP 仍是 200）。
        if let Ok(v) = resp.json::<Value>().await
            && let ResponseVerdict::Failed { code, errmsg } = classify_response(&v)
        {
            return Err(format!("sendmessage code={code} msg={errmsg}"));
        }
        Ok(())
    }

    /// 文本消息分块上限（官方上限未披露，保守 2000）。
    fn max_chunk(&self) -> usize {
        WECHAT_MAX_CHUNK
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 解析 JSON fixture 为 WeixinMessage。
    fn msg(json_str: &str) -> WeixinMessage {
        serde_json::from_str(json_str).expect("fixture 应能反序列化")
    }

    const USER_TEXT: &str = r#"{"seq":11,"message_id":1001,"from_user_id":"u1@im.wechat","to_user_id":"bot@im.bot","message_type":1,"message_state":0,"context_token":"ctx1","item_list":[{"type":1,"text_item":{"text":"你好"}}]}"#;

    #[test]
    fn client_version_encodes_mmnnpp() {
        assert_eq!(encode_client_version_str("0.3.0"), 768);
        assert_eq!(encode_client_version_str("2.1.1"), 0x0002_0101);
        // 容错：缺段/非数字兜底 0，超范围截断。
        assert_eq!(encode_client_version_str("2"), (2 << 16));
        assert_eq!(encode_client_version_str("x.y.z"), 0);
        assert_eq!(encode_client_version_str("1.300.5"), (1 << 16) | (0xFF << 8) | 5);
    }

    #[test]
    fn random_uin_shape() {
        let a = random_uin();
        let b = random_uin();
        let decoded = STANDARD.decode(&a).expect("应为合法 base64");
        let s = std::str::from_utf8(&decoded).expect("应为十进制字符串");
        assert!(!s.is_empty() && s.chars().all(|c| c.is_ascii_digit()), "{s}");
        assert_ne!(a, b, "随机 u32 连续两次相等的概率 ~2^-32，命中即 bug");
    }

    #[test]
    fn parse_user_text_message() {
        let p = convert(&msg(USER_TEXT)).expect("应解析出单聊文本");
        assert_eq!(p.chat_id, "u1@im.wechat");
        assert_eq!(p.sender, "u1@im.wechat");
        assert_eq!(p.text, "你好");
        assert_eq!(p.dedup_id, 1001);
        assert_eq!(p.seq, 11);
        assert_eq!(p.context_token, "ctx1");
    }

    #[test]
    fn parse_merges_multiple_text_items() {
        let m = msg(r#"{"seq":12,"message_id":1002,"from_user_id":"u1@im.wechat","message_type":1,"context_token":"c","item_list":[{"type":1,"text_item":{"text":"你"}},{"type":1,"text_item":{"text":"好"}}]}"#);
        let p = convert(&m).expect("多文本项应合并");
        assert_eq!(p.text, "你好");
    }

    #[test]
    fn parse_skips_bot_echo_and_group_and_non_text() {
        // BOT 回声
        let bot = msg(r#"{"seq":1,"message_id":1,"from_user_id":"bot@im.bot","message_type":2,"item_list":[{"type":1,"text_item":{"text":"回复"}}]}"#);
        assert!(convert(&bot).is_none());
        // 群聊
        let group = msg(r#"{"seq":2,"message_id":2,"from_user_id":"u@im.wechat","group_id":"g1","message_type":1,"item_list":[{"type":1,"text_item":{"text":"hi"}}]}"#);
        assert!(convert(&group).is_none());
        // 纯图片（type=2 无文本项）
        let image = msg(r#"{"seq":3,"message_id":3,"from_user_id":"u@im.wechat","message_type":1,"item_list":[{"type":2}]}"#);
        assert!(convert(&image).is_none());
        // 文本全空白
        let blank = msg(r#"{"seq":4,"message_id":4,"from_user_id":"u@im.wechat","message_type":1,"item_list":[{"type":1,"text_item":{"text":"   "}}]}"#);
        assert!(convert(&blank).is_none());
        // 缺 from_user_id
        let no_sender = msg(r#"{"seq":5,"message_id":5,"message_type":1,"item_list":[{"type":1,"text_item":{"text":"hi"}}]}"#);
        assert!(convert(&no_sender).is_none());
    }

    #[test]
    fn dedup_id_falls_back_to_seq() {
        let m = msg(r#"{"seq":77,"message_id":0,"from_user_id":"u@im.wechat","message_type":1,"item_list":[{"type":1,"text_item":{"text":"hi"}}]}"#);
        let p = convert(&m).expect("应解析");
        assert_eq!(p.dedup_id, 77, "message_id 缺省时退 seq");
        assert_eq!(p.seq, 77);
    }

    #[tokio::test]
    async fn admit_dedups_by_message_id() {
        let p = WechatProvider::new("t", None);
        assert!(p.admit_id(1001).await);
        assert!(!p.admit_id(1001).await, "同 message_id 二次入队应被去重");
        assert!(p.admit_id(0).await, "id=0 不做去重恒放行");
        assert!(p.admit_id(0).await);
    }

    #[tokio::test]
    async fn seen_cap_clears_eventually() {
        let p = WechatProvider::new("t", None);
        for i in 0..(SEEN_CAP as i64 + 2) {
            let _ = p.admit_id(i).await;
        }
        assert!(p.admit_id(0).await, "超限清空后旧 id 应可重新入队");
    }

    #[test]
    fn send_body_shape() {
        let b = build_send_body("u1@im.wechat", "回复内容", Some("ctx1"), "cid-1");
        assert_eq!(b["msg"]["to_user_id"], "u1@im.wechat");
        assert_eq!(b["msg"]["message_type"], 2);
        assert_eq!(b["msg"]["message_state"], 2);
        assert_eq!(b["msg"]["context_token"], "ctx1");
        assert_eq!(b["msg"]["client_id"], "cid-1");
        assert_eq!(b["msg"]["item_list"][0]["type"], 1);
        assert_eq!(b["msg"]["item_list"][0]["text_item"]["text"], "回复内容");
        assert_eq!(b["base_info"]["channel_version"], ILINK_CHANNEL_VERSION);
        assert_eq!(b["base_info"]["bot_agent"], "OpenClaw", "2.3.1 起必带 bot_agent");

        let no_ctx = build_send_body("u", "hi", None, "cid-2");
        assert!(no_ctx["msg"].get("context_token").is_none(), "无 token 不带字段");
    }

    #[test]
    fn classify_response_matches_official_semantics() {
        // 官方 v2.4.8 monitor.js：ret/errcode **存在且 ≠0** 才是错误；缺失 = 成功。
        // 真机回归：getupdates 空轮响应无 ret 字段，此前被误兜底为 -1 报错。
        assert_eq!(classify_response(&json!({})), ResponseVerdict::Ok);
        assert_eq!(
            classify_response(&json!({ "ret": 0, "errcode": null, "errmsg": null })),
            ResponseVerdict::Ok
        );
        assert_eq!(
            classify_response(&json!({ "msgs": [], "get_updates_buf": "x" })),
            ResponseVerdict::Ok,
            "无 ret 的正常响应应判成功"
        );
        assert_eq!(
            classify_response(&json!({ "errcode": 0 })),
            ResponseVerdict::Ok,
            "errcode=0 是成功不是错误"
        );

        // -14（ret 或 errcode）= token 失效（STALE_TOKEN），单列交调用方暂停。
        assert_eq!(classify_response(&json!({ "ret": -14 })), ResponseVerdict::StaleToken);
        assert_eq!(classify_response(&json!({ "errcode": -14 })), ResponseVerdict::StaleToken);

        // 其它非零码 → Failed（errmsg 可能缺失）。
        assert_eq!(
            classify_response(&json!({ "ret": -1, "errmsg": "boom" })),
            ResponseVerdict::Failed { code: -1, errmsg: "boom".into() }
        );
        assert_eq!(
            classify_response(&json!({ "ret": 7 })),
            ResponseVerdict::Failed { code: 7, errmsg: String::new() },
            "errmsg 缺失兜底空串（真机形态）"
        );
        assert_eq!(
            classify_response(&json!({ "errcode": 401 })),
            ResponseVerdict::Failed { code: 401, errmsg: String::new() }
        );
    }

    #[tokio::test]
    async fn poll_drains_injected_inbox_without_network() {
        let p = WechatProvider::new("t", None);
        p.inject("u1", "第一句").await;
        p.inject_with_sender("u2", "第二句", "u2@im.wechat").await;
        let (msgs, next) = p.poll(0).await.expect("排水不应触网");
        assert_eq!(msgs.len(), 2);
        assert!(next >= 2);
        assert_eq!(msgs[1].sender, "u2@im.wechat");
    }

    #[tokio::test]
    async fn poll_silent_during_pause_and_backoff() {
        let p = WechatProvider::new("t", None);
        *p.inner.pause_until.lock().await = Some(Instant::now() + Duration::from_secs(60));
        let (msgs, _) = p.poll(0).await.expect("暂停期应返回空轮");
        assert!(msgs.is_empty());
        *p.inner.pause_until.lock().await = None;
        *p.inner.backoff_until.lock().await = Some(Instant::now() + Duration::from_secs(30));
        let (msgs, _) = p.poll(0).await.expect("退避期应返回空轮");
        assert!(msgs.is_empty());
    }

    #[tokio::test]
    async fn context_token_expiry() {
        let p = WechatProvider::new("t", None);
        p.inner
            .contexts
            .lock()
            .await
            .insert("u1".into(), ("ctx1".into(), Instant::now()));
        assert_eq!(p.context_for("u1").await.as_deref(), Some("ctx1"));
        p.inner.contexts.lock().await.insert(
            "u2".into(),
            ("old".into(), Instant::now() - (CONTEXT_TTL + Duration::from_secs(1))),
        );
        assert_eq!(p.context_for("u2").await, None, "过期 token 不再回传");
        assert_eq!(p.context_for("missing").await, None);
    }

    #[tokio::test]
    async fn send_without_login_fails_fast_with_hint() {
        let p = WechatProvider::new("", None);
        let err = p.send("u1", "hi").await.expect_err("未登录应报错");
        assert!(err.contains("im-login"), "{err}");
        // 静默期字段不被 send 触碰（fail fast 在网络之前）。
        assert!(p.inner.pause_until.lock().await.is_none());
    }

    #[test]
    fn qr_status_parse_variants() {
        assert_eq!(parse_qr_status(&json!({"status":"wait"})), QrRaw::Wait);
        assert_eq!(parse_qr_status(&json!({"status":"scaned"})), QrRaw::Scaned);
        assert_eq!(
            parse_qr_status(&json!({"status":"scaned_but_redirect","redirect_host":"a.b.c"})),
            QrRaw::Redirect("a.b.c".into())
        );
        // 缺 redirect_host：兜底继续等（不当致命错误）。
        assert_eq!(parse_qr_status(&json!({"status":"scaned_but_redirect"})), QrRaw::Wait);
        assert_eq!(parse_qr_status(&json!({"status":"expired"})), QrRaw::Expired);
        assert_eq!(
            parse_qr_status(&json!({"status":"confirmed","bot_token":"tok","ilink_bot_id":"b@im.bot","ilink_user_id":"u1","baseurl":"https://x"})),
            QrRaw::Confirmed {
                bot_token: "tok".into(),
                bot_id: "b@im.bot".into(),
                user_id: "u1".into(),
                base: Some("https://x".into()),
            }
        );
        // confirmed 缺 bot_token → Unknown（不 panic、不误报成功）。
        assert_eq!(
            parse_qr_status(&json!({"status":"confirmed"})),
            QrRaw::Unknown("confirmed 响应缺 bot_token".into())
        );
        assert_eq!(parse_qr_status(&json!({"status":"bogus"})), QrRaw::Unknown("bogus".into()));
        assert_eq!(parse_qr_status(&json!({})), QrRaw::Unknown("响应缺 status".into()));
    }

    #[test]
    fn qr_login_html_embeds_lib_and_content() {
        let lib = "/* qrcode-generator stub */ var qrcode=function(){};";
        let html = qr_login_html("https://liteapp.weixin.qq.com/q/7GiQu1?qrcode=c01b", lib);
        assert!(html.contains(lib), "应内嵌渲染库");
        assert!(
            html.contains(r#""https://liteapp.weixin.qq.com/q/7GiQu1?qrcode=c01b""#),
            "内容应作为 JSON 字符串字面量转义嵌入"
        );
        // content 含引号也不炸（JSON 转义）。
        let html2 = qr_login_html("a\"b", "");
        assert!(html2.contains(r#""a\"b""#));
    }
}
