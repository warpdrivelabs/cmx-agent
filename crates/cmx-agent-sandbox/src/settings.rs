//! 沙箱应用级配置（方案 §6.3）：**独立于 core `Policy`**（回合旋钮）的持久化配置轴。
//!
//! - 落点：`<data_dir>/settings.json`（与 model.json 同级；app 启动时 [`init`] 读入，前门 set 命令写回）。
//! - 未初始化（单测/CLI 无 data_dir）时用默认值，纯内存，不落盘。
//! - 持久化必要性（红队 C-8）：`set_policy` 是纯内存旋钮——若 sandbox 配置也走它，
//!   用户为老内核切 `require_os=false` 后**重启即回 true**，WorkspaceWrite 下 shell 全拒（砖化）。

use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, Serialize};

/// 子进程出站网络档位（只管 shell 类子进程；in-process net 工具走工具闸+SSRF 白名单，见方案 §3.4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NetMode {
    /// 默认：不碰网络（npm install / cargo fetch 等工作流不受影响）。
    #[default]
    Open,
    /// advisory：注入黑洞代理环境变量（node 内置 fetch/Java/.NET 默认无视——失效名单见方案 §3.4）。
    Poison,
    /// Linux seccomp 硬断网（含 io_uring 旁路封堵）；Windows 配置即拒（平台不支持，诚实报错）。
    Enforce,
}

impl NetMode {
    pub fn as_str(self) -> &'static str {
        match self {
            NetMode::Open => "open",
            NetMode::Poison => "poison",
            NetMode::Enforce => "enforce",
        }
    }
}

/// Windows 包管理器缓存写入策略（方案 §4.3：write-restricted 下用户 SID 的缓存授权不算数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum WinCachePolicy {
    /// 默认：env 重定向进工作区 `.agent-cache/`（NPM_CONFIG_CACHE / PIP_CACHE_DIR / CARGO_HOME）。
    /// 零放行、零供应链残余面；首次重下载一次的代价如实接受。
    #[default]
    Redirect,
    /// 显式放行缓存目录（供应链投毒残余面，须配合 extra_write_roots）。
    Allow,
    /// 不做任何缓存处理（npm/pip/cargo 缓存写入将被受限令牌拒绝，工具自行报错）。
    Deny,
}

/// 沙箱应用级配置全集。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct SandboxSettings {
    /// 子进程出站网络档（默认 open，已拍板）。
    pub net: NetMode,
    /// fail-closed 旋钮（默认 true，已拍板）：沙箱不可用时拒绝执行而非裸跑；false=降级+黄标。
    pub require_os: bool,
    /// 命令级风险屏（S3 advisory 层，默认开；只升审批不硬拒）。
    pub cmd_risk_screen: bool,
    /// Windows 追加可写目录（如包缓存目录；相对路径按工作区根解释）。
    pub extra_write_roots: Vec<PathBuf>,
    /// Windows 包管理器缓存策略（默认 redirect）。
    pub win_cache_policy: WinCachePolicy,
    /// deny-read 加固（默认关；副作用：主进程读同受限——见方案 §4.8）。
    /// v1.2 预留字段：本批次未实现 deny-read ACE，置 true 无效（如实声明，不假装生效）。
    pub hardened_read: bool,
}

impl Default for SandboxSettings {
    fn default() -> Self {
        Self {
            net: NetMode::Open,
            require_os: true,
            cmd_risk_screen: true,
            extra_write_roots: Vec::new(),
            win_cache_policy: WinCachePolicy::Redirect,
            hardened_read: false,
        }
    }
}

/// 进程内共享内核：settings + 数据根 + SANDBOX_SID 对（shell/git 两枚，见 [`SandboxSidPair`]）。
struct SettingsCore {
    settings: SandboxSettings,
    data_dir: Option<PathBuf>,
    /// 已生成/加载的 SANDBOX_SID 对。None = 尚未生成。
    sids: Option<SandboxSidPair>,
}

/// **双 SANDBOX_SID**（已拍板 §十-3「两套令牌 profile」的机制基础）：
/// - `shell`：shell/run_tests 令牌的 restricting SID——`.git` 的 deny ACE 只对它挂；
/// - `git`：git 工具令牌的 restricting SID——`.git` 无 deny，可合法写 index.lock/objects；
///   根与 %TEMP% 的 allow ACE 同时授两枚 SID；deny 只挂 shell SID → git 令牌不受 .git deny 影响；
///   两枚均安装期生成一次并持久化（避免 per-session 换 SID 导致全树 ACL 重传播 + ACE 无限累积）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SandboxSidPair {
    pub shell: String,
    pub git: String,
}

static CORE: OnceLock<Arc<RwLock<SettingsCore>>> = OnceLock::new();

fn core() -> Arc<RwLock<SettingsCore>> {
    CORE.get_or_init(|| {
        Arc::new(RwLock::new(SettingsCore { settings: SandboxSettings::default(), data_dir: None, sids: None }))
    })
    .clone()
}

const SETTINGS_FILE: &str = "settings.json";
const SID_FILE: &str = "sandbox_sid.json";

/// app 启动装配时调用（builder）：绑定数据根并读入持久化配置 + SANDBOX_SID 对。幂等。data_dir=None=纯内存。
pub fn init(data_dir: Option<PathBuf>) {
    let lock = core();
    let mut c = lock.write().expect("sandbox settings lock");
    c.data_dir = data_dir.clone();
    if let Some(dir) = &data_dir {
        // 读 settings.json（坏文件容忍：默认值 + 记录，不炸——对齐插件清单"坏清单跳过"哲学）。
        let p = dir.join(SETTINGS_FILE);
        if let Ok(text) = std::fs::read_to_string(&p) {
            match serde_json::from_str::<SandboxSettings>(&text) {
                Ok(s) => c.settings = s,
                Err(e) => eprintln!("[sandbox] settings.json 解析失败，用默认值：{e}"),
            }
        }
        // 读 SANDBOX_SID 对（安装期生成一次并持久化——方案 §4.4）。
        let sp = dir.join(SID_FILE);
        if let Ok(text) = std::fs::read_to_string(&sp)
            && let Ok(pair) = serde_json::from_str::<SandboxSidPair>(&text)
        {
            c.sids = Some(pair);
        }
    }
}

/// 当前配置快照。
pub fn get() -> SandboxSettings {
    core().read().expect("sandbox settings lock").settings.clone()
}

/// 前门 set 命令：整体替换并持久化。返回 `persisted`（data_dir 未绑定时仅内存生效=false，
/// 前门须如实回 `persisted:false` 而非假报已存——红队3 P2-4）。
pub fn set(new: SandboxSettings) -> Result<bool, String> {
    let lock = core();
    let mut c = lock.write().expect("sandbox settings lock");
    c.settings = new;
    let Some(dir) = c.data_dir.clone() else {
        return Ok(false); // 无数据根：内存生效（测试/无头）
    };
    let p = dir.join(SETTINGS_FILE);
    let text = serde_json::to_string_pretty(&c.settings).map_err(|e| e.to_string())?;
    std::fs::write(&p, text).map_err(|e| format!("写 {}: {e}", p.display()))?;
    Ok(true)
}

/// SANDBOX_SID 对：取（已持久化的）或生成（首次生成后立即持久化）。无 data_dir 时生成进程内临时对。
pub fn sid_pair() -> SandboxSidPair {
    let lock = core();
    let mut c = lock.write().expect("sandbox settings lock");
    if let Some(p) = &c.sids {
        let pair = p.clone();
        // 缓存命中≠磁盘有档：sids 可能在 data_dir 绑定前生成（如探针先跑）——补持久化。
        if let Some(dir) = c.data_dir.clone() {
            let sp = dir.join(SID_FILE);
            if !sp.exists()
                && let Ok(text) = serde_json::to_string(&pair)
                && std::fs::write(&sp, &text).is_err()
            {
                eprintln!("[sandbox] SANDBOX_SID 补持久化失败（{}），进程内继续使用", sp.display());
            }
        }
        return pair;
    }
    // 随机性：时间纳秒 + PID + 计数器。只需不与真实 SID/既有 ACE 冲突，无密码学要求。
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = u128::from(std::process::id());
    let ctr = u128::from(SID_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let pair = SandboxSidPair {
        shell: format!("S-1-5-{}-{}-{}", (pid ^ (nanos & 0xffff_ffff)) as u32, ((nanos >> 32) & 0xffff_ffff) as u32, ctr as u32),
        git: format!("S-1-5-{}-{}-{}", ((pid << 16) ^ ((nanos >> 16) & 0xffff_ffff)) as u32, ((nanos >> 48) & 0xffff_ffff) as u32, (ctr + 1) as u32),
    };
    if let Some(dir) = &c.data_dir
        && let Ok(text) = serde_json::to_string(&pair)
        && std::fs::write(dir.join(SID_FILE), &text).is_err()
    {
        eprintln!(
            "[sandbox] SANDBOX_SID 持久化失败（{}），进程内继续使用",
            dir.join(SID_FILE).display()
        );
    }
    c.sids = Some(pair.clone());
    pair
}

static SID_COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_board_decisions() {
        let _g = crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let s = SandboxSettings::default();
        assert_eq!(s.net, NetMode::Open); // 拍板-1
        assert!(s.require_os); // 拍板-2：fail-closed 字面义
        assert!(s.cmd_risk_screen); // S3 默认开
        assert_eq!(s.win_cache_policy, WinCachePolicy::Redirect); // §4.3 默认 env 重定向
        assert!(!s.hardened_read); // §4.8 默认关
    }

    #[test]
    fn settings_roundtrip_and_sid_persistence() {
        let _g = crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("cmx-sbx-set-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init(Some(dir.clone()));
        let mut s = get();
        s.net = NetMode::Poison;
        s.require_os = false;
        set(s).unwrap();

        // 新"进程"视角：重新 init 同一目录应读回持久化值。
        init(Some(dir.clone()));
        let s2 = get();
        assert_eq!(s2.net, NetMode::Poison);
        assert!(!s2.require_os);

        let sid = sid_pair();
        assert!(sid.shell.starts_with("S-1-5-"), "{:?}", sid);
        assert_ne!(sid.shell, sid.git, "两枚 profile SID 必须不同");
        // 再次取应稳定（持久化后不换）。
        assert_eq!(sid_pair().shell, sid.shell);
        assert!(dir.join(SID_FILE).exists());

        // 复原默认，避免影响同进程其它测试。
        set(SandboxSettings::default()).unwrap();
        init(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uninit_uses_defaults_in_memory() {
        let _g = crate::TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        init(None);
        assert_eq!(get(), SandboxSettings::default());
        let sid = sid_pair();
        assert_eq!(sid_pair().shell, sid.shell, "无 data_dir 时进程内稳定");
    }
}
