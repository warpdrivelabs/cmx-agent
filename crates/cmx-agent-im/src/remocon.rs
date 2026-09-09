//! IM 遥控连接配置的 GUI 持久化（`<data_dir>/im.json`）+ 凭证连通性预检。
//!
//! 与 `model.json` 同范式：**env（`CMX_AGENT_IM_*`）> im.json** —— 开发联调用 env 覆盖，
//! 最终用户经桌面壳「设置 → IM 遥控」面板写入 im.json（无需会 env/编辑文本文件）。
//! 壳启动装配入口 [`resolve`]：先试 env、再试 im.json，都不可用返回 Err（壳据此跳过 IM）。
//!
//! 安全：im.json 含 app_secret/token 明文（用户自己的凭证，本机 data_dir，与 model.json
//! 存 api_key 同级）；面板读取走 [`ImRemoconConfig::masked`] 脱敏回显，改密文不回传明文。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{FeishuProvider, ImKind, ImProvider, TelegramProvider, config::parse_kind, config::ImConfig};

/// 飞书国内开放平台基址（与 `feishu.rs` 的默认一致；面板「区域」默认值用）。
pub(crate) const FEISHU_BASE_CN: &str = "https://open.feishu.cn";

/// IM 遥控连接配置（im.json 的 schema）。GUI 面板 get/set 的落盘形态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImRemoconConfig {
    /// 总开关：false = 已保存凭证但停用（壳跳过 IM 桥）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// provider 类型：`feishu` / `telegram`。
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub feishu: FeishuCreds,
    #[serde(default)]
    pub telegram: TelegramCreds,
    /// chat_id 白名单；**空 = 不限**（绑定模式的门 = 已绑定身份，推荐普通用户留空）。
    #[serde(default)]
    pub allow: Vec<String>,
}

/// 飞书自建应用凭证。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FeishuCreds {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    /// 空 = 默认国内 `https://open.feishu.cn`；海外 Lark 填 `https://open.larksuite.com`。
    #[serde(default)]
    pub base: String,
}

/// Telegram bot 凭证。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TelegramCreds {
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub base: String,
}

fn default_true() -> bool {
    true
}

/// im.json 路径（`<data_dir>/im.json`）。
pub fn im_config_path(data_dir: &Path) -> PathBuf {
    data_dir.join("im.json")
}

/// 读 im.json；不存在/损坏返回 None（视为未配置，面板显示默认值）。
pub fn load_im_config(data_dir: &Path) -> Option<ImRemoconConfig> {
    let text = std::fs::read_to_string(im_config_path(data_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 写 im.json（自动建目录）。失败返回人类可读原因。
pub fn save_im_config(data_dir: &Path, cfg: &ImRemoconConfig) -> Result<(), String> {
    std::fs::create_dir_all(data_dir).map_err(|e| format!("创建数据目录失败：{e}"))?;
    let text = serde_json::to_string_pretty(cfg).map_err(|e| format!("序列化 im.json 失败：{e}"))?;
    std::fs::write(im_config_path(data_dir), text).map_err(|e| format!("写入 im.json 失败：{e}"))
}

impl Default for ImRemoconConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            kind: "feishu".into(),
            feishu: FeishuCreds::default(),
            telegram: TelegramCreds::default(),
            allow: Vec::new(),
        }
    }
}

impl ImRemoconConfig {
    /// 脱敏视图（面板 get 回显）：secret/token 只给掩码，明文不出后端。
    pub fn masked(&self) -> Value {
        json!({
            "configured": true,
            "enabled": self.enabled,
            "kind": self.kind,
            "app_id": self.feishu.app_id,
            "app_secret_masked": mask_secret(&self.feishu.app_secret),
            "base": self.feishu.base,
            "telegram_token_masked": mask_secret(&self.telegram.token),
            "allow": self.allow.join(", "),
        })
    }

    /// 文件配置 → 运行装配件。凭证缺失返回 Err（带缺哪个），供面板/启动日志直说问题。
    fn to_resolved(&self) -> Result<ResolvedIm, String> {
        let kind = parse_kind(&self.kind).map_err(|e| format!("im.json {e}"))?;
        let allow = if self.allow.is_empty() {
            None
        } else {
            Some(self.allow.iter().cloned().collect::<HashSet<String>>())
        };
        let provider: Arc<dyn ImProvider> = match kind {
            ImKind::Feishu => {
                let f = &self.feishu;
                if f.app_id.trim().is_empty() {
                    return Err("im.json 已选 feishu 但缺 App ID".into());
                }
                if f.app_secret.trim().is_empty() {
                    return Err("im.json 已选 feishu 但缺 App Secret".into());
                }
                Arc::new(FeishuProvider::new(
                    f.app_id.trim(),
                    f.app_secret.trim(),
                    non_empty(&f.base),
                ))
            }
            ImKind::Telegram => {
                let t = &self.telegram;
                if t.token.trim().is_empty() {
                    return Err("im.json 已选 telegram 但缺 Bot Token".into());
                }
                Arc::new(TelegramProvider::new(t.token.trim(), non_empty(&t.base)))
            }
        };
        Ok(ResolvedIm { kind, allow, provider, source: "im.json" })
    }
}

/// 壳启动装好的 IM 遥控：provider + 白名单 + 来源标签（日志/面板提示用）。
pub struct ResolvedIm {
    pub kind: ImKind,
    /// `None` = 不限（绑定模式门 = 已绑定身份）。
    pub allow: Option<HashSet<String>>,
    pub provider: Arc<dyn ImProvider>,
    /// `"env"`（开发联调）或 `"im.json"`（GUI 面板）。
    pub source: &'static str,
}

/// 壳启动装配：**env 优先**（开发联调覆盖），回落 im.json（GUI）。两者皆无 → Err（壳跳过 IM）。
///
/// env 路径沿用 [`ImConfig::from_env`]（含白名单校验）；注意 env 只配凭证不配白名单时会落到
/// im.json 分支——开发联调请同时配 `CMX_AGENT_IM_ALLOW` 或 `CMX_AGENT_IM_NO_ALLOW=1`。
pub fn resolve(data_dir: Option<&Path>) -> Result<ResolvedIm, String> {
    // 1) env（开发联调覆盖）
    if let Ok(cfg) = ImConfig::from_env()
        && let Ok(provider) = cfg.build_provider()
    {
        return Ok(ResolvedIm {
            kind: cfg.kind,
            allow: cfg.allow,
            provider,
            source: "env",
        });
    }
    // 2) im.json（GUI 面板保存）
    if let Some(dir) = data_dir
        && let Some(cfg) = load_im_config(dir)
    {
        if !cfg.enabled {
            return Err("IM 遥控已在设置中停用".into());
        }
        return cfg.to_resolved();
    }
    Err("未配置 IM 遥控（env 与 im.json 均无，桌面壳为纯本地模式）".into())
}

/// 面板提示用：当前 env 是否生效（env 配齐时 GUI 保存的 im.json **不生效**，面板须提示）。
pub fn env_active() -> bool {
    ImConfig::from_env()
        .map(|c| c.build_provider().is_ok())
        .unwrap_or(false)
}

/// 飞书凭证连通性预检（面板「测试连接」）：tenant_access_token + Stream endpoint 两跳。
/// 与 [`FeishuProvider`] 内部请求同款端点，但不建长连接——秒级返回，配错当场暴露。
pub async fn test_feishu(app_id: &str, app_secret: &str, base: Option<&str>) -> Result<String, String> {
    if app_id.trim().is_empty() || app_secret.trim().is_empty() {
        return Err("App ID / App Secret 不能为空".into());
    }
    let base = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or(FEISHU_BASE_CN)
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))?;

    // 1) tenant_access_token：凭证对不对、应用在不在。
    let v: Value = client
        .post(format!("{base}/open-apis/auth/v3/tenant_access_token/internal"))
        .json(&json!({ "app_id": app_id.trim(), "app_secret": app_secret.trim() }))
        .send()
        .await
        .map_err(|e| format!("请求失败（网络不通或地址错）：{e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败：{e}"))?;
    if v.get("code").and_then(|c| c.as_i64()) != Some(0) {
        return Err(format!(
            "凭证无效：code={} msg={}",
            v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
            v.get("msg").and_then(|m| m.as_str()).unwrap_or("")
        ));
    }

    // 2) Stream endpoint：长连接可用性（未开通「事件订阅-长连接模式」在此暴露）。
    let v: Value = client
        .post(format!("{base}/callback/ws/endpoint"))
        .header("locale", "zh")
        .json(&json!({ "AppID": app_id.trim(), "AppSecret": app_secret.trim() }))
        .send()
        .await
        .map_err(|e| format!("endpoint 请求失败：{e}"))?
        .json()
        .await
        .map_err(|e| format!("endpoint 解析失败：{e}"))?;
    if v.get("code").and_then(|c| c.as_i64()) != Some(0) {
        return Err(format!(
            "Stream 通道不可用：code={} msg={}（检查应用是否开通事件订阅·长连接模式）",
            v.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
            v.get("msg").and_then(|m| m.as_str()).unwrap_or("")
        ));
    }
    if v.pointer("/data/URL").and_then(|u| u.as_str()).is_none() {
        return Err("endpoint 响应缺 data.URL".into());
    }
    Ok("✓ 凭证有效，飞书 Stream 通道可用".into())
}

/// trim 后非空才 `Some`（provider base 可选参数用）。
fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// 密文掩码：短值全打码，长值保尾 4 位（照 `ModelProviderConfig::masked_api_key` 范式）。
fn mask_secret(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }
    let n = s.chars().count();
    if n <= 8 {
        return "*".repeat(n);
    }
    let tail: String = s.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("...{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 唯一临时目录（照仓内离线测试约定：std::env::temp_dir + 唯一后缀，不引 tempfile）。
    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cmx-im-remocon-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|x| x.subsec_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tmp_dir("roundtrip");
        let cfg = ImRemoconConfig {
            enabled: true,
            kind: "feishu".into(),
            feishu: FeishuCreds {
                app_id: "cli_x".into(),
                app_secret: "sec-secret-secret".into(),
                base: String::new(),
            },
            telegram: TelegramCreds::default(),
            allow: vec!["oc_a".into(), "oc_b".into()],
        };
        save_im_config(&dir, &cfg).unwrap();
        let back = load_im_config(&dir).expect("应能读回");
        assert_eq!(back.feishu.app_id, "cli_x");
        assert_eq!(back.feishu.app_secret, "sec-secret-secret");
        assert_eq!(back.allow, vec!["oc_a", "oc_b"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_missing_or_corrupt_is_none() {
        let dir = tmp_dir("missing");
        assert!(load_im_config(&dir).is_none()); // 文件不存在
        std::fs::write(im_config_path(&dir), "{not json").unwrap();
        assert!(load_im_config(&dir).is_none()); // 损坏 → 视为未配置
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn default_partial_json_fills_defaults() {
        // 旧版本/手写的 im.json 只有部分字段：serde default 补齐，不炸。
        let dir = tmp_dir("partial");
        std::fs::write(im_config_path(&dir), r#"{"kind":"feishu","feishu":{"app_id":"cli_1","app_secret":"s"}}"#)
            .unwrap();
        let cfg = load_im_config(&dir).unwrap();
        assert!(cfg.enabled); // default_true
        assert!(cfg.allow.is_empty());
        assert_eq!(cfg.feishu.app_id, "cli_1");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn masked_hides_secret() {
        let cfg = ImRemoconConfig {
            telegram: TelegramCreds { token: "1234567890abcd".into(), ..Default::default() },
            ..Default::default()
        };
        let m = cfg.masked();
        assert_eq!(m["app_secret_masked"], ""); // 未配置 → 空串
        assert_eq!(m["telegram_token_masked"], "...abcd");
        assert!(!m.to_string().contains("1234567890abcd"));
    }

    #[test]
    fn to_resolved_validates_creds() {
        // feishu 缺 secret → Err 带字段名
        let bad = ImRemoconConfig { kind: "feishu".into(), ..Default::default() };
        let err = match bad.to_resolved() {
            Err(e) => e,
            Ok(_) => panic!("缺凭证应 Err"),
        };
        assert!(err.contains("App ID"), "{err}");

        // 齐备 → Ok，allow 空表示不限
        let ok = ImRemoconConfig {
            kind: "feishu".into(),
            feishu: FeishuCreds { app_id: "cli_1".into(), app_secret: "s".into(), base: String::new() },
            ..Default::default()
        };
        let r = ok.to_resolved().unwrap();
        assert_eq!(r.kind, ImKind::Feishu);
        assert!(r.allow.is_none());

        // telegram 缺 token → Err
        let tg = ImRemoconConfig { kind: "telegram".into(), ..Default::default() };
        assert!(tg.to_resolved().is_err());
    }

    #[test]
    fn to_resolved_allow_listed() {
        let cfg = ImRemoconConfig {
            kind: "feishu".into(),
            feishu: FeishuCreds { app_id: "i".into(), app_secret: "s".into(), base: String::new() },
            allow: vec!["oc_a".into()],
            ..Default::default()
        };
        let r = cfg.to_resolved().unwrap();
        let allow = r.allow.expect("应返回白名单");
        assert!(allow.contains("oc_a"));
    }

    #[test]
    fn mask_secret_shapes() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("1234"), "****");
        assert_eq!(mask_secret("12345678"), "********");
        assert_eq!(mask_secret("123456789"), "...6789");
    }
}
