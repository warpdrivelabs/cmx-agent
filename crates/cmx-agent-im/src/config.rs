//! IM 遥控配置：把 provider 选择、白名单、各 provider 凭证从 env 收口到一处。
//!
//! 范式对齐 [`cmx_agent_model::ModelProviderConfig`]：`from_env()` 解析，`None`/`Err` 表未配置/缺项。
//! 解析出的 [`ImConfig`] 经 CLI 装配成 `Arc<dyn ImProvider>` + 白名单，喂给 [`crate::ImBridge`]。
//!
//! 环境变量（IM 前缀 `CMX_AGENT_IM_*`）：
//! - `CMX_AGENT_IM_KIND`：provider 类型，默认 `telegram`；可选 `feishu`（企业微信/钉钉后续追加）。
//! - `CMX_AGENT_IM_ALLOW`：逗号分隔的 chat_id 白名单（安全必需；CLI 无白名单拒启动）。
//! - `CMX_AGENT_IM_NO_ALLOW`：设 `1` 显式放开白名单（仅测试/纯内网；生产勿用）。
//! - Telegram：`CMX_AGENT_IM_TOKEN`（必需）+ 可选 `CMX_AGENT_IM_BASE`。
//! - 飞书：`CMX_AGENT_IM_FEISHU_APP_ID` + `CMX_AGENT_IM_FEISHU_APP_SECRET`（必需）
//!   + 可选 `CMX_AGENT_IM_FEISHU_BASE`（默认 `https://open.feishu.cn`，海外用 `https://open.larksuite.com`）。

use std::collections::HashSet;

use crate::{FeishuProvider, ImProvider, TelegramProvider};

/// IM provider 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImKind {
    Telegram,
    Feishu,
}

impl ImKind {
    /// 小写标签，用于会话命名前缀 `im-<label>-<chat>` 与日志。
    pub fn label(&self) -> &'static str {
        match self {
            Self::Telegram => "telegram",
            Self::Feishu => "feishu",
        }
    }
}

impl ImKind {
    /// 解析 `CMX_AGENT_IM_KIND`（默认 `telegram`）。未知值返回 `Err`（带可用列表）。
    pub fn from_env() -> Result<Self, String> {
        let raw = std::env::var("CMX_AGENT_IM_KIND").unwrap_or_else(|_| "telegram".into());
        parse_kind(&raw)
    }
}

/// 纯函数：解析 kind 原始值（trim；空 → telegram）。`from_env` 与测试共用，避免 env 写入。
pub fn parse_kind(raw: &str) -> Result<ImKind, String> {
    match raw.trim() {
        "" | "telegram" => Ok(ImKind::Telegram),
        "feishu" => Ok(ImKind::Feishu),
        other => Err(format!("未知 CMX_AGENT_IM_KIND: {other}（可选 telegram / feishu）")),
    }
}

/// IM 遥控装配配置：provider 类型 + 白名单。
#[derive(Debug, Clone)]
pub struct ImConfig {
    pub kind: ImKind,
    /// `Some(set)` = 白名单；`None` = 开放（仅测试/纯内网，由 `CMX_AGENT_IM_NO_ALLOW=1` 显式开启）。
    pub allow: Option<HashSet<String>>,
}

impl ImConfig {
    /// 从 env 解析装配配置。
    ///
    /// - `allow`：`CMX_AGENT_IM_ALLOW` 非空 → `Some(set)`；
    ///   `CMX_AGENT_IM_NO_ALLOW=1` → `None`（开放，仅测试/内网）；两者都未设 → `Err`（拒裸奔）。
    /// - `kind`：见 [`ImKind::from_env`]。
    pub fn from_env() -> Result<Self, String> {
        let kind = ImKind::from_env()?;
        let allow = parse_allow()?;
        Ok(Self { kind, allow })
    }

    /// 按配置装配 provider（读各自凭证 env；缺凭证返回 `Err`，带缺哪个）。
    pub fn build_provider(&self) -> Result<std::sync::Arc<dyn ImProvider>, String> {
        let p: std::sync::Arc<dyn ImProvider> = match self.kind {
            ImKind::Telegram => {
                let p = TelegramProvider::from_env()
                    .ok_or_else(|| "缺 CMX_AGENT_IM_TOKEN（Telegram bot token）".to_string())?;
                std::sync::Arc::new(p)
            }
            ImKind::Feishu => {
                let p = FeishuProvider::from_env().ok_or_else(|| {
                    "缺 CMX_AGENT_IM_FEISHU_APP_ID / CMX_AGENT_IM_FEISHU_APP_SECRET".to_string()
                })?;
                std::sync::Arc::new(p)
            }
        };
        Ok(p)
    }
}

/// 解析白名单 env。返回值语义见 [`ImConfig::from_env`]。
pub fn parse_allow() -> Result<Option<HashSet<String>>, String> {
    let raw = std::env::var("CMX_AGENT_IM_ALLOW").unwrap_or_default();
    let set = parse_allow_str(&raw);
    if !set.is_empty() {
        return Ok(Some(set));
    }
    // 显式放开（仅测试/纯内网）：CMX_AGENT_IM_NO_ALLOW=1
    let no_allow = std::env::var("CMX_AGENT_IM_NO_ALLOW")
        .ok()
        .map(|v| v.trim() == "1" || v.trim().eq_ignore_ascii_case("true"));
    if no_allow == Some(true) {
        Ok(None)
    } else {
        Err(
            "IM 遥控需白名单：设 CMX_AGENT_IM_ALLOW=逗号分隔的 chat_id（避免任何人驱动你的 agent）"
                .into(),
        )
    }
}

/// 纯函数：逗号分隔 → 去空格 → 过滤空。`parse_allow` 与测试共用。
pub fn parse_allow_str(raw: &str) -> HashSet<String> {
    raw.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_str_parses_comma_list() {
        let s = parse_allow_str("oc_1, oc_2 ,oc_3");
        assert_eq!(s.len(), 3);
        assert!(s.contains("oc_1") && s.contains("oc_2") && s.contains("oc_3"));
    }

    #[test]
    fn allow_str_empty_is_empty() {
        assert!(parse_allow_str("").is_empty());
        assert!(parse_allow_str(" , , ").is_empty());
    }

    #[test]
    fn kind_parses_values() {
        assert_eq!(parse_kind("").unwrap(), ImKind::Telegram);
        assert_eq!(parse_kind("telegram").unwrap(), ImKind::Telegram);
        assert_eq!(parse_kind("feishu").unwrap(), ImKind::Feishu);
        assert_eq!(parse_kind("  feishu  ").unwrap(), ImKind::Feishu);
        assert!(parse_kind("bogus").is_err());
    }
}
