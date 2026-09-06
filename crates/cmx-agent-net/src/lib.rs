//! cmx-agent-net（U10 联网研究）：`web_fetch` 抓网页取正文 + `web_search` 网络搜索。
//!
//! 重依赖 reqwest 隔离在本 crate；对模型只暴露两个「工具」。设计对齐 [`cmx_agent_office`]：
//! HTML→文本用手写扫描（不引重解析库），搜索结果解析同理。
//!
//! **SSRF 基线**：`web_fetch` 的 URL 由模型提供（不可信）→ 默认拒绝 loopback/私网/链路本地/元数据地址；
//! `web_search` 的检索端点由运营方配置（可信，env `CMX_AGENT_SEARCH_URL`）→ 不做此拦截。
//! 逃生阀 env `CMX_AGENT_NET_ALLOW_PRIVATE=1`（自托管/测试）放开私网限制。

mod browser;
mod cdp;
mod computer;
mod fetch;
mod html;
mod interact;
mod search;
mod vision;

pub use browser::{BrowserReadTool, BrowserScreenshotTool};
pub use computer::ComputerUseTool;
pub use fetch::WebFetchTool;
pub use interact::BrowserDoTool;
pub use search::WebSearchTool;
pub use vision::VisionCfg;

use std::time::Duration;

/// 构建带超时/UA 的共享 HTTP 客户端。
pub(crate) fn client(timeout_ms: u64) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_millis(timeout_ms))
        .user_agent("Mozilla/5.0 (compatible; cmx-agent/1.0; +https://cmx.local)")
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

/// SSRF 基线防护：仅允许 http/https；`allow_private=false` 时拒绝 loopback/私网/链路本地/元数据主机。
/// 仅拦截「字面量私网 IP + localhost 类主机名」——桌面助手基线，不做 DNS 重绑定深防。
/// `allow_private` 由工具持有（builder 从 env `CMX_AGENT_NET_ALLOW_PRIVATE` 读，自托管/测试可放开）。
pub(crate) fn ensure_public_url(raw: &str, allow_private: bool) -> Result<url::Url, String> {
    let u = url::Url::parse(raw).map_err(|_| format!("非法 URL：{raw}"))?;
    match u.scheme() {
        "http" | "https" => {}
        s => return Err(format!("仅支持 http/https（收到 {s}）")),
    }
    if allow_private {
        return Ok(u);
    }
    // 用 url 的类型化 Host 判定（IPv6 字面量经 host_str 会带 [] 括号，parse 会失败——故走 Host）。
    let bad = match u.host() {
        None => return Err("URL 缺少主机".into()),
        Some(url::Host::Ipv4(a)) => is_private_ip(&std::net::IpAddr::V4(a)),
        Some(url::Host::Ipv6(a)) => is_private_ip(&std::net::IpAddr::V6(a)),
        Some(url::Host::Domain(d)) => {
            let h = d.to_lowercase();
            h == "localhost"
                || h.ends_with(".localhost")
                || h.ends_with(".local")
                || h == "metadata.google.internal"
        }
    };
    if bad {
        return Err(format!("拒绝访问内网地址：{}", u.host_str().unwrap_or("")));
    }
    Ok(u)
}

fn is_private_ip(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(a) => {
            a.is_loopback()
                || a.is_private()
                || a.is_link_local()
                || a.is_unspecified()
                || a.is_broadcast()
                || a.octets()[0] == 0
        }
        std::net::IpAddr::V6(a) => {
            a.is_loopback()
                || a.is_unspecified()
                || (a.segments()[0] & 0xfe00) == 0xfc00 // ULA fc00::/7
                || (a.segments()[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ensure_public_url;

    #[test]
    fn blocks_private_and_loopback() {
        // allow_private=false 时应拦截
        assert!(ensure_public_url("http://localhost/x", false).is_err());
        assert!(ensure_public_url("http://127.0.0.1:8080/", false).is_err());
        assert!(ensure_public_url("http://10.0.0.5/", false).is_err());
        assert!(ensure_public_url("http://192.168.1.1/", false).is_err());
        assert!(ensure_public_url("http://169.254.169.254/latest/meta-data/", false).is_err());
        assert!(ensure_public_url("http://[::1]/", false).is_err());
        assert!(ensure_public_url("ftp://example.com/", false).is_err()); // 非 http(s)
        // allow_private=true 放开私网
        assert!(ensure_public_url("http://127.0.0.1:8080/", true).is_ok());
    }

    #[test]
    fn allows_public() {
        assert!(ensure_public_url("https://example.com/page", false).is_ok());
        assert!(ensure_public_url("http://93.184.216.34/", false).is_ok()); // 公网 IP 字面量
    }
}
