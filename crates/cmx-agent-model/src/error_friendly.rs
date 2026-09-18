//! 模型错误的七类分类 + 友好话术（方案 20260917「聊天报错可见性」B 项）。
//!
//! 在模型层就把话说人话（而不是前端映射）：IM 桥复用同一文本，后端成型三端
//! （Web 壳 / Tauri 壳 / 飞书）同时受益；HTTP 状态码等分类信息 Rust 端现成。
//!
//! 产出格式：**友好话术（说用户该干什么）＋ 技术原文保留**——原文供支持人员排障，
//! 原文截断 200 字防 IM 卡片被撑爆。分类与「测试连接」（`test_model_config`）共用。

/// 七类错误（对齐 ZCode `settings.modelProvider.*` 的错误分类；`err_kind` 是稳定枚举值，
/// 前端/测试可据此分支）。stable 值：auth / network / timeout / model_not_found /
/// rate_limit / server / unknown；另有第八类 `context_overflow`（压缩方案 §4.1.5，
/// 上下文超限——溢出救援 `is_context_overflow` 与人话话术共用此分类）。
pub fn friendly_model_error(raw: &str) -> String {
    let kind = classify(raw);
    let friendly = match kind {
        "context_overflow" => {
            "对话内容超出模型上下文窗口。可输入 /compact 压缩历史后重试；若反复出现，请在 设置 → 模型 把该模型的「上下文窗口」改为真实值"
        }
        "auth" => "API Key 无效或没有权限。请到 设置 → 模型 检查 API Key",
        "model_not_found" => {
            "接口地址或模型名不对（404）。请检查 Base URL（一般以 /v1 结尾）和模型 ID 拼写"
        }
        "rate_limit" => "请求太频繁或额度不足（429）。请稍后再试，或检查账户余额",
        "server" => "模型服务暂时不可用（服务端错误）。请稍后再试",
        "timeout" => "模型服务超时无响应。可在 设置 → 模型 里调大超时时间后重试",
        "network" => "连不上模型服务。请检查网络 / VPN，以及 Base URL 是否填对",
        _ => return raw.trim().to_string(), // unknown：原样透传（不丢信息兜底）
    };
    format!("{friendly}（原文：{}）", truncate_raw(raw))
}

/// 小空间场景（测试连接悬浮条等）：只要话术、不带「（原文：…）」（用户：不用展示原文）。
/// unknown 也没有话术可给，退化为截断到 80 字的原文。
pub fn friendly_model_error_brief(raw: &str) -> String {
    match classify(raw) {
        "context_overflow" => "对话超出模型上下文窗口，可 /compact 压缩后重试".into(),
        "auth" => "API Key 无效或没有权限。请到 设置 → 模型 检查 API Key".into(),
        "model_not_found" => "接口地址或模型名不对（404）。请检查 Base URL 与模型 ID 拼写".into(),
        "rate_limit" => "请求太频繁或额度不足（429）。请稍后再试，或检查账户余额".into(),
        "server" => "模型服务暂时不可用（服务端错误）。请稍后再试".into(),
        "timeout" => "模型服务超时无响应。可调大超时时间后重试".into(),
        "network" => "连不上模型服务。请检查网络，以及 Base URL 是否填对".into(),
        _ => {
            let s = raw.trim();
            const MAX: usize = 80;
            if s.chars().count() <= MAX {
                s.to_string()
            } else {
                let t: String = s.chars().take(MAX).collect();
                format!("{t}…")
            }
        }
    }
}

/// 机器可判定的「上下文超限」识别（压缩方案 §4.2.6 溢出救援的触发条件）。
pub fn is_context_overflow(msg: &str) -> bool {
    classify(msg) == "context_overflow"
}

/// 技术原文截断（IM 卡片防撑爆；UI 侧另有 line-clamp 双保险）。
fn truncate_raw(raw: &str) -> String {
    let s = raw.trim();
    const MAX: usize = 200;
    if s.chars().count() <= MAX {
        s.to_string()
    } else {
        let t: String = s.chars().take(MAX).collect();
        format!("{t}…")
    }
}

/// 七类分类启发式：优先认 openai.rs 错误构造点写死的「HTTP {code}」标注，
/// 其次认上游业务文案关键词（HTTP 200 但信封报错，如 invalid api key），
/// 最后认 reqwest 传输错误文案（connect / timeout / dns）。
/// 上下文超限特征（中英文；openai `context_length_exceeded` / anthropic
/// `prompt is too long` / 中文网关文案）。出现在请求体超窗报错里，高度特异。
const OVERFLOW_KEYWORDS: &[&str] = &[
    "context_length_exceeded",
    "maximum context length",
    "context length exceeded",
    "exceeds the context window",
    "context window exceeded",
    "prompt is too long",
    "too many tokens",
    "reduce the length",
    "上下文长度",
    "超出上下文",
    "上下文超限",
    "过长",
];

pub fn classify(raw: &str) -> &'static str {
    let l = raw.to_ascii_lowercase();
    // —— 上下文超限（压缩方案 §4.1.5；最特异，最先判）——
    if OVERFLOW_KEYWORDS.iter().any(|k| l.contains(k)) {
        return "context_overflow";
    }
    // —— HTTP 状态码标注（构造点保证带；前缀匹配 http 401/403/404/429 与 http 5xx）——
    for (pat, kind) in [
        ("http 401", "auth"),
        ("http 403", "auth"),
        ("http 404", "model_not_found"),
        ("http 429", "rate_limit"),
    ] {
        if l.contains(pat) {
            return kind;
        }
    }
    if let Some(rest) = l.find("http 5").map(|i| &l[i + 6..]) {
        // "http 5" 后取状态码首位判断 5xx（http 500..599）
        if rest.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            return "server";
        }
    }
    // —— 上游业务文案关键词（信封错误多为英文；中文网关文案一并覆盖）——
    let auth_kws = [
        "invalid api key",
        "invalid api-key",
        "api key invalid",
        "invalid_api_key",
        "invalid token",
        "unauthorized",
        "forbidden",
        "authentication",
        "鉴权失败",
        "令牌无效",
        "无效令牌",
        "未提供令牌",
        "无效的令牌",
        "认证失败",
    ];
    if auth_kws.iter().any(|k| l.contains(k)) {
        return "auth";
    }
    let nf_kws = [
        "model_not_found",
        "model not found",
        "does not exist",
        "无可用模型",
        "没有可用模型",
        "模型不存在",
        "未知模型",
    ];
    if nf_kws.iter().any(|k| l.contains(k)) {
        return "model_not_found";
    }
    let rl_kws = [
        "rate limit",
        "ratelimit",
        "too many requests",
        "quota",
        "insufficient",
        "限流",
        "请求过于频繁",
        "余额不足",
        "欠费",
        "额度不足",
    ];
    if rl_kws.iter().any(|k| l.contains(k)) {
        return "rate_limit";
    }
    // —— 传输层（reqwest 文案；小写化后匹配）——
    // reqwest 超时错误含 "operation timed out"；自建超时文案含「超时」。
    if l.contains("timed out") || l.contains("timeout") || l.contains("超时") {
        return "timeout";
    }
    let net_kws = [
        // reqwest 发送失败裸文案：实测部分错误串止于 "error sending request for url (…)"，
        // 没有后续 "error trying to connect" tail，缺这条会落 unknown 原样透传（2026-09-17）
        "error sending request",
        "error trying to connect",
        "connection refused",
        "connection reset",
        "connection closed",
        "connect error",
        "dns error",
        "name or service not known",
        "network",
        "os error",
        "broken pipe",
        "tcp connect",
        "tls",
        "证书",
        "网络",
        "连不上",
        "连接失败",
    ];
    if net_kws.iter().any(|k| l.contains(k)) {
        return "network";
    }
    "unknown"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_http_status_annotations() {
        assert_eq!(classify("模型服务返回 HTTP 401"), "auth");
        assert_eq!(classify("模型服务返回 HTTP 403"), "auth");
        assert_eq!(classify("模型服务返回 HTTP 404"), "model_not_found");
        assert_eq!(classify("模型服务返回 HTTP 429"), "rate_limit");
        assert_eq!(classify("模型服务返回 HTTP 500"), "server");
        assert_eq!(classify("模型服务返回 HTTP 503"), "server");
        assert_eq!(classify("模型服务返回 HTTP 400"), "unknown", "4xx 其他类不冒认");
    }

    #[test]
    fn classify_upstream_envelope_keywords() {
        assert_eq!(classify("invalid api key"), "auth");
        assert_eq!(classify("上游模型错误: Invalid Token"), "auth");
        assert_eq!(classify("未提供令牌"), "auth");
        assert_eq!(classify("model mlamp/nope does not exist"), "model_not_found");
        assert_eq!(classify("Too Many Requests"), "rate_limit");
        assert_eq!(classify("余额不足，请充值"), "rate_limit");
    }

    #[test]
    fn classify_transport_errors() {
        // reqwest 连接失败典型文案
        assert_eq!(
            classify("请求模型失败: error sending request for url (http://x/v1/chat/completions): error trying to connect: tcp connect error: os error 10061"),
            "network"
        );
        // 裸发送失败文案（无 connect tail；2026-09-17 真实案例 llmgw 网关断连）
        assert_eq!(
            classify("请求模型失败: error sending request for url (https://llmgw-bz.mlamp.cn/v1/chat/completions)"),
            "network"
        );
        // reqwest 超时典型文案
        assert_eq!(
            classify("请求模型失败: error sending request for url (http://x/v1/chat/completions): operation timed out"),
            "timeout"
        );
        assert_eq!(classify("模型 60 秒内无响应（超时）"), "timeout");
    }

    #[test]
    fn classify_unknown_passes_through() {
        assert_eq!(classify("解析模型响应失败: eof"), "unknown");
        let s = friendly_model_error("解析模型响应失败: eof");
        assert_eq!(s, "解析模型响应失败: eof", "unknown 原样透传不包壳");
    }

    #[test]
    fn friendly_keeps_raw_and_truncates_long() {
        let s = friendly_model_error("模型服务返回 HTTP 401");
        assert!(s.starts_with("API Key 无效或没有权限"), "{s}");
        assert!(s.contains("原文：模型服务返回 HTTP 401"), "{s}");
        let long = format!("无效令牌 {}", "x".repeat(500));
        let s = friendly_model_error(&long);
        assert!(s.chars().count() < 500, "原文应截断防撑爆: {s}");
        assert!(s.ends_with("…）"));
    }

    #[test]
    fn friendly_every_kind_has_actionable_copy() {
        for raw in [
            "HTTP 401 invalid api key",
            "模型服务返回 HTTP 404",
            "模型服务返回 HTTP 429",
            "模型服务返回 HTTP 500",
            "error trying to connect: refused",
            "operation timed out",
        ] {
            let s = friendly_model_error(raw);
            assert!(s.contains("原文："), "每类都应带原文: {s}");
            assert!(s.chars().count() > raw.len(), "友好话术应比原文长（含指引）: {s}");
        }
    }
}
