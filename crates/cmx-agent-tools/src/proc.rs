//! 共享子进程执行器（shell / git / run_tests 复用）：cwd 限定 + 超时 + 输出截断 + 并发读防死锁。
//!
//! Windows 适配（2026-09-08 方案 P0/P1/P1.5）：
//! - **shell 探测链**（[`resolve_shell`]）：`CMX_AGENT_SHELL > pwsh > powershell > sh（PATH 或由
//!   git.exe 反推 usr\bin）> cmd`，不再硬编码 `sh -c`（持久 PATH 不含 Git 的 usr\bin，
//!   "启动 sh 失败" 的根因）；
//! - **argv 模板**（[`shell_argv`]）：PowerShell 用 `-NoProfile -NonInteractive -Command` +
//!   UTF-8 输出前缀；cmd 用 `/C` 且拒绝多行；sh 维持 `-c`；
//! - **CREATE_NO_WINDOW**：桌面壳（windows 子系统）下子进程不闪黑窗；
//! - **Job Object**：超时 Terminate 整棵进程树（`start_kill` 只杀直接子进程，孙进程会孤儿化）；
//! - 输出按字节读 + `from_utf8_lossy`：非 UTF-8 输出（cmd/GBK）至少不整段丢失。
//!
//! OS 沙箱（方案 20260914_沙箱OS级隔离完善方案 S0-S3，v1.1）：[`run_profiled`] /
//! [`run_cmd_profiled`] 在 WorkspaceWrite 档走 [`cmx_agent_sandbox::spawn_native`] 原生受限
//! spawn（Windows 受限令牌+ACL / Linux Landlock+seccomp），包装结果以 `sandbox` 字段嵌进返回
//! JSON（ToolResult 通道审计，core 不动）；ReadOnly/DangerFullAccess 与沙箱降级走既有 [`run`]。

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use cmx_agent_sandbox::{ProcCtx, WrapDecision};
use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

pub use cmx_agent_sandbox::Profile;

pub const MAX_OUT: usize = 64 * 1024;
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TIMEOUT_MS: u64 = 120_000;

/// PowerShell 命令前的 UTF-8 输出编码前缀（中文输出防乱码；对齐 codex powershell.rs）。
pub const PS_UTF8_PREFIX: &str = "try { [Console]::OutputEncoding=[System.Text.Encoding]::UTF8 } catch {}\n";

fn truncate(mut s: String) -> (String, bool) {
    if s.len() > MAX_OUT {
        // 按字符边界安全截断
        let mut end = MAX_OUT;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        s.truncate(end);
        (s, true)
    } else {
        (s, false)
    }
}

/// shell 种类（决定 argv 模板与命令语义）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    /// POSIX sh（Unix 默认；Windows 上为 Git Bash）。
    Sh,
    /// PowerShell（pwsh 7 优先，回退 Windows PowerShell 5.1）。
    PowerShell,
    /// cmd.exe（最后兜底；无可靠多行语义）。
    Cmd,
}

/// 解析后的 shell：可执行（路径或短名）+ 种类。
#[derive(Debug, Clone)]
pub struct ResolvedShell {
    pub program: String,
    pub kind: ShellKind,
}

/// CMX_AGENT_SHELL 合法值（文件名白名单；借鉴 Claude Code 教训——任意路径会被误配成 shell）。
fn shell_kind_of(file_name: Option<&str>) -> Option<ShellKind> {
    match file_name?.to_ascii_lowercase().as_str() {
        "sh.exe" | "sh" | "bash.exe" | "bash" => Some(ShellKind::Sh),
        "pwsh.exe" | "pwsh" | "powershell.exe" | "powershell" => Some(ShellKind::PowerShell),
        "cmd.exe" | "cmd" => Some(ShellKind::Cmd),
        _ => None,
    }
}

/// 把 CMX_AGENT_SHELL 的值（绝对路径或短名）映射为 ResolvedShell；路径形式必须真实存在。
fn shell_from_config(raw: &str, exists: &dyn Fn(&Path) -> bool) -> Option<ResolvedShell> {
    let p = Path::new(raw);
    let kind = shell_kind_of(p.file_name().and_then(|s| s.to_str()))?;
    if p.components().count() > 1 && !exists(p) {
        return None; // 显式路径不存在 → 忽略该配置，走探测链
    }
    Some(ResolvedShell { program: raw.to_string(), kind })
}

/// 探测链核心（纯函数，参数化 env/PATH/存在性谓词，便于单测）。
///
/// 优先级（Windows 适配方案 §5.1，已拍板 pwsh 优先）：`CMX_AGENT_SHELL > pwsh > powershell >
/// sh（PATH 直查 → 由 git.exe 反推同安装树 usr\bin\sh.exe）> cmd（COMSPEC）`。
/// 非 Windows 维持 `sh`（PATH 语义，与旧行为一致）。
fn resolve_shell_inner(
    env: &dyn Fn(&str) -> Option<String>,
    path_var: &str,
    exists: &dyn Fn(&Path) -> bool,
) -> ResolvedShell {
    if !cfg!(windows) {
        return ResolvedShell { program: "sh".into(), kind: ShellKind::Sh };
    }
    // ① 显式覆盖（无效值忽略并继续探测，不硬失败）。
    if let Some(raw) = env("CMX_AGENT_SHELL")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
    {
        if let Some(sh) = shell_from_config(&raw, exists) {
            return sh;
        }
        eprintln!(
            "[proc] CMX_AGENT_SHELL={raw:?} 无效或不存在，已忽略（合法：sh/bash/pwsh/powershell/cmd 短名或其 .exe 路径）"
        );
    }
    let dirs: Vec<PathBuf> = path_var
        .split(';')
        .map(|s| s.trim().trim_matches('"'))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .collect();
    let on_path = |exe: &str| dirs.iter().map(|d| d.join(exe)).find(|p| exists(p));
    // ②③ PowerShell 优先（拍板 §7-Q2：企业桌面零依赖兜底，powershell.exe 5.1 恒在）。
    for exe in ["pwsh.exe", "powershell.exe"] {
        if let Some(p) = on_path(exe) {
            return ResolvedShell { program: p.to_string_lossy().into_owned(), kind: ShellKind::PowerShell };
        }
    }
    // ④ sh：PATH 直查 → 由 git.exe 反推同安装树（D:\Git\cmd\git.exe → D:\Git\usr\bin\sh.exe）。
    if let Some(p) = on_path("sh.exe") {
        return ResolvedShell { program: p.to_string_lossy().into_owned(), kind: ShellKind::Sh };
    }
    if let Some(git) = on_path("git.exe")
        && let Some(root) = git.parent().and_then(Path::parent)
    {
        let sh = root.join("usr").join("bin").join("sh.exe");
        if exists(&sh) {
            return ResolvedShell { program: sh.to_string_lossy().into_owned(), kind: ShellKind::Sh };
        }
    }
    // ⑤ 兜底 cmd。
    ResolvedShell {
        program: env("COMSPEC").filter(|s| !s.trim().is_empty()).unwrap_or_else(|| "cmd.exe".into()),
        kind: ShellKind::Cmd,
    }
}

/// 解析当前进程应使用的 shell（进程内缓存一次；CMX_AGENT_SHELL / PATH 以启动时为准）。
pub fn resolve_shell() -> ResolvedShell {
    static CACHE: std::sync::OnceLock<ResolvedShell> = std::sync::OnceLock::new();
    CACHE
        .get_or_init(|| {
            let sh = resolve_shell_inner(
                &|k| std::env::var(k).ok(),
                &std::env::var("PATH").unwrap_or_default(),
                &|p| p.is_file(),
            );
            #[cfg(windows)]
            eprintln!("[proc] shell 解析：{}（{:?}）", sh.program, sh.kind);
            sh
        })
        .clone()
}

/// 组装 shell argv（命令字符串 → 完整参数表）。`Err` = 可行动的拒绝原因（编码进工具结果回灌模型）。
pub fn shell_argv(sh: &ResolvedShell, cmd: &str) -> Result<Vec<String>, String> {
    match sh.kind {
        ShellKind::Sh => Ok(vec![sh.program.clone(), "-c".into(), cmd.to_string()]),
        ShellKind::PowerShell => Ok(vec![
            sh.program.clone(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-Command".into(),
            format!("{PS_UTF8_PREFIX}{cmd}"),
        ]),
        ShellKind::Cmd => {
            if cmd.lines().count() > 1 {
                Err("cmd.exe 不支持多行命令；请改写为单行，或设 CMX_AGENT_SHELL 指向 pwsh/sh".into())
            } else {
                Ok(vec![sh.program.clone(), "/C".into(), cmd.to_string()])
            }
        }
    }
}

/// 解析 shell 并执行一条命令字符串（shell 工具与 run_tests 自定义命令的统一入口）。
pub async fn run_cmd(cmd: &str, cwd: &Path, timeout_ms: u64) -> Value {
    let sh = resolve_shell();
    match shell_argv(&sh, cmd) {
        Ok(argv) => run(&sh.program, &argv[1..], cwd, timeout_ms).await,
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// 运行 `program args...`，cwd=`cwd`，超时 `timeout_ms`。返回结构化 JSON（exit_code/stdout/stderr/…）。
/// 永不 Err——错误也编码进返回值，供模型自愈。
pub async fn run(program: &str, args: &[String], cwd: &Path, timeout_ms: u64) -> Value {
    let timeout_ms = timeout_ms.clamp(1, MAX_TIMEOUT_MS);
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // CREATE_NO_WINDOW：Tauri 壳（windows 子系统）下子进程不闪黑窗；console 子系统无影响。
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    let mut child = match command.spawn() {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": format!("启动 {program} 失败: {e}")}),
    };
    // Job Object（Windows）：整棵进程树纳入 kill-on-close；超时走 Terminate 全树收尸。
    #[cfg(windows)]
    let job = child.raw_handle().and_then(|h| crate::job::attach_child_job(h as usize));
    let mut out_buf = Vec::new();
    let mut err_buf = Vec::new();
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    // stdout/stderr 必须**并发**读：串行先读尽 stdout 再读 stderr 时，子进程写满 stderr
    // 管道缓冲（~64KB）即写阻塞 → stdout 停产 → 第一个 read_to_end 永不 EOF → 拖到超时误杀
    // （cargo/编译类 stderr 高产场景必踩）。
    let read_fut = async {
        let read_out = async {
            if let Some(s) = so.as_mut() {
                let _ = s.read_to_end(&mut out_buf).await;
            }
        };
        let read_err = async {
            if let Some(s) = se.as_mut() {
                let _ = s.read_to_end(&mut err_buf).await;
            }
        };
        tokio::join!(read_out, read_err);
        child.wait().await
    };
    // 字节读 + lossy：非 UTF-8 输出（cmd/GBK 场景）不整段丢失（PowerShell 已由 UTF-8 前缀规整）。
    match tokio::time::timeout(Duration::from_millis(timeout_ms), read_fut).await {
        Ok(Ok(status)) => {
            let (out, ot) = truncate(String::from_utf8_lossy(&out_buf).into_owned());
            let (err, et) = truncate(String::from_utf8_lossy(&err_buf).into_owned());
            json!({
                "ok": status.success(),
                "exit_code": status.code(),
                "stdout": out,
                "stderr": err,
                "truncated": ot || et,
                "timed_out": false,
            })
        }
        Ok(Err(e)) => json!({"ok": false, "error": format!("等待进程失败: {e}")}),
        Err(_) => {
            #[cfg(windows)]
            if let Some(j) = &job {
                j.terminate(); // 先灭整树（孙进程收尸），start_kill 仅兜底直接子进程。
            }
            let _ = child.start_kill();
            let (out, _) = truncate(String::from_utf8_lossy(&out_buf).into_owned());
            let (err, _) = truncate(String::from_utf8_lossy(&err_buf).into_owned());
            json!({
                "ok": false,
                "exit_code": Value::Null,
                "stdout": out,
                "stderr": err,
                "timed_out": true,
                "note": format!("超时（{timeout_ms}ms）已终止"),
            })
        }
    }
}

/// 解析 shell 并执行一条命令字符串（**沙箱感知**：WorkspaceWrite 档走 OS 沙箱原生 spawn）。
pub async fn run_cmd_profiled(ctx: &ProcCtx<'_>, cmd: &str, cwd: &Path, timeout_ms: u64) -> Value {
    let sh = resolve_shell();
    let net = cmx_agent_sandbox::settings::get().net;
    match shell_argv(&sh, cmd) {
        Ok(argv) => run_profiled(ctx, &sh.program, &argv[1..], cwd, timeout_ms).await,
        Err(e) => json!({
            "ok": false, "error": e,
            "sandbox": {
                "wrapped": false, "denied": Value::Null, "net": net.as_str(),
                "profile": ctx.profile.as_str(), "degraded": Value::Null,
            },
        }),
    }
}

/// 运行 `program args...`（**沙箱感知**）：
/// - ReadOnly / DangerFullAccess → 既有 [`run`]（不包装）；
/// - WorkspaceWrite → `cmx_agent_sandbox::wrap_decision` 分路：Wrap=原生受限 spawn
///   （`spawn_blocking` + tokio timeout 适配阻塞执行模型；超时先整树终止再收兜底输出）；
///   Denied=fail-closed 拒绝；Degraded=require_os=false 的裸跑降级（带 degraded 标记）。
///   包装结果嵌 `sandbox` 字段（wrapped/net/profile/degraded/elapsed_ms）——ToolResult 通道
///   审计：模型可见、SessionLog 落库，*Model-visible means logged* 不变式不破。
pub async fn run_profiled(ctx: &ProcCtx<'_>, program: &str, args: &[String], cwd: &Path, timeout_ms: u64) -> Value {
    let timeout_ms = timeout_ms.clamp(1, MAX_TIMEOUT_MS);
    let net = cmx_agent_sandbox::settings::get().net;
    match cmx_agent_sandbox::wrap_decision(ctx) {
        WrapDecision::NoWrap => run(program, args, cwd, timeout_ms).await,
        WrapDecision::Denied { reason } => json!({
            "ok": false,
            "error": reason,
            "sandbox": {
                "wrapped": false, "denied": reason, "net": net.as_str(),
                "profile": ctx.profile.as_str(), "degraded": Value::Null,
            },
        }),
        WrapDecision::Degraded { reason } => {
            let mut v = run(program, args, cwd, timeout_ms).await;
            if let Value::Object(ref mut m) = v {
                // net 如实报「未应用」（degraded 裸跑不走 spawn_native，黑洞代理/缓存重定向都
                // 不生效——红队2 P1-2：报 poison 实际畅通 = 审计失真）。
                m.insert(
                    "sandbox".into(),
                    json!({
                        "wrapped": false, "net": net.as_str(), "net_applied": false,
                        "profile": ctx.profile.as_str(), "degraded": reason,
                    }),
                );
            }
            v
        }
        WrapDecision::Wrap => {
            let t0 = std::time::Instant::now();
            let child = match cmx_agent_sandbox::spawn_native(ctx, program, args, cwd) {
                Ok(c) => c,
                Err(reason) => {
                    // 错误分类（红队2 P2-4/P1-3，前缀由 win::spawn 标注）：
                    // [vol]/[acl]/[token] = 沙箱设施类——require_os=true 走 fail-closed 拒绝；
                    //   false 走降级裸跑（方案 §4.3 FAT 卷逃生门；env 注入不可用，net 如实报未应用）。
                    // [spawn] = 普通启动失败（程序不存在等）——不加沙箱话术，普通错误回灌。
                    // 其余（[sandbox]）= 沙箱机制失败——恒 fail-closed 拒绝。
                    if reason.starts_with("[vol]") || reason.starts_with("[acl]") || reason.starts_with("[token]") {
                        if cmx_agent_sandbox::settings::get().require_os {
                            return json!({
                                "ok": false,
                                "error": cmx_agent_sandbox::deny_text(&reason),
                                "sandbox": {
                                    "wrapped": false, "denied": reason, "net": net.as_str(),
                                    "profile": ctx.profile.as_str(), "degraded": Value::Null,
                                },
                            });
                        }
                        let mut v = run(program, args, cwd, timeout_ms).await;
                        if let Value::Object(ref mut m) = v {
                            m.insert(
                                "sandbox".into(),
                                json!({
                                    "wrapped": false, "net": net.as_str(), "net_applied": false,
                                    "profile": ctx.profile.as_str(), "degraded": reason,
                                }),
                            );
                        }
                        return v;
                    }
                    if let Some(plain) = reason.strip_prefix("[spawn] ") {
                        return json!({
                            "ok": false, "error": plain,
                            "sandbox": {
                                "wrapped": false, "denied": Value::Null, "net": net.as_str(),
                                "profile": ctx.profile.as_str(), "degraded": Value::Null,
                            },
                        });
                    }
                    return json!({
                        "ok": false,
                        "error": cmx_agent_sandbox::deny_text(&reason),
                        "sandbox": {
                            "wrapped": false, "denied": reason, "net": net.as_str(),
                            "profile": ctx.profile.as_str(), "degraded": Value::Null,
                        },
                    });
                }
            };
            // 超时杀树与阻塞读并存：克隆一份句柄当杀树柄，阻塞读任务把结果写共享槽。
            let killer = child.clone();
            type SpawnSlot = std::sync::Arc<tokio::sync::Mutex<Option<(i32, Vec<u8>, Vec<u8>)>>>;
            let slot: SpawnSlot = std::sync::Arc::new(tokio::sync::Mutex::new(None));
            let slot2 = slot.clone();
            let jh = tokio::task::spawn_blocking(move || {
                let r = child.read_wait();
                *slot2.blocking_lock() = Some(r);
            });
            let timed_out = tokio::time::timeout(Duration::from_millis(timeout_ms), jh).await.is_err();
            if timed_out {
                killer.terminate_tree();
            }
            // 非超时路径任务已完；超时路径终止后管道 EOF → 任务毫秒级收敛（5s 兜底轮询）。
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let mut result = slot.lock().await.take();
            while result.is_none() && std::time::Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
                result = slot.lock().await.take();
            }
            let Some((code, out_raw, err_raw)) = result else {
                return json!({
                    "ok": false, "exit_code": Value::Null, "timed_out": true,
                    "sandbox": {
                        "wrapped": true, "net": net.as_str(), "profile": ctx.profile.as_str(),
                        "degraded": Value::Null,
                    },
                    "note": format!("超时（{timeout_ms}ms）已整树终止；输出未及回收"),
                });
            };
            let (out, ot) = truncate(String::from_utf8_lossy(&out_raw).into_owned());
            let (err, et) = truncate(String::from_utf8_lossy(&err_raw).into_owned());
            json!({
                "ok": code == 0,
                "exit_code": code,
                "stdout": out,
                "stderr": err,
                "truncated": ot || et,
                "timed_out": timed_out,
                "note": timed_out.then(|| format!("超时（{timeout_ms}ms）已整树终止")),
                "sandbox": {
                    "wrapped": true, "net": net.as_str(), "profile": ctx.profile.as_str(),
                    "degraded": Value::Null, "elapsed_ms": t0.elapsed().as_millis() as u64,
                },
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    use std::fs;

    fn sh(kind: ShellKind) -> ResolvedShell {
        ResolvedShell { program: "X".into(), kind }
    }

    #[test]
    fn argv_sh_uses_dash_c() {
        let a = shell_argv(&sh(ShellKind::Sh), "echo hi").unwrap();
        assert_eq!(a, vec!["X", "-c", "echo hi"]);
    }

    #[test]
    fn argv_powershell_no_profile_noninteractive_utf8_prefix() {
        let a = shell_argv(&sh(ShellKind::PowerShell), "Get-Date").unwrap();
        assert_eq!(&a[..4], &["X", "-NoProfile", "-NonInteractive", "-Command"]);
        assert!(a[4].starts_with(PS_UTF8_PREFIX));
        assert!(a[4].ends_with("Get-Date"));
    }

    #[test]
    fn argv_cmd_single_line_ok_multi_line_rejected() {
        assert_eq!(shell_argv(&sh(ShellKind::Cmd), "dir").unwrap(), vec!["X", "/C", "dir"]);
        let err = shell_argv(&sh(ShellKind::Cmd), "dir\necho x").unwrap_err();
        assert!(err.contains("多行"), "{err}");
    }

    #[test]
    fn config_shell_short_name_and_whitelist() {
        let exists = |_: &Path| false;
        assert_eq!(shell_from_config("pwsh", &exists).unwrap().kind, ShellKind::PowerShell);
        assert_eq!(shell_from_config("bash", &exists).unwrap().kind, ShellKind::Sh);
        assert!(shell_from_config("zsh", &exists).is_none(), "白名单外拒绝");
        assert!(shell_from_config("C:/no/such/sh.exe", &exists).is_none(), "路径形式必须存在");
    }

    #[cfg(windows)]
    mod windows_resolve {
        use super::super::*;
        use std::fs;

        #[test]
        fn env_override_wins_and_invalid_falls_through() {
            let env = |k: &str| (k == "CMX_AGENT_SHELL").then(|| "C:/tools/bash.exe".to_string());
            let hit = |p: &Path| p.ends_with("tools/bash.exe");
            let r = resolve_shell_inner(&env, "", &hit);
            assert_eq!((r.program.as_str(), r.kind), ("C:/tools/bash.exe", ShellKind::Sh));

            let bad = |k: &str| (k == "CMX_AGENT_SHELL").then(|| "C:/evil/notepad.exe".to_string());
            let r = resolve_shell_inner(&bad, "", &|_| false);
            assert_eq!(r.kind, ShellKind::Cmd, "无效配置忽略后落到 cmd 兜底");
        }

        #[test]
        fn pwsh_beats_sh_on_path() {
            let dir = std::env::temp_dir().join(format!("cmx-sh-{}", std::process::id()));
            let _ = fs::create_dir_all(dir.join("a"));
            let _ = fs::create_dir_all(dir.join("b"));
            fs::write(dir.join("a/pwsh.exe"), b"").unwrap();
            fs::write(dir.join("b/sh.exe"), b"").unwrap();
            let path_var = format!("{};{}", dir.join("a").display(), dir.join("b").display());
            let r = resolve_shell_inner(&|_| None, &path_var, &|p| p.is_file());
            assert_eq!(r.kind, ShellKind::PowerShell);
            assert!(r.program.ends_with("pwsh.exe"));
            let _ = fs::remove_dir_all(&dir);
        }

        #[test]
        fn sh_derived_from_git_exe_install_tree() {
            let dir = std::env::temp_dir().join(format!("cmx-git-{}", std::process::id()));
            let _ = fs::create_dir_all(dir.join("cmd"));
            let _ = fs::create_dir_all(dir.join("usr/bin"));
            fs::write(dir.join("cmd/git.exe"), b"").unwrap();
            fs::write(dir.join("usr/bin/sh.exe"), b"").unwrap();
            let path_var = format!("{}", dir.join("cmd").display());
            let r = resolve_shell_inner(&|_| None, &path_var, &|p| p.is_file());
            assert_eq!(r.kind, ShellKind::Sh);
            assert!(r.program.ends_with("usr\\bin\\sh.exe"), "{}", r.program);
            let _ = fs::remove_dir_all(&dir);
        }
    }

    /// 沙箱路径（run_profiled → 受限令牌 + spawn 属性挂 Job）下超时整树收尸（红队 P2-2 验收锚：
    /// 孙进程零漏杀，且必须覆盖 spawn 属性路线而非旧 attach 路线）。
    #[cfg(windows)]
    #[tokio::test]
    async fn profiled_timeout_kills_tree_under_sandbox() {
        use cmx_agent_core::guard::SandboxMode;
        let root = std::env::temp_dir().join(format!("cmx-pf-tree-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let child_ps1 = root.join("child.ps1");
        fs::write(&child_ps1, format!(
            "Set-Content -Path '{}' -Value $PID -Encoding ascii
Start-Sleep -Seconds 60
",
            root.join("gpid.txt").display()
        )).unwrap();
        let parent_ps1 = root.join("parent.ps1");
        fs::write(&parent_ps1, format!(
            "$p = Start-Process powershell -ArgumentList '-NoProfile','-File','{}' -PassThru -WindowStyle Hidden
Set-Content -Path '{}' -Value $p.Id -Encoding ascii
Wait-Process -Id $p.Id
",
            child_ps1.display(), root.join("ppid.txt").display()
        )).unwrap();
        let args = vec!["-NoProfile".to_string(), "-File".to_string(), parent_ps1.to_string_lossy().into_owned()];
        let ctx = ProcCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            roots: std::slice::from_ref(&root),
            profile: Profile::Shell,
        };
        let out = run_profiled(&ctx, "powershell", &args, &root, 8_000).await;
        assert_eq!(out["timed_out"], serde_json::json!(true), "{out}");
        assert_eq!(out["sandbox"]["wrapped"], serde_json::json!(true), "须走沙箱包装路径");
        let alive = |pid: u32| {
            let o = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output().unwrap();
            String::from_utf8_lossy(&o.stdout).contains(&format!("\",{pid},\""))
        };
        let mut pids = Vec::new();
        for f in ["ppid.txt", "gpid.txt"] {
            if let Ok(s) = fs::read_to_string(root.join(f))
                && let Ok(pid) = s.trim().parse::<u32>() { pids.push(pid); }
        }
        assert_eq!(pids.len(), 2, "父子两代都应记录了 PID");
        for _ in 0..25 {
            if !pids.iter().any(|&p| alive(p)) { break; }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        for &p in &pids {
            assert!(!alive(p), "PID {p} 应已被整树收尸（沙箱 Job 路线）");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn timeout_kills_whole_process_tree_via_job() {
        // 父 powershell 起一个孙 powershell（各自写 PID 后长睡）；超时后两代都必须死。
        // PID 文件用 ascii 编码写（PS5.1 Set-Content 默认 UTF-16 会读不回来）；超时给足冷启动时间。
        let dir = std::env::temp_dir().join(format!("cmx-job-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let child_ps1 = dir.join("child.ps1");
        fs::write(
            &child_ps1,
            format!(
                "Set-Content -Path '{}' -Value $PID -Encoding ascii\nStart-Sleep -Seconds 60\n",
                dir.join("gpid.txt").display()
            ),
        )
        .unwrap();
        let parent_ps1 = dir.join("parent.ps1");
        fs::write(
            &parent_ps1,
            format!(
                "$p = Start-Process powershell -ArgumentList '-NoProfile','-File','{}' -PassThru -WindowStyle Hidden\nSet-Content -Path '{}' -Value $p.Id -Encoding ascii\nWait-Process -Id $p.Id\n",
                child_ps1.display(),
                dir.join("ppid.txt").display()
            ),
        )
        .unwrap();
        let args = vec!["-NoProfile".to_string(), "-File".to_string(), parent_ps1.to_string_lossy().into_owned()];
        let out = run("powershell", &args, &dir, 8_000).await;
        assert_eq!(out["timed_out"], serde_json::json!(true), "{out}");

        // tasklist CSV 精确按 PID 列匹配（避免内存数字子串误报）。
        let alive = |pid: u32| {
            let o = std::process::Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).contains(&format!("\",{pid}\","))
        };
        let mut pids = Vec::new();
        for f in ["ppid.txt", "gpid.txt"] {
            if let Ok(s) = fs::read_to_string(dir.join(f))
                && let Ok(pid) = s.trim().parse::<u32>()
            {
                pids.push(pid);
            }
        }
        assert_eq!(pids.len(), 2, "父子两代都应记录了 PID（冷启动 8s 内）");
        // 等待作业收尸落地（Terminate 异步生效）。
        for _ in 0..25 {
            if !pids.iter().any(|&p| alive(p)) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        for &p in &pids {
            assert!(!alive(p), "PID {p} 应已被 Job Object 收尸");
        }
        let _ = fs::remove_dir_all(&dir);
    }
}
