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

use crate::wechat::{
    WECHAT_BASE_DEFAULT, ResponseVerdict, apply_auth, apply_common, base_info, classify_response,
};
use crate::{
    FeishuProvider, ImKind, ImProvider, QqProvider, WechatProvider,
    config::ImConfig, config::parse_kind,
};

/// 飞书国内开放平台基址（与 `feishu.rs` 的默认一致；面板「区域」默认值用）。
pub(crate) const FEISHU_BASE_CN: &str = "https://open.feishu.cn";

/// IM 遥控连接配置（im.json 的 schema）。GUI 面板 get/set 的落盘形态。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImRemoconConfig {
    /// 总开关：false = 已保存凭证但停用（壳跳过 IM 桥）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 主 provider 类型：`feishu` / `qq` / `wechat`。兼容保留字段——多通道语义见
    /// [`Self::active`]；旧版本/手写的 im.json 只有本字段（单选），`resolve` 按它回落。
    #[serde(default)]
    pub kind: String,
    /// **多通道**（2026-09-10）：同时启用的 provider 列表（`feishu`/`qq`/`wechat` 可组合）。
    /// 空 = 旧版单选文件，回落 [`Self::kind`]。GUI set 恒写入本字段；`kind` 同步为首个启用项。
    /// 旧文件里已不支持的 kind（telegram 等）读入时被 [`Self::active`] 过滤。
    #[serde(default)]
    pub active: Vec<String>,
    /// 个人模式（默认 true）：所有 IM 消息直接以桌面壳当前登录用户身份跑回合，
    /// 无需验证码绑定（自己私聊自己的机器人）。false = 绑定模式（按发送者 open_id
    /// 鉴权，群聊多用户用，需在桌面端取码完成绑定）。
    #[serde(default = "default_true")]
    pub personal: bool,
    /// 无人值守全权（默认 true）：IM 回合以回合级覆盖档执行——沙箱完全放行、从不弹审批卡
    /// （审批档 Never：条件级人审被 danger 豁免，Always 级硬人审直接拒绝，不挂 300 秒）。
    /// false = IM 回合跟随桌面全局两旋钮（桌面切了什么档 IM 就是什么档）。
    #[serde(default = "default_true")]
    pub full_access: bool,
    #[serde(default)]
    pub feishu: FeishuCreds,
    #[serde(default)]
    pub qq: QqCreds,
    /// 微信 ClawBot（iLink）凭证：`cmx-agent im-login` 扫码后写入（bot_token 长期复用）。
    #[serde(default)]
    pub wechat: WechatCreds,
    /// chat_id 白名单；**空 = TOFU**：首个发消息的会话锁定为唯一放行会话（进程生命周期内，
    /// 重启后重锁），其余一律拒绝。个人模式下这是唯一的安全门——正式使用务必配置固定白名单。
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

/// QQ 官方机器人开放平台凭证（q.qq.com 管理端「开发设置」页）。
/// 注：Telegram 通道已于 2026-09-11 下线——旧 im.json 里的 `telegram` 字段被 serde
/// 静默忽略，无需迁移。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QqCreds {
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    /// 空 = 默认正式 `https://api.sgroup.qq.com`；沙箱联调填 `https://sandbox.api.sgroup.qq.com`。
    #[serde(default)]
    pub base: String,
}

/// 微信 ClawBot（iLink）凭证：`cmx-agent im-login` 扫码登录后写入，长期复用。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WechatCreds {
    /// 扫码获取的 bot_token（Bearer；有效期未文档化，失效后重登即可）。
    #[serde(default)]
    pub bot_token: String,
    /// 机器人 id（`xxx@im.bot`，面板展示用）。
    #[serde(default)]
    pub bot_id: String,
    /// 登录用户 id（展示用）。
    #[serde(default)]
    pub user_id: String,
    /// 空 = 默认 `https://ilinkai.weixin.qq.com`（登录确认后会下发实际 baseurl）。
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
            active: Vec::new(),
            personal: true,
            full_access: true,
            feishu: FeishuCreds::default(),
            qq: QqCreds::default(),
            wechat: WechatCreds::default(),
            allow: Vec::new(),
        }
    }
}

impl ImRemoconConfig {
    /// 当前生效的 provider 列表（保序去重）：`active` 非空用之；空回落单选 `kind`
    /// （旧版文件兼容）。返回空 = 启用态下没有任何可用通道（resolve 报错提示）。
    pub fn active(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let push = |k: &str, out: &mut Vec<String>| {
            let k = k.trim();
            if !k.is_empty() && parse_kind(k).is_ok() && !out.iter().any(|e| e == k) {
                out.push(k.to_string());
            }
        };
        for k in &self.active {
            push(k, &mut out);
        }
        if out.is_empty() {
            push(&self.kind, &mut out);
        }
        out
    }

    /// 脱敏视图（面板 get 回显）：secret/token 只给掩码，明文不出后端。
    pub fn masked(&self) -> Value {
        json!({
            "configured": true,
            "enabled": self.enabled,
            "kind": self.kind,
            "active": self.active(),
            "personal": self.personal,
            "full_access": self.full_access,
            "app_id": self.feishu.app_id,
            "app_secret_masked": mask_secret(&self.feishu.app_secret),
            "base": self.feishu.base,
            "qq_app_id": self.qq.app_id,
            "qq_secret_masked": mask_secret(&self.qq.app_secret),
            "qq_base": self.qq.base,
            "wechat_bot_id": self.wechat.bot_id,
            "wechat_token_masked": mask_secret(&self.wechat.bot_token),
            "wechat_base": self.wechat.base,
            "allow": self.allow.join(", "),
        })
    }

    /// 单个 provider（按 kind 标签）→ 运行装配件。凭证缺失返回 Err（带缺哪个）。
    fn resolve_one(&self, kind: &str) -> Result<(ImKind, Arc<dyn ImProvider>), String> {
        let kind = parse_kind(kind).map_err(|e| format!("im.json {e}"))?;
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
            ImKind::Qq => {
                let q = &self.qq;
                if q.app_id.trim().is_empty() {
                    return Err("im.json 已选 qq 但缺 AppID".into());
                }
                if q.app_secret.trim().is_empty() {
                    return Err("im.json 已选 qq 但缺 AppSecret".into());
                }
                Arc::new(QqProvider::new(q.app_id.trim(), q.app_secret.trim(), non_empty(&q.base)))
            }
            ImKind::Wechat => {
                let w = &self.wechat;
                if w.bot_token.trim().is_empty() {
                    return Err(
                        "im.json 已选 wechat 但未扫码登录（运行 cmx-agent im-login 获取 bot_token）".into(),
                    );
                }
                Arc::new(WechatProvider::new(w.bot_token.trim(), non_empty(&w.base)))
            }
        };
        Ok((kind, provider))
    }
}

/// 壳启动装好的 IM 遥控：**多个** provider（多通道同时在线）+ 白名单 + 来源标签（日志/面板用）。
pub struct ResolvedIm {
    /// 每个启用通道一项（保序 = im.json `active` 顺序）。空 = 无可用通道（调用方按未启用处理）。
    pub channels: Vec<ResolvedChannel>,
    /// `None` = 不限（个人模式建议配白名单；绑定模式门 = 已绑定身份）。
    pub allow: Option<HashSet<String>>,
    /// 个人模式（im.json 来源读 `personal`；env 来源恒 false = 绑定模式，env 语义不变）。
    pub personal: bool,
    /// 无人值守全权（im.json 来源读 `full_access`，默认 true；env 来源恒 true）：IM 回合以
    /// 回合级覆盖档 [`cmx_agent_core::TurnPolicyOverride::FULL_ACCESS`] 执行——沙箱完全放行、
    /// 从不打断（不弹审批卡）。false = IM 回合跟随桌面全局两旋钮（与桌面会话同档）。
    pub full_access: bool,
    /// `"env"`（开发联调）或 `"im.json"`（GUI 面板）。
    pub source: &'static str,
}

/// 一个启用的 IM 通道。
pub struct ResolvedChannel {
    pub kind: ImKind,
    pub provider: Arc<dyn ImProvider>,
}

impl ResolvedIm {
    /// 兼容单通道语义的便捷读法：首个通道（env 来源恒单通道）。
    pub fn first(&self) -> Option<&ResolvedChannel> {
        self.channels.first()
    }
}

/// 测试用：unwrap_err 需要 T: Debug（provider 是 dyn trait 不实现，手动补最小实现）。
impl std::fmt::Debug for ResolvedIm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kinds: Vec<&str> = self.channels.iter().map(|c| c.kind.label()).collect();
        f.debug_struct("ResolvedIm")
            .field("channels", &kinds)
            .field("allow", &self.allow)
            .field("personal", &self.personal)
            .field("source", &self.source)
            .finish()
    }
}

/// 壳启动装配：**env 优先**（开发联调覆盖，env 语义不变：恒单通道），回落 im.json（GUI，
/// 多通道）。两者皆无 → Err（壳跳过 IM）。
///
/// env 路径沿用 [`ImConfig::from_env`]（含白名单校验）；注意 env 只配凭证不配白名单时会落到
/// im.json 分支——开发联调请同时配 `CMX_AGENT_IM_ALLOW` 或 `CMX_AGENT_IM_NO_ALLOW=1`。
pub fn resolve(data_dir: Option<&Path>) -> Result<ResolvedIm, String> {
    // 1) env（开发联调覆盖）
    if let Ok(cfg) = ImConfig::from_env()
        && let Ok(provider) = cfg.build_provider()
    {
        return Ok(ResolvedIm {
            channels: vec![ResolvedChannel { kind: cfg.kind, provider }],
            allow: cfg.allow,
            personal: false,
            full_access: true,
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
        let allow = if cfg.allow.is_empty() {
            None
        } else {
            Some(cfg.allow.iter().cloned().collect::<HashSet<String>>())
        };
        // 逐通道装配：凭证齐备的通道启用；有通道配了一半（启用了但缺凭证）→ 报错直说缺哪个
        // （fail-closed：宁可不启动任何通道，不让用户误以为全部在线）。全部未配齐 → 报首个错。
        let mut channels = Vec::new();
        let mut first_err: Option<String> = None;
        for k in cfg.active() {
            match cfg.resolve_one(&k) {
                Ok((kind, provider)) => channels.push(ResolvedChannel { kind, provider }),
                Err(e) => {
                    first_err = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = first_err {
            return Err(e);
        }
        if channels.is_empty() {
            return Err("im.json 未启用任何 IM 通道（请到 设置 → IM 遥控 勾选并填凭证）".into());
        }
        return Ok(ResolvedIm { channels, allow, personal: cfg.personal, full_access: cfg.full_access, source: "im.json" });
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

/// QQ 凭证连通性预检（面板「测试连接」备用，与 `test_feishu` 同地位）：getAppAccessToken 一跳。
/// token 端点固定 `api.bot.qq.com`（官方统一鉴权域，不随沙箱 base 走）。
pub async fn test_qq(app_id: &str, app_secret: &str, base: Option<&str>) -> Result<String, String> {
    if app_id.trim().is_empty() || app_secret.trim().is_empty() {
        return Err("AppID / AppSecret 不能为空".into());
    }
    let api_base = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or("https://api.sgroup.qq.com")
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))?;

    // 1) getAppAccessToken：凭证对不对。
    let v: Value = client
        .post("https://api.bot.qq.com/app/getAppAccessToken")
        .json(&json!({ "appId": app_id.trim(), "clientSecret": app_secret.trim() }))
        .send()
        .await
        .map_err(|e| format!("请求失败（网络不通）：{e}"))?
        .json()
        .await
        .map_err(|e| format!("响应解析失败：{e}"))?;
    let token = v.get("access_token").and_then(|t| t.as_str()).unwrap_or("");
    if token.is_empty() {
        return Err(format!("凭证无效：{v}"));
    }

    // 2) gateway：API 基址可达 + 机器人存在（沙箱地址配错在此暴露）。
    let resp = client
        .get(format!("{api_base}/gateway"))
        .header("Authorization", format!("QQBot {token}"))
        .send()
        .await
        .map_err(|e| format!("gateway 请求失败（检查 API 地址，沙箱为 sandbox.api.sgroup.qq.com）：{e}"))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("gateway HTTP {status}: {body}"));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("gateway 解析失败：{e}"))?;
    if v.get("url").and_then(|u| u.as_str()).is_none() {
        return Err("gateway 响应缺 url".into());
    }
    Ok("✓ 凭证有效，QQ 网关可用".into())
}

/// 微信 ClawBot 凭证连通性预检（面板「测试连接」备用，与 `test_feishu`/`test_qq` 同地位）：
/// 空转一次 `getupdates`（3s 超时、空游标）——token 对不对当场暴露，不消费消息。
pub async fn test_wechat(bot_token: &str, base: Option<&str>) -> Result<String, String> {
    if bot_token.trim().is_empty() {
        return Err("尚未扫码登录：先运行 `cmx-agent im-login` 获取微信 bot_token".into());
    }
    let base = base
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .unwrap_or(WECHAT_BASE_DEFAULT)
        .trim_end_matches('/')
        .to_string();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .build()
        .map_err(|e| format!("构建 HTTP 客户端失败：{e}"))?;
    let body = json!({ "get_updates_buf": "", "base_info": base_info() });
    let rb = apply_auth(apply_common(client.post(format!("{base}/ilink/bot/getupdates"))), bot_token.trim());
    let resp = rb
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("请求失败（网络不通或地址错）：{e}"))?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("bot_token 已失效（401）：请重新运行 `cmx-agent im-login` 扫码".into());
    }
    if !resp.status().is_success() {
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        return Err(format!("getupdates HTTP {status}: {text}"));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("响应解析失败：{e}"))?;
    // 判读对齐官方 v2.4.8：ret/errcode 存在且 ≠0 才是错误（缺失 = 成功）。
    match classify_response(&v) {
        ResponseVerdict::Ok => Ok("✓ bot_token 有效，iLink 长轮询通道可用".into()),
        ResponseVerdict::StaleToken => {
            Err("iLink 返回 -14（bot_token 已失效）：请重新运行 `cmx-agent im-login` 扫码".into())
        }
        ResponseVerdict::Failed { code, errmsg } => {
            Err(format!("iLink 返回 code={code}（msg={errmsg}）"))
        }
    }
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
            kind: "qq".into(),
            active: vec!["qq".into(), "feishu".into()],
            personal: true,
            full_access: false, // 回程断言默认序列化/读回无损（显式 false 更有区分度）
            feishu: FeishuCreds {
                app_id: "cli_x".into(),
                app_secret: "sec-secret-secret".into(),
                base: String::new(),
            },
            qq: QqCreds { app_id: "10xx".into(), app_secret: "qq-secret".into(), base: String::new() },
            wechat: WechatCreds {
                bot_token: "wx-token-secret".into(),
                bot_id: "bot@im.bot".into(),
                user_id: "u9".into(),
                base: String::new(),
            },
            allow: vec!["oc_a".into(), "oc_b".into()],
        };
        save_im_config(&dir, &cfg).unwrap();
        let back = load_im_config(&dir).expect("应能读回");
        assert_eq!(back.feishu.app_id, "cli_x");
        assert_eq!(back.feishu.app_secret, "sec-secret-secret");
        assert_eq!(back.qq.app_id, "10xx");
        assert_eq!(back.qq.app_secret, "qq-secret");
        assert_eq!(back.wechat.bot_token, "wx-token-secret");
        assert_eq!(back.wechat.bot_id, "bot@im.bot");
        assert_eq!(back.kind, "qq");
        assert_eq!(back.active(), vec!["qq".to_string(), "feishu".to_string()]);
        assert_eq!(back.allow, vec!["oc_a", "oc_b"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn active_falls_back_to_kind_for_legacy_files() {
        // 旧版单选文件（无 active）→ 回落 kind；去重 + 未知值过滤。
        let legacy = ImRemoconConfig { kind: "qq".into(), ..Default::default() };
        assert_eq!(legacy.active(), vec!["qq".to_string()]);

        let dup = ImRemoconConfig {
            kind: "feishu".into(),
            active: vec!["feishu".into(), "bogus".into(), "feishu ".into()],
            ..Default::default()
        };
        assert_eq!(dup.active(), vec!["feishu".to_string()]);
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
            qq: QqCreds { app_secret: "1234567890abcd".into(), ..Default::default() },
            ..Default::default()
        };
        let m = cfg.masked();
        assert_eq!(m["app_secret_masked"], ""); // 未配置 → 空串
        assert_eq!(m["qq_secret_masked"], "...abcd");
        assert!(!m.to_string().contains("1234567890abcd"));
    }

    #[test]
    fn resolve_one_validates_creds() {
        // feishu 缺 secret → Err 带字段名
        let bad = ImRemoconConfig { kind: "feishu".into(), ..Default::default() };
        let err = match bad.resolve_one("feishu") {
            Err(e) => e,
            Ok(_) => panic!("缺凭证应 Err"),
        };
        assert!(err.contains("App ID"), "{err}");

        // 齐备 → Ok
        let ok = ImRemoconConfig {
            kind: "feishu".into(),
            feishu: FeishuCreds { app_id: "cli_1".into(), app_secret: "s".into(), base: String::new() },
            ..Default::default()
        };
        let (kind, _) = ok.resolve_one("feishu").unwrap();
        assert_eq!(kind, ImKind::Feishu);

        // 旧 kind（telegram，已下线）→ parse_kind Err，被 active() 过滤不炸
        let legacy = ImRemoconConfig { kind: "telegram".into(), ..Default::default() };
        assert!(
            legacy.resolve_one("telegram").is_err() && legacy.active().is_empty(),
            "已下线 kind 应被拒/过滤"
        );

        // qq 缺 AppSecret → Err 带字段名
        let bad_qq = ImRemoconConfig {
            kind: "qq".into(),
            qq: QqCreds { app_id: "10xx".into(), ..Default::default() },
            ..Default::default()
        };
        let err = match bad_qq.resolve_one("qq") {
            Err(e) => e,
            Ok(_) => panic!("缺凭证应 Err"),
        };
        assert!(err.contains("AppSecret"), "{err}");

        // qq 齐备 → Ok
        let ok_qq = ImRemoconConfig {
            kind: "qq".into(),
            qq: QqCreds { app_id: "10xx".into(), app_secret: "s".into(), base: String::new() },
            ..Default::default()
        };
        let (kind, _) = ok_qq.resolve_one("qq").unwrap();
        assert_eq!(kind, ImKind::Qq);

        // wechat 未扫码 → Err 带指引
        let bad_wx = ImRemoconConfig { kind: "wechat".into(), ..Default::default() };
        let err = match bad_wx.resolve_one("wechat") {
            Err(e) => e,
            Ok(_) => panic!("未扫码应 Err"),
        };
        assert!(err.contains("im-login"), "{err}");

        // wechat 齐备 → Ok
        let ok_wx = ImRemoconConfig {
            kind: "wechat".into(),
            wechat: WechatCreds { bot_token: "t".into(), ..Default::default() },
            ..Default::default()
        };
        let (kind, _) = ok_wx.resolve_one("wechat").unwrap();
        assert_eq!(kind, ImKind::Wechat);
    }

    #[test]
    fn masked_hides_wechat_token() {
        let cfg = ImRemoconConfig {
            wechat: WechatCreds {
                bot_token: "wx-secret-token-9999".into(),
                bot_id: "bot@im.bot".into(),
                ..Default::default()
            },
            ..Default::default()
        };
        let m = cfg.masked();
        assert_eq!(m["wechat_token_masked"], "...9999");
        assert_eq!(m["wechat_bot_id"], "bot@im.bot");
        assert!(!m.to_string().contains("wx-secret-token-9999"));
    }

    #[tokio::test]
    async fn test_wechat_rejects_empty_token() {
        // 纯参数校验（不触网）：空 token 直接 Err 带指引。
        let err = test_wechat("", None).await.expect_err("空 token 应 Err");
        assert!(err.contains("im-login"), "{err}");
    }

    #[test]
    fn resolve_im_json_multi_channel() {
        // 多通道：飞书 + QQ 凭证齐备、active 双选 → 两个通道都装上。
        let dir = tmp_dir("multi");
        std::fs::write(
            im_config_path(&dir),
            r#"{"kind":"qq","active":["qq","feishu"],"feishu":{"app_id":"cli_1","app_secret":"s"},"qq":{"app_id":"10xx","app_secret":"s"}}"#,
        )
        .unwrap();
        let r = resolve(Some(&dir)).unwrap();
        assert_eq!(r.channels.len(), 2);
        assert_eq!(r.channels[0].kind, ImKind::Qq); // 保序 = active 顺序
        assert_eq!(r.channels[1].kind, ImKind::Feishu);
        assert!(r.allow.is_none());
        assert!(r.personal);
        std::fs::remove_dir_all(&dir).ok();

        // 旧版单选文件（无 active）：回落 kind，仍单通道可用。
        let dir = tmp_dir("legacy");
        std::fs::write(
            im_config_path(&dir),
            r#"{"kind":"feishu","feishu":{"app_id":"cli_1","app_secret":"s"}}"#,
        )
        .unwrap();
        let r = resolve(Some(&dir)).unwrap();
        assert_eq!(r.channels.len(), 1);
        assert_eq!(r.channels[0].kind, ImKind::Feishu);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_im_json_fails_closed_on_partial_creds() {
        // 有通道配了一半（QQ 缺 AppSecret）→ 整体 Err 直说缺哪个，不静默半启用。
        let dir = tmp_dir("partial-creds");
        std::fs::write(
            im_config_path(&dir),
            r#"{"active":["qq","feishu"],"feishu":{"app_id":"cli_1","app_secret":"s"},"qq":{"app_id":"10xx"}}"#,
        )
        .unwrap();
        let err = resolve(Some(&dir)).unwrap_err();
        assert!(err.contains("AppSecret"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_im_json_no_channel_is_err() {
        // 启用但没勾任何通道（active 空 + kind 空）→ Err 提示去面板勾选。
        let dir = tmp_dir("no-channel");
        std::fs::write(im_config_path(&dir), r#"{"kind":""}"#).unwrap();
        let err = resolve(Some(&dir)).unwrap_err();
        assert!(err.contains("未启用任何 IM 通道"), "{err}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resolve_allow_listed() {
        let dir = tmp_dir("allow");
        std::fs::write(
            im_config_path(&dir),
            r#"{"kind":"feishu","feishu":{"app_id":"i","app_secret":"s"},"allow":["oc_a"]}"#,
        )
        .unwrap();
        let r = resolve(Some(&dir)).unwrap();
        let allow = r.allow.expect("应返回白名单");
        assert!(allow.contains("oc_a"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn mask_secret_shapes() {
        assert_eq!(mask_secret(""), "");
        assert_eq!(mask_secret("1234"), "****");
        assert_eq!(mask_secret("12345678"), "********");
        assert_eq!(mask_secret("123456789"), "...6789");
    }
}
