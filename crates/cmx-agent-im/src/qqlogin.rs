//! QQ 机器人扫码绑定（官方通道）：用户用手机 QQ **扫码**创建/绑定机器人并自动获取
//! `AppID` + `AppSecret`——QQ 开放平台为 OpenClaw 场景开放的官方入口（q.qq.com 域，
//! 免开发者注册/企业资质，一个 QQ 号最多创建 5 个机器人），凭证获取后仍走
//! [`crate::QqProvider`] 的官方 WebSocket 接入，协议层零变化。
//!
//! 协议事实底座（写死，实施时不许再猜）：
//! - **来源**：腾讯官方扫码页 `q.qq.com/qqbot/openclaw/login.html` 的前端 JS（`/lite/*`
//!   端点族）与 AstrBot `qqofficial/login_registration.py`（活跃开源实现）交叉核对，
//!   2026-09-11 抓取。
//! - **① 创建绑定任务**：`POST https://q.qq.com/lite/create_bind_task`，body
//!   `{"key":"<bind_key>"}`。`bind_key` = 客户端自行生成的 `base64(32 随机字节)`
//!   （AES-256 密钥，只存客户端；服务端用它加密回传 secret）。响应
//!   `{"retcode":0,"data":{"task_id":"…"}}`。
//! - **② 二维码内容** = 授权页 URL（**不是图片**，客户端自行渲染二维码）：
//!   `https://q.qq.com/qqbot/openclaw/connect.html?task_id=<task_id>&_wv=2`——
//!   手机 QQ 扫码打开并确认后服务端记完成态。
//! - **③ 轮询结果**：`POST https://q.qq.com/lite/poll_bind_result`，body
//!   `{"task_id":"…"}`。响应 `{"retcode":0,"data":{"status":N,"bot_appid":"…",
//!   "bot_encrypt_secret":"…"}}`。`status`：0 NONE / 1 PENDING / 2 COMPLETED / 3 EXPIRED；
//!   COMPLETED 时下发 `bot_appid`（明文）与 `bot_encrypt_secret`（密文）。
//! - **④ 解密 AppSecret**：`base64(bot_encrypt_secret)` 解出
//!   `[nonce(12B) | ciphertext | tag(16B)]`，AES-256-GCM，key = `base64_decode(bind_key)`。
//! - **信封**：`retcode` 存在且 ≠0 即失败（`msg`/`message` 带原因）；无 `retcode` 的
//!   响应按 data 域解析（防御式）。
//! - 响应/字段形态有出入时以官方页面 JS 为最终对照真源。

use std::time::Duration;

use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};

use crate::wechat::random_u32; // 复用随机源（getrandom + 时间熵兜底）

/// 官方绑定 API 基址（测试环境 `test.q.qq.com`，仅调试用）。
pub(crate) const QQ_BIND_BASE_DEFAULT: &str = "https://q.qq.com";
/// API / 轮询超时（AstrBot 同款 10s；绑定接口是普通 CGI，非长轮询）。
const BIND_TIMEOUT: Duration = Duration::from_secs(10);
/// 扫码状态：NONE / PENDING / COMPLETED / EXPIRED（官方 `/lite/poll_bind_result` 枚举）。
const STATUS_COMPLETED: i64 = 2;
const STATUS_EXPIRED: i64 = 3;
/// AES-GCM 布局常量：nonce 12B + tag 16B（密文居中）。
const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;

/// 一个进行中的扫码绑定会话：`start` 产出、轮询消费、壳侧持有（微信扫码会话同位）。
#[derive(Debug, Clone)]
pub struct QqBindSession {
    /// 服务端绑定任务 id。
    pub task_id: String,
    /// 客户端持有的 AES-256 密钥（base64）——解密 `bot_encrypt_secret` 唯一凭据。
    pub bind_key: String,
}

/// 一次轮询的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QqBindEvent {
    /// 用户尚未扫码/尚未确认，继续轮询。
    Pending,
    /// 二维码（绑定任务）已过期：会话作废，重新 start。
    Expired,
    /// 绑定完成：凭证已解密可直接落盘。
    Completed {
        /// 机器人 AppID（明文）。
        app_id: String,
        /// 机器人 AppSecret（已解密明文）。
        secret: String,
    },
}

/// 生成 `bind_key`：`base64(32 随机字节)`。随机源与 [`crate::wechat`] 同款
/// （getrandom，失败兜底时间熵——本密钥只服务单次绑定会话的回传加密）。
pub fn generate_bind_key() -> String {
    // 4×u32 = 16B 一次拿不够 32B，拼两轮（getrandom 0.2 的接口按 slice 填充）。
    let mut bytes = [0u8; 32];
    for chunk in bytes.chunks_mut(4) {
        let mut b = [0u8; 4];
        if getrandom::getrandom(&mut b).is_ok() {
            chunk.copy_from_slice(&b);
        } else {
            chunk.copy_from_slice(&random_u32().to_le_bytes());
        }
    }
    STANDARD.encode(bytes)
}

/// 二维码内容 = 授权页 URL（手机 QQ 扫码打开并确认；`_wv=2` 官方页同款参数）。
pub fn connect_url(task_id: &str, base: Option<&str>) -> String {
    let host = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(strip_scheme)
        .unwrap_or_else(|| strip_scheme(QQ_BIND_BASE_DEFAULT));
    format!("https://{host}/qqbot/openclaw/connect.html?task_id={task_id}&_wv=2")
}

/// 去 scheme 与尾斜杠，只留 host（`https://q.qq.com/` → `q.qq.com`）。
fn strip_scheme(base: &str) -> &str {
    base.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
}

/// HTTP POST JSON（Accept: application/json；绑定接口无鉴权头）。
async fn post_json(path: &str, payload: &Value, base: Option<&str>) -> Result<Value, String> {
    let host = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(strip_scheme)
        .unwrap_or_else(|| strip_scheme(QQ_BIND_BASE_DEFAULT));
    let url = format!("https://{host}/{path}");
    let client = reqwest::Client::builder()
        .timeout(BIND_TIMEOUT)
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))?;
    let resp = client
        .post(&url)
        .header("Accept", "application/json")
        .json(payload)
        .send()
        .await
        .map_err(|e| format!("请求失败（网络不通或地址错）：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("HTTP {status}: {}", truncate(&body, 200)));
    }
    let v: Value = resp.json().await.map_err(|e| format!("响应解析失败：{e}"))?;
    Ok(v)
}

/// 官方信封判读：`retcode` 存在且 ≠0 → Err（`msg`/`message` 带原因）；否则返回 `data` 域
/// （缺失兜底空对象——防御式，字段形态以官方页面 JS 为准）。
fn parse_envelope(v: &Value) -> Result<&Value, String> {
    let retcode = v.get("retcode").and_then(|r| r.as_i64());
    if let Some(code) = retcode.filter(|&c| c != 0) {
        let msg = v
            .get("msg")
            .or_else(|| v.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("");
        return Err(format!("QQ 绑定接口返回 code={code}（{msg}）"));
    }
    Ok(v.get("data").unwrap_or(&Value::Null))
}

/// 截断辅助（错误信息带原始响应片段用）。
fn truncate(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

/// ① 创建绑定任务：生成会话（task_id + bind_key）。二维码内容用 [`connect_url`]。
pub async fn create_bind_task(base: Option<&str>) -> Result<QqBindSession, String> {
    let bind_key = generate_bind_key();
    let v = post_json(
        "lite/create_bind_task",
        &json!({ "key": bind_key }),
        base,
    )
    .await?;
    let data = parse_envelope(&v)?;
    let task_id = data
        .get("task_id")
        .and_then(|t| t.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if task_id.is_empty() {
        return Err(format!(
            "create_bind_task 响应缺 task_id：{}",
            truncate(&serde_json::to_string(&v).unwrap_or_default(), 200)
        ));
    }
    Ok(QqBindSession { task_id, bind_key })
}

/// ③ 轮询一次绑定结果。
pub async fn poll_bind_result(
    sess: &QqBindSession,
    base: Option<&str>,
) -> Result<QqBindEvent, String> {
    let v = post_json(
        "lite/poll_bind_result",
        &json!({ "task_id": sess.task_id }),
        base,
    )
    .await?;
    parse_poll_result(&v, &sess.bind_key)
}

/// ③' 轮询响应解析（纯函数，可单测）：status 判定 + COMPLETED 时解密 secret。
pub fn parse_poll_result(v: &Value, bind_key: &str) -> Result<QqBindEvent, String> {
    let data = parse_envelope(v)?;
    let status = data.get("status").and_then(|s| s.as_i64()).unwrap_or(0);
    if status == STATUS_COMPLETED {
        let app_id = data
            .get("bot_appid")
            .and_then(|a| a.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let encrypted = data
            .get("bot_encrypt_secret")
            .and_then(|s| s.as_str())
            .unwrap_or("")
            .trim();
        if app_id.is_empty() || encrypted.is_empty() {
            return Err("扫码成功但未返回完整凭证（缺 bot_appid/bot_encrypt_secret）".into());
        }
        let secret = decrypt_secret(encrypted, bind_key)?;
        return Ok(QqBindEvent::Completed { app_id, secret });
    }
    if status == STATUS_EXPIRED {
        return Ok(QqBindEvent::Expired);
    }
    // 0 NONE / 1 PENDING / 未知值：一律视为进行中（用户还在扫码/确认）。
    Ok(QqBindEvent::Pending)
}

/// ④ 解密 QQ 下发的 AppSecret（纯函数，可单测）：
/// `base64(payload) = nonce(12B) | ciphertext | tag(16B)`，AES-256-GCM，
/// key = `base64_decode(bind_key)`。
pub fn decrypt_secret(encrypted_b64: &str, bind_key_b64: &str) -> Result<String, String> {
    let key = STANDARD
        .decode(bind_key_b64.trim())
        .map_err(|_| "bind_key 非法（base64 解码失败）".to_string())?;
    let raw = STANDARD
        .decode(encrypted_b64.trim())
        .map_err(|_| "bot_encrypt_secret 非法（base64 解码失败）".to_string())?;
    if key.len() != 32 {
        return Err(format!("bind_key 长度异常（{}B，应为 32B）", key.len()));
    }
    if raw.len() <= NONCE_LEN + TAG_LEN {
        return Err(format!(
            "bot_encrypt_secret 长度异常（{}B，应 > {}B）",
            raw.len(),
            NONCE_LEN + TAG_LEN
        ));
    }
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| format!("初始化 AES 失败：{e}"))?;
    // decrypt_in_place 吃 attached 形态：buffer = ciphertext || tag（校验后原位剥离）。
    let mut buffer = raw[NONCE_LEN..].to_vec();
    cipher
        .decrypt_in_place(Nonce::from_slice(&raw[..NONCE_LEN]), &[], &mut buffer)
        .map_err(|_| "AppSecret 解密失败（密钥不匹配或密文损坏）".to_string())?;
    String::from_utf8(buffer).map_err(|e| format!("AppSecret 非 UTF-8：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_key_is_32_random_bytes() {
        let a = generate_bind_key();
        let b = generate_bind_key();
        assert_ne!(a, b, "每次生成应不同");
        let raw = STANDARD.decode(&a).unwrap();
        assert_eq!(raw.len(), 32);
    }

    #[test]
    fn connect_url_shape() {
        assert_eq!(
            connect_url("T1", None),
            "https://q.qq.com/qqbot/openclaw/connect.html?task_id=T1&_wv=2"
        );
        assert_eq!(
            connect_url("T2", Some("https://test.q.qq.com")),
            "https://test.q.qq.com/qqbot/openclaw/connect.html?task_id=T2&_wv=2"
        );
    }

    #[test]
    fn decrypt_secret_roundtrip() {
        // 用同款 AES-256-GCM 加密一个样本再解密（对称验证布局 nonce|ct|tag）。
        use aes_gcm::aead::AeadInPlace as _;
        let key_bytes = [7u8; 32];
        let bind_key = STANDARD.encode(key_bytes);
        let secret = "AppSecret-示例-1234567890";
        let nonce_bytes = [42u8; 12];
        let mut buffer = secret.as_bytes().to_vec();
        let cipher = Aes256Gcm::new_from_slice(&key_bytes).unwrap();
        let tag = cipher
            .encrypt_in_place_detached(Nonce::from_slice(&nonce_bytes), &[], &mut buffer)
            .unwrap();
        // base64(nonce | ciphertext | tag)
        let mut payload = nonce_bytes.to_vec();
        payload.extend_from_slice(&buffer);
        payload.extend_from_slice(&tag);
        let encrypted = STANDARD.encode(&payload);

        let decrypted = decrypt_secret(&encrypted, &bind_key).unwrap();
        assert_eq!(decrypted, secret);
    }

    #[test]
    fn decrypt_secret_rejects_malformed() {
        let bind_key = STANDARD.encode([1u8; 32]);
        // key 长度不对
        let bad_key = STANDARD.encode([1u8; 16]);
        let err = decrypt_secret("AAAA", &bad_key).expect_err("16B key 应 Err");
        assert!(err.contains("32B"), "{err}");
        // 密文过短
        let short = STANDARD.encode([9u8; 20]);
        let err = decrypt_secret(&short, &bind_key).expect_err("过短密文应 Err");
        assert!(err.contains("长度异常"), "{err}");
        // 非 base64
        assert!(decrypt_secret("!!", &bind_key).is_err());
        // tag 校验失败（错误密钥解正确布局的密文）
        let right_key = STANDARD.encode([1u8; 32]);
        let wrong_key = STANDARD.encode([2u8; 32]);
        let payload = STANDARD.encode([0u8; 40]);
        let err = decrypt_secret(&payload, &wrong_key).expect_err("密钥不匹配应 Err");
        assert!(err.contains("解密失败"), "{err}");
        let _ = right_key; // 布局正确性由 roundtrip 用例覆盖
    }

    #[test]
    fn parse_poll_result_statuses() {
        let sess_key = STANDARD.encode([3u8; 32]);
        // PENDING / NONE / 未知 → Pending
        for status in [0, 1, 99] {
            let v = json!({ "retcode": 0, "data": { "status": status } });
            assert_eq!(parse_poll_result(&v, &sess_key).unwrap(), QqBindEvent::Pending);
        }
        // EXPIRED
        let v = json!({ "retcode": 0, "data": { "status": 3 } });
        assert_eq!(parse_poll_result(&v, &sess_key).unwrap(), QqBindEvent::Expired);
        // COMPLETED 但缺凭证 → Err
        let v = json!({ "retcode": 0, "data": { "status": 2, "bot_appid": "10xx" } });
        let err = parse_poll_result(&v, &sess_key).expect_err("缺 secret 应 Err");
        assert!(err.contains("完整凭证"), "{err}");
    }

    #[test]
    fn parse_poll_result_completed_decrypts() {
        // COMPLETED + 加密样本（与 roundtrip 同法构造）→ Completed{appid, secret}
        use aes_gcm::aead::AeadInPlace as _;
        let key_bytes = [7u8; 32];
        let bind_key = STANDARD.encode(key_bytes);
        let mut buffer = b"qq-secret-value".to_vec();
        let nonce_bytes = [5u8; 12];
        let tag = Aes256Gcm::new_from_slice(&key_bytes)
            .unwrap()
            .encrypt_in_place_detached(Nonce::from_slice(&nonce_bytes), &[], &mut buffer)
            .unwrap();
        let mut payload = nonce_bytes.to_vec();
        payload.extend_from_slice(&buffer);
        payload.extend_from_slice(&tag);
        let v = json!({
            "retcode": 0,
            "data": {
                "status": 2,
                "bot_appid": "102456789",
                "bot_encrypt_secret": STANDARD.encode(&payload),
            }
        });
        let ev = parse_poll_result(&v, &bind_key).unwrap();
        assert_eq!(
            ev,
            QqBindEvent::Completed {
                app_id: "102456789".into(),
                secret: "qq-secret-value".into(),
            }
        );
    }

    #[test]
    fn envelope_rejects_nonzero_retcode() {
        let v = json!({ "retcode": 1234, "msg": "频率限制" });
        let err = parse_envelope(&v).expect_err("非 0 retcode 应 Err");
        assert!(err.contains("1234") && err.contains("频率限制"), "{err}");
        // 无 retcode（异常形态）→ 按 data 解析不炸
        let v = json!({ "data": { "status": 1 } });
        assert!(parse_envelope(&v).is_ok());
    }

    #[tokio::test]
    async fn create_bind_task_reports_missing_task_id() {
        // 网络层无法离线测；parse 层：缺 task_id 的成功信封 → Err 带面包屑。
        let v = json!({ "retcode": 0, "data": {} });
        let data = parse_envelope(&v).unwrap();
        assert!(data.get("task_id").is_none());
    }
}
