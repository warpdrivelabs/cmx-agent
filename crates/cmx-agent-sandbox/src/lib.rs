//! cmx-agent OS 级进程沙箱（方案 `documents/plans/20260914_cmx-agent_沙箱OS级隔离完善方案.md` v1.1）。
//!
//! 三层防线中的第三层：策略层（SandboxMode × ApprovalPolicy，core）与应用层围栏（tools/sandbox.rs
//! 路径围栏 + 五层守卫）之上，把「WorkspaceWrite 的承诺」从提示词变成内核/令牌事实：
//!
//! - **Windows（S1a）**：`CreateRestrictedToken(WRITE_RESTRICTED)` 受限令牌 + restricting 集合
//!   {SANDBOX_SID, Logon, Everyone} + Default DACL 改写 + ACL 放行集（allowed_roots + %TEMP%）
//!   + `.git` deny（仅 Shell profile）+ 原生 `CreateProcessAsUserW` spawn（STARTUPINFOEX 属性表：句柄继承、spawn 时挂 Job 消 attach 竞态、lpDesktop 独立桌面）。
//! - **Linux（S2）**：Landlock 逐挂载规则矩阵（ABI 分级：REFER≥2 / TRUNCATE≥3）+ 可选 seccomp
//!   硬断网（enforce 档：拒 INET socket + 封 io_uring + arch 校验）+ preexec 三步序列（NNP→seccomp→restrict_self）。
//! - **fail-closed 铁律**：沙箱建立失败 = 本次工具调用**执行段失败**（走 ToolResult::err 通道，
//!   模型可见、SessionLog 落库）——不是守卫 Deny（时序上沙箱失败发生在守卫 Allow 之后），见方案 §6.2。
//! - **审计**：包装结果以 `sandbox` 字段嵌进工具返回 JSON（`{"wrapped":true,"net":"open",...}`）。
//!
//! 本 crate 无 tokio 依赖：[`NativeChild::read_wait`] 是纯阻塞执行模型，由 tools/proc.rs 的
//! `spawn_blocking` + `tokio::time::timeout` 适配异步。
//!
//! 不覆盖（方案 §1.3 豁免清单）：MCP / LSP / Chrome 子进程；IM 无人值守全权档（既有独立决策）。
//! 边界声明：**这是写围栏不是读围栏**（受限令牌挡写不挡读）；exfiltration 归 data-auth/连接器线。

pub mod settings;
#[cfg(windows)]
pub mod win;
#[cfg(target_os = "linux")]
pub mod linux;
pub mod cmd_risk;
pub use cmd_risk::CmdRiskGuard;
pub mod probe;

use std::path::{Path, PathBuf};

use cmx_agent_core::guard::SandboxMode;
use serde_json::{Value, json};

pub use settings::{NetMode, SandboxSettings, WinCachePolicy};

/// 子进程出口的调用方 profile（`.git` deny 只挂 [`Profile::Shell`]，git 工具走无 deny 的
/// 第二套令牌——已拍板 §十-3；插件/测试命令同 Shell 语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// shell 工具与 run_tests：命令任意 → `.git` deny 生效（防 `rm -rf .git` 类误伤）。
    Shell,
    /// git 工具：合法写 `.git`（index.lock/objects）→ 无 deny。
    Git,
    /// 插件 command/wasm 载体。
    Plugin,
}

impl Profile {
    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Shell => "shell",
            Profile::Git => "git",
            Profile::Plugin => "plugin",
        }
    }
}

/// tools/proc.rs 调用沙箱包装器的上下文（方案 §6.1：core 不动，sandbox 档与 roots 从
/// `ToolCtx` 逐调用传入；`.git` profile 由调用方标识）。
pub struct ProcCtx<'a> {
    pub sandbox: SandboxMode,
    pub roots: &'a [PathBuf],
    pub profile: Profile,
}

/// 包装决策（tools/proc.rs 据此分路）。
#[derive(Debug, Clone)]
pub enum WrapDecision {
    /// 走 OS 沙箱原生 spawn（Windows 受限令牌 / Linux Landlock+seccomp）。
    Wrap,
    /// 不包装（ReadOnly 本就不跑 shell；DangerFullAccess = 显式信任）。
    NoWrap,
    /// fail-closed 拒绝（含行动指引文案，直接回灌模型）。
    Denied { reason: String },
    /// 沙箱不可用但用户已 require_os=false：降级裸跑 + degraded 标记（UI 黄标依据）。
    Degraded { reason: String },
}

/// 判定是否包装（含 require_os fail-closed 与 net 档平台校验）。
pub fn wrap_decision(ctx: &ProcCtx<'_>) -> WrapDecision {
    let s = settings::get();
    match ctx.sandbox {
        SandboxMode::ReadOnly | SandboxMode::DangerFullAccess => WrapDecision::NoWrap,
        SandboxMode::WorkspaceWrite => {
            // enforce 档平台校验：Windows 在受限令牌路线内无解（AppContainer 已否决，§2.2）——
            // 配置了即拒绝（诚实报错而非假装生效）。
            #[cfg(windows)]
            if s.net == NetMode::Enforce {
                return WrapDecision::Denied {
                    reason: deny_text("sandbox.net=enforce 仅 Linux 支持（Windows 受限令牌路线无硬断网手段；AppContainer 路线已否决）。请改用 poison 档或切换平台。"),
                };
            }
            match probe::os_sandbox_available() {
                probe::Availability::Available => WrapDecision::Wrap,
                probe::Availability::Unavailable(reason) => {
                    if s.require_os {
                        WrapDecision::Denied {
                            reason: deny_text(&format!(
                                "OS 沙箱不可用（{reason}）。fail-closed：本次执行被拒绝。\
                                 可由用户在设置中把 sandbox.require_os 置为 false 降级（将带黄标警示）。"
                            )),
                        }
                    } else {
                        WrapDecision::Degraded { reason }
                    }
                }
            }
        }
    }
}

/// fail-closed 拒绝文案模板（方案 §6.4：带行动指引，防模型换姿势空转烧回合）。
pub fn deny_text(reason: &str) -> String {
    format!("[沙箱拒绝·fail-closed] {reason}")
}

/// 沙箱包装报告（嵌进工具返回 JSON 的 `sandbox` 字段；denied/degraded 场景由调用方另行编码）。
pub fn report_json(wrapped: bool, net: NetMode, profile: Profile, degraded: Option<&str>) -> Value {
    json!({
        "wrapped": wrapped,
        "net": net.as_str(),
        "profile": profile.as_str(),
        "degraded": degraded,
    })
}

/// 环境变量覆盖集（poison 档黑洞代理 + 缓存 env 重定向，方案 §3.4/§4.3）。
/// 返回 (key, value) 列表；大小写双写（部分程序只认其一）。
pub fn env_overrides(roots: &[PathBuf]) -> Vec<(String, String)> {
    let s = settings::get();
    let mut out = Vec::new();
    if s.net == NetMode::Poison {
        for k in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
            out.push((k.to_string(), "http://127.0.0.1:9".to_string()));
        }
    }
    // 平台中立（红队3 P1-3）：Linux WorkspaceWrite 下 ~/.npm/~/.cargo/pip 缓存同样不可写，
    // 不重定向 = npm/cargo 主工作流断——与 Windows 同策略注入。
    if s.win_cache_policy == settings::WinCachePolicy::Redirect
        && let Some(root) = roots.first()
    {
        let base = root.join(".agent-cache");
        for (k, sub) in [
            ("NPM_CONFIG_CACHE", "npm"),
            ("PIP_CACHE_DIR", "pip"),
            ("CARGO_HOME", "cargo"),
        ] {
            let dir = base.join(sub);
            if std::fs::create_dir_all(&dir).is_ok() {
                out.push((k.to_string(), dir.display().to_string()));
            }
        }
    }
    out
}

/// Windows：放行集 = roots + %TEMP% + extra_write_roots +（redirect 档）.agent-cache。
/// Linux：放行集 = roots + /tmp + XDG_RUNTIME_DIR + /dev/null（逐挂载矩阵在 linux.rs）。
pub fn effective_write_roots(roots: &[PathBuf]) -> Vec<PathBuf> {
    let s = settings::get();
    let mut out: Vec<PathBuf> = roots.to_vec();
    if cfg!(windows) {
        out.push(std::env::temp_dir());
    } else {
        out.push(PathBuf::from("/tmp"));
        if let Ok(x) = std::env::var("XDG_RUNTIME_DIR") {
            out.push(PathBuf::from(x));
        }
    }
    for extra in &s.extra_write_roots {
        if extra.is_absolute() {
            out.push(extra.clone());
        } else if let Some(root) = roots.first() {
            out.push(root.join(extra));
        }
    }
    out
}

/// 一个受 OS 沙箱约束的子进程（平台各自实现；统一阻塞执行模型）。
/// `Arc` 内核 + `Clone`：超时路径由外层克隆一份调 [`NativeChild::terminate_tree`]，
/// 阻塞读由 spawn_blocking 持另一份——两线程互不干扰句柄生命周期（Drop 在最后一份归零）。
#[derive(Clone)]
pub struct NativeChild {
    pub pid: u32,
    inner: std::sync::Arc<NativeChildInner>,
}

#[cfg(windows)]
type NativeChildInner = win::child::WinChild;
#[cfg(target_os = "linux")]
type NativeChildInner = crate::linux::LinuxChild;
#[cfg(not(any(windows, target_os = "linux")))]
type NativeChildInner = UnsupportedChild;

impl NativeChild {
    /// 阻塞：并发读 stdout/stderr 至 EOF、等进程退出，返回 (exit_code, stdout, stderr)。
    /// 幂等约束：只应调用一次（管道所有权内部 take）；超时由外层 spawn_blocking + tokio timeout 控制。
    pub fn read_wait(&self) -> (i32, Vec<u8>, Vec<u8>) {
        self.inner.read_wait()
    }

    /// 整树终止（Job Terminate / SIGKILL 进程组）。线程安全：可与 read_wait 并发调用。
    pub fn terminate_tree(&self) {
        self.inner.terminate_tree();
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
struct UnsupportedChild;

#[cfg(not(any(windows, target_os = "linux")))]
impl UnsupportedChild {
    fn read_wait(&self) -> (i32, Vec<u8>, Vec<u8>) {
        (i32::MAX, Vec::new(), Vec::new())
    }
    fn terminate_tree(&self) {}
}

/// OS 沙箱原生 spawn（Windows CreateProcessAsUserW / Linux fork+exec+pre_exec）。
/// 调用前提：`wrap_decision` 已返回 `Wrap`。失败 = fail-closed（调用方编码为沙箱拒绝）。
#[cfg(windows)]
pub fn spawn_native(
    ctx: &ProcCtx<'_>,
    program: &str,
    args: &[String],
    cwd: &Path,
) -> Result<NativeChild, String> {
    let env = env_overrides(ctx.roots);
    let inner = win::spawn(ctx, program, args, cwd, &env)?;
    let pid = inner.pid();
    Ok(NativeChild { pid, inner: std::sync::Arc::new(inner) })
}

#[cfg(target_os = "linux")]
pub fn spawn_native(
    ctx: &ProcCtx<'_>,
    program: &str,
    args: &[String],
    cwd: &Path,
) -> Result<NativeChild, String> {
    let env = env_overrides(ctx.roots);
    let inner = linux::spawn(ctx, program, args, cwd, &env)?;
    let pid = inner.pid();
    Ok(NativeChild { pid, inner: std::sync::Arc::new(inner) })
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn spawn_native(
    _ctx: &ProcCtx<'_>,
    _program: &str,
    _args: &[String],
    _cwd: &Path,
) -> Result<NativeChild, String> {
    Err("本平台无 OS 沙箱实现".to_string())
}

/// 测试串行锁：settings 全局内核（OnceLock 单例）在并行测试下会被 `init(None)` 与持久化
/// 测试互踩——所有调用 `settings::init/set/sid_pair` 的测试必须先取本锁。
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// crate 版本标识（健康报告/事件溯源用）。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn sandboxed_spawn_runs_and_captures() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let root = std::env::temp_dir().join(format!("cmx-sbx-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Shell,
        };
        let decision = wrap_decision(&ctx);
        assert!(matches!(decision, WrapDecision::Wrap), "探针判定：{decision:?}");
        let child = spawn_native(&ctx, "cmd", &["/C".to_string(), "echo hello-sbx".to_string()], &root)
            .expect("原生受限 spawn（普通桌面用户首验收项）");
        let (code, out, err) = child.read_wait();
        println!("code={code} stdout={:?} stderr={:?}", String::from_utf8_lossy(&out), String::from_utf8_lossy(&err));
        assert_eq!(code, 0, "退出码");
        assert!(
            String::from_utf8_lossy(&out).contains("hello-sbx"),
            "stdout 应捕获到子进程输出"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 逃逸负例（§8.1）：在**允许集之外**的用户可写目录落文件——OS 层必须拒绝。
    /// 注意逃逸靶点不能是 %TEMP%（它在默认放行集内），取工程同级的临时兄弟目录。
    #[cfg(windows)]
    #[test]
    fn sandboxed_write_outside_roots_denied() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let root = std::env::temp_dir().join(format!("cmx-sbx-ws-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        // 逃逸靶：EXE 所在盘根下的临时目录（正常用户可写、不在 roots/TEMP 放行集）。
        let exe_dir = std::env::current_exe().unwrap();
        let drive = exe_dir.ancestors().nth(2).map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("C:"));
        let outside = drive.join(format!("cmx-escape-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&outside).unwrap();

        let ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Shell,
        };
        let child = spawn_native(
            &ctx,
            "cmd",
            &["/C".to_string(), format!("echo pwned > {}", outside.join("x.txt").display())],
            &root,
        )
        .expect("受限 spawn 本体应成功");
        let (code, _out, err) = child.read_wait();
        let inside = root.join("ok.txt");
        let child2 = spawn_native(&ctx, "cmd", &["/C".to_string(), format!("echo ok > {}", inside.display())], &root)
            .expect("受限 spawn（对照组）");
        let (code2, _o2, _e2) = child2.read_wait();
        let escaped = outside.join("x.txt").exists();
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(code2, 0, "工作区内写（对照组）应成功");
        assert_ne!(code, 0, "工作区外写应被 OS 拒（exit={code} err={:?}）", String::from_utf8_lossy(&err));
        assert!(!escaped, "逃逸文件不得存在");
    }

    /// 删除类逃逸（§8.1 → §十二 残余登记）：实测 **DELETE 不在 write-restricted 第二道
    /// 写检查覆盖内**（父目录授予用户删除权时，受限子进程可删除工作区外文件）——这是
    /// write-restricted 机制的诚实边界（codex 同限）。删除的实际闸 = S3 命令风险屏
    /// （del/rm 模式升审批）+ 审计；本测试钉住该边界语义，防止未来误以为 OS 层已覆盖。
    #[cfg(windows)]
    #[test]
    fn sandboxed_delete_outside_roots_is_known_residual() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let root = std::env::temp_dir().join(format!("cmx-sbx-del-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let exe_dir = std::env::current_exe().unwrap();
        let drive = exe_dir.ancestors().nth(2).map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("C:"));
        let outside = drive.join(format!("cmx-escape-del-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        let victim = outside.join("victim.txt");
        std::fs::write(&victim, b"keep").unwrap();

        let ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Shell,
        };
        let c = spawn_native(
            &ctx, "cmd",
            &["/C".to_string(), format!("del /f {}", victim.display())],
            &root,
        ).expect("受限 spawn");
        let (code, _o, _e) = c.read_wait();
        let deleted = !victim.exists();
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&root);
        // 边界钉死：无论本机卷 ACL 如何配置，测试只证明「OS 层不承诺删除围栏」这一事实本身。
        // （当前机器实测=可删；若未来 Windows 收紧语义，此测试自然仍通过。）
        let _ = (code, deleted);
    }

    /// .git deny 双 profile 语义（已拍板 §十-3）：shell 令牌拒写 .git 且**可读**（deny mask 排除
    /// SYNCHRONIZE/READ_CONTROL——红队 A-7）；git 令牌合法写 .git。
    #[cfg(windows)]
    #[test]
    fn git_deny_shell_profile_git_tool_allowed() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let root = std::env::temp_dir().join(format!("cmx-sbx-git-{}", std::process::id()));
        let gitdir = root.join(".git");
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(gitdir.join("HEAD"), "ref: refs/heads/main").unwrap();

        let shell_ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Shell,
        };
        // ① shell 令牌：写 .git 拒。
        let c1 = spawn_native(
            &shell_ctx, "cmd",
            &["/C".to_string(), format!("echo x > {}", gitdir.join("probe.txt").display())],
            &root,
        ).expect("shell profile spawn");
        let (code1, _o, _e) = c1.read_wait();
        // ② shell 令牌：读 .git 允许（deny 位集不含读/SYNCHRONIZE 的验收锚点）。
        let c2 = spawn_native(
            &shell_ctx, "cmd",
            &["/C".to_string(), format!("type {}", gitdir.join("HEAD").display())],
            &root,
        ).expect("shell profile spawn 2");
        let (code2, out2, _e2) = c2.read_wait();
        // ③ git 令牌：写 .git 允许（第二套 profile）。
        let git_ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Git,
        };
        let c3 = spawn_native(
            &git_ctx, "cmd",
            &["/C".to_string(), format!("echo x > {}", gitdir.join("probe.txt").display())],
            &root,
        ).expect("git profile spawn");
        let (code3, _o3, _e3) = c3.read_wait();

        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(code1, 0, "shell profile 写 .git 应拒（exit={code1}）");
        assert_eq!(code2, 0, "shell profile 读 .git 应允许");
        assert!(String::from_utf8_lossy(&out2).contains("main"), "读到的 HEAD 内容正确");
        assert_eq!(code3, 0, "git profile 写 .git 应允许（exit={code3}）");
    }

    /// HKCU（红队 A-14）：写拒（防持久化）/ 读允——注册表键同受 restricting 写检查。
    #[cfg(windows)]
    #[test]
    fn hkcu_write_denied_read_allowed() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let root = std::env::temp_dir().join(format!("cmx-sbx-reg-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Shell,
        };
        let c1 = spawn_native(
            &ctx, "reg",
            &["add".to_string(), r"HKCU\Software\cmx-sbx-probe".into(), "/v".into(), "x".into(), "/d".into(), "1".into(), "/f".into()],
            &root,
        ).expect("spawn reg add");
        let (code1, _o, _e) = c1.read_wait();
        let c2 = spawn_native(
            &ctx, "reg",
            &["query".to_string(), r"HKCU\Software\Microsoft".into()],
            &root,
        ).expect("spawn reg query");
        let (code2, _o2, _e2) = c2.read_wait();
        let _ = std::fs::remove_dir_all(&root);
        assert_ne!(code1, 0, "写 HKCU 应拒（exit={code1}）");
        assert_eq!(code2, 0, "读 HKCU 应允许（exit={code2}）");
    }

    /// 越界读允许（§8.1：写围栏语义，非保密边界——明示锚点）。
    #[cfg(windows)]
    #[test]
    fn read_outside_roots_allowed() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        crate::settings::init(None);
        let root = std::env::temp_dir().join(format!("cmx-sbx-rd-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Shell,
        };
        let hosts = PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts");
        if !hosts.exists() {
            return; // 环境缺失容忍
        }
        let c = spawn_native(
            &ctx, "cmd",
            &["/C".to_string(), format!("type {}", hosts.display())],
            &root,
        ).expect("spawn type");
        let (code, _o, _e) = c.read_wait();
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(code, 0, "读工作区外系统文件应允许（exit={code}）");
    }

    #[test]
    fn wrap_decision_danger_and_readonly_never_wrap() {
        settings::init(None);
        let roots = vec![PathBuf::from("/nonexistent")];
        let d = wrap_decision(&ProcCtx { sandbox: SandboxMode::DangerFullAccess, roots: &roots, profile: Profile::Shell });
        assert!(matches!(d, WrapDecision::NoWrap));
        let r = wrap_decision(&ProcCtx { sandbox: SandboxMode::ReadOnly, roots: &roots, profile: Profile::Shell });
        assert!(matches!(r, WrapDecision::NoWrap));
    }

    #[test]
    fn report_json_shape() {
        let v = report_json(true, NetMode::Open, Profile::Shell, None);
        assert_eq!(v["wrapped"], json!(true));
        assert_eq!(v["net"], json!("open"));
        assert_eq!(v["profile"], json!("shell"));
        assert_eq!(v["degraded"], Value::Null);
    }
}
