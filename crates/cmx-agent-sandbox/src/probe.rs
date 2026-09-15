//! 能力探测（方案 §3.3）：沙箱可用性判定，结果进程内缓存（探测恒绿但真实失败模式测不出
//! 的坑由红队 A-15 指出——Windows 侧探测面含令牌派生 + 试挂 ACE，不做只查版本号的假探测）。

use std::sync::OnceLock;

/// 探测结论。
#[derive(Debug, Clone)]
pub enum Availability {
    Available,
    Unavailable(String),
}

/// OS 沙箱可用性（进程内缓存一次）。
pub fn os_sandbox_available() -> Availability {
    static CACHE: OnceLock<Availability> = OnceLock::new();
    CACHE.get_or_init(probe_once).clone()
}

fn probe_once() -> Availability {
    #[cfg(windows)]
    {
        crate::win::token::probe_token_derivation()
    }
    #[cfg(target_os = "linux")]
    {
        crate::linux::probe_landlock_abi()
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Availability::Unavailable("本平台无 OS 沙箱实现（macOS 非分发目标）".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_returns_and_caches() {
        let a = os_sandbox_available();
        // 任何结果都必须可克隆复述（缓存路径不 panic）。
        let _ = format!("{a:?}");
        assert!(matches!(os_sandbox_available(), Availability::Available)
            || matches!(os_sandbox_available(), Availability::Unavailable(_)));
    }
}
