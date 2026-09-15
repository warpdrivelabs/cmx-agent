//! Windows 原生 spawn 链路（方案 §4.6，红队 A-5/C-5 修正后的完整设计）。
//!
//! std/tokio `Command` **均无带令牌创建进程的 API**（CommandExt 官方自认无 CreateProcessAsUserW
//! 等价物；tools/proc.rs 现行 tokio Command 链路对受限令牌不可复用）——本模块实现：
//! `CreateProcessAsUserW` + `STARTUPINFOEXW` 属性表（`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`
//! 显式继承 stdio 管道、`PROC_THREAD_ATTRIBUTE_JOB` **spawn 时挂 Job** 消掉现行 job.rs
//! spawn 后 attach 的孙进程竞态、`lpDesktop` = 独立 `CreateDesktop`——不设则受限进程可能
//! `STATUS_DLL_INIT_FAILED` 起不来；设默认桌面则有 SendMessage 攻击面——MSDN 警告 + Codex 同款）。
//!
//! `CreateProcessAsUserW` 对**自派生受限令牌**无需 `SeAssignPrimaryTokenPrivilege`
//! （codex unelevated 产线同款；红蓝审查判 C-5 权限论断为倾向误报，但保留为测试首验收锚点）。

#![cfg(windows)]

pub(crate) mod acl;
pub(crate) mod child;
pub(crate) mod token;

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::JobObjects::{
    CreateJobObjectW, JobObjectExtendedLimitInformation, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, SetInformationJobObject,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::StationsAndDesktops::CreateDesktopW;
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    InitializeProcThreadAttributeList, PROCESS_INFORMATION, STARTF_USESTDHANDLES,
    STARTUPINFOEXW, UpdateProcThreadAttribute, CREATE_NO_WINDOW,
};

use crate::win::child::WinChild;
use crate::ProcCtx;

/// usize ⇄ HANDLE 边界转换（句柄存储用 usize 保持 Send——对齐 tools/job.rs 惯例）。
fn h(v: usize) -> HANDLE {
    v as _
}

/// 属性表/旗标常量（windows-sys 0.59 导出不全——本地定义，值取 Win32 SDK）。
const PROC_THREAD_ATTRIBUTE_HANDLE_LIST: usize = 0x0002_0002;
const PROC_THREAD_ATTRIBUTE_JOB: usize = 0x0002_000D; // JOB_LIST = 13（19 是别的属性，GLE=24 实测锚定）
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
/// 独立桌面名（lpDesktop）。
const SBX_DESKTOP: &str = "cmx-agent-sbx";
/// lpDesktop 常驻 wide 缓冲（桌面句柄也常驻进程生命周期——成对，零分配）。
static SBX_DESKTOP_W: std::sync::OnceLock<&'static [u16]> = std::sync::OnceLock::new();

/// 独立桌面：进程生命周期内创建一次、常驻不关（DestroyDesktop 需其上无进程）。
fn sbx_desktop() -> Option<usize> {
    use std::sync::OnceLock;
    static DESK: OnceLock<Option<usize>> = OnceLock::new();
    *DESK.get_or_init(|| {
        let wide: Vec<u16> = SBX_DESKTOP.encode_utf16().chain(std::iter::once(0)).collect();
        let h = unsafe {
            CreateDesktopW(
                wide.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                GENERIC_ALL,
                std::ptr::null(),
            )
        };
        if h.is_null() {
            eprintln!(
                "[sandbox] CreateDesktopW 失败（GLE={}）——回退默认桌面",
                unsafe { windows_sys::Win32::Foundation::GetLastError() }
            );
            None
        } else {
            Some(h as usize)
        }
    })
}

/// kill-on-close Job（优先 spawn 属性挂入；属性表失败退 CREATE_SUSPENDED + assign）。
fn create_job() -> Option<usize> {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            CloseHandle(job);
            return None;
        }
        Some(job as usize)
    }
}

/// Windows 参数引号规则（CommandLineToArgvW 完整语义，红队 P1-1）：引号前 n 个连续反斜杠
/// 须写成 2n 个、作为引号前置时 2n+1 个、闭合引号前尾随反斜杠加倍——否则尾反斜杠参数
/// （`C:\dir\`）会吞引号并与后续参数合并。空串/含空白/含引号才加引号。
fn quote_arg(s: &str) -> String {
    if !s.is_empty() && !s.chars().any(|c| c.is_whitespace() || c == '"') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    let mut backslashes = 0usize;
    for c in s.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                for _ in 0..=backslashes {
                    out.push('\\');
                }
                out.push('"');
                backslashes = 0;
            }
            _ => {
                for _ in 0..backslashes {
                    out.push('\\');
                }
                out.push(c);
                backslashes = 0;
            }
        }
    }
    for _ in 0..backslashes {
        out.push('\\');
    }
    out.push('"');
    out
}

/// 构建完整命令行（UTF-16 可变缓冲——CreateProcessAsUserW 可能原地修改）。
fn build_cmdline(program: &str, args: &[String]) -> Vec<u16> {
    let mut line = quote_arg(program);
    for a in args {
        line.push(' ');
        line.push_str(&quote_arg(a));
    }
    line.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 环境块（UTF-16，"K=V\0…\0\0"）：父环境 + 覆盖（大小写不敏感替换）。
fn build_env_block(overrides: &[(String, String)]) -> Vec<u16> {
    // vars_os + lossy：vars() 对非 Unicode 项会 panic（红队 P2-6）。
    let mut pairs: Vec<(String, String)> = std::env::vars_os()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
        .collect();
    for (k, v) in overrides {
        pairs.retain(|(ek, _)| !ek.eq_ignore_ascii_case(k));
        pairs.push((k.clone(), v.clone()));
    }
    // CreateProcess 要求环境块**按名字母序（大小写不敏感）**排序——未排序块会 GLE=87
    // （std::process 用 BTreeMap 天然有序，手拼块必须自排序；实测教训）。
    pairs.sort_by(|a, b| a.0.to_uppercase().cmp(&b.0.to_uppercase()).then_with(|| a.0.cmp(&b.0)));
    let mut block = String::new();
    for (k, v) in pairs {
        block.push_str(&k);
        block.push('=');
        block.push_str(&v);
        block.push('\0');
    }
    block.push('\0'); // 结尾双 \0
    block.encode_utf16().collect()
}

/// 主入口：受限令牌 + 放行集 ACL + 原生 spawn。
///
/// **错误分类**（红队2 P2-4/P1-3，调用方 proc.rs 据前缀分路）：
/// - `[vol]` / `[acl]` / `[token]`：沙箱设施类失败——require_os=true 时 fail-closed 拒绝，
///   false 时降级裸跑（degraded）；
/// - `[sandbox]`：沙箱机制失败（Job/属性表/管道）——恒 fail-closed 拒绝；
/// - `[spawn]`：普通启动失败（程序不存在/cwd 无效等，与沙箱无关）——不加沙箱话术直接回灌。
pub(crate) fn spawn(
    ctx: &ProcCtx<'_>,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_overrides: &[(String, String)],
) -> Result<WinChild, String> {
    // ⓪ 入参防御（红队 P2-6）：NUL 会静默截断命令行——fail-closed 拒绝；cwd 转换前置
    // （资源获取后才发现 cwd 不可编码会泄管道/Job/属性表句柄——红队 P1-3）。
    if program.contains('\0') || args.iter().any(|a| a.contains('\0')) {
        return Err("[spawn] 参数含 NUL 字符，拒绝执行".into());
    }
    let cwd_w: Vec<u16> = cwd
        .to_str()
        .ok_or_else(|| format!("[spawn] cwd 转 UTF-16 失败：{}", cwd.display()))?
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    // ① 卷探测：**放行集全集**（roots + %TEMP% + extras）+ cwd 所在卷必须 NTFS/ReFS
    // （红队 P2-7：%TEMP% 落 FAT RAM-disk 的场景此前漏探）。前缀 [vol] = 设施类。
    let mut vol_targets: Vec<PathBuf> = crate::effective_write_roots(ctx.roots);
    vol_targets.push(cwd.to_path_buf());
    for p in &vol_targets {
        if p.exists() {
            acl::ensure_acl_capable_volume(p).map_err(|e| format!("[vol] {e}"))?;
        }
    }
    // ② ACL 放行集（幂等；首挂对既有树自动传播为一次性分钟级成本）。
    acl::ensure_workspace_acls(ctx.roots, ctx.profile).map_err(|e| format!("[acl] {e}"))?;
    // ③ 受限令牌（profile 决定挂哪枚 SANDBOX_SID → .git deny 语义）。
    // 残余登记（红队 P2-3）：未加 LUA_TOKEN（codex 为三旗标）——elevated 运行 agent 时子进程
    // 保留管理员组做读/exec，写围栏不受影响；二旗标语义已在本机真实测试锚定，不冒险改。
    let token = token::create(ctx.profile).map_err(|e| format!("[token] {e}"))?;

    // P0-1（红队1）：UpdateProcThreadAttribute 的 lpValue 只存指针、CreateProcessAsUserW 时才解引用
    // （MSDN：须存活至 DeleteProcThreadAttributeList）——两个 value 缓冲必须**函数级作用域**，
    // 不能是 if 块局部变量（悬垂 → 随机 87 / 错误句柄集被继承进受限子进程）。
    #[allow(unused_assignments)] // 初始值仅为满足确定性初始化；真实值在管道建好后回填
    let mut inherit_handles: [HANDLE; 2] = [std::ptr::null_mut(), std::ptr::null_mut()]; // P0-1：函数级作用域（属性表只存指针，须活到 CreateProcess 之后）
    #[allow(unused_assignments)]
    let mut job_handle_value: usize = 0;

    unsafe {
        // ④ 管道：写端可继承（进 HANDLE_LIST），读端父侧持有且不继承。
        let sa = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let mut out_r: HANDLE = std::ptr::null_mut();
        let mut out_w: HANDLE = std::ptr::null_mut();
        let mut err_r: HANDLE = std::ptr::null_mut();
        let mut err_w: HANDLE = std::ptr::null_mut();
        if CreatePipe(&mut out_r, &mut out_w, &sa, 0) == 0 {
            return Err(format!("[sandbox] CreatePipe(stdout) 失败（GLE={}）", windows_sys::Win32::Foundation::GetLastError()));
        }
        if CreatePipe(&mut err_r, &mut err_w, &sa, 0) == 0 {
            CloseHandle(out_r);
            CloseHandle(out_w);
            return Err(format!("[sandbox] CreatePipe(stderr) 失败（GLE={}）", windows_sys::Win32::Foundation::GetLastError()));
        }
        const HANDLE_FLAG_INHERIT: u32 = 1;
        windows_sys::Win32::Foundation::SetHandleInformation(out_r, HANDLE_FLAG_INHERIT, 0);
        windows_sys::Win32::Foundation::SetHandleInformation(err_r, HANDLE_FLAG_INHERIT, 0);

        // ⑤ Job + 属性表：两者**任一不可用即 fail-closed 拒绝**（红队 P1-2——旧兜底路径
        // bInheritHandles=1 却无 HANDLE_LIST，父进程全表可继承句柄泄入受限子进程 = 写围栏逃逸通道）。
        let job = create_job().ok_or_else(|| "[sandbox] Job Object 创建失败（fail-closed）".to_string())?;
        let mut attr_buf: Vec<u64> = Vec::new();
        let attr_list: *mut core::ffi::c_void = match init_attr_list(&mut attr_buf, true) {
            Some(_) => attr_buf.as_mut_ptr() as *mut core::ffi::c_void,
            None => {
                return Err("[sandbox] spawn 属性表初始化失败（fail-closed）".into());
            }
        };
        // 回填函数级 value 缓冲（P0-1）。
        inherit_handles = [out_w, err_w];
        job_handle_value = job;
        let ok_hlist = UpdateProcThreadAttribute(
            attr_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            inherit_handles.as_mut_ptr() as *const _,
            std::mem::size_of_val(&inherit_handles),
            std::ptr::null_mut(),
            std::ptr::null(),
        );
        let ok_job = if ok_hlist != 0 {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB,
                std::ptr::addr_of!(job_handle_value) as *const _,
                std::mem::size_of::<usize>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        } else {
            0
        };
        if ok_hlist == 0 || ok_job == 0 {
            let gle = windows_sys::Win32::Foundation::GetLastError();
            let which = if ok_hlist == 0 { "HANDLE_LIST" } else { "JOB" };
            DeleteProcThreadAttributeList(attr_list);
            return Err(format!("[sandbox] UpdateProcThreadAttribute({which}) 失败（GLE={gle}，fail-closed）"));
        }

        // ⑥ STARTUPINFOEX：USESTDHANDLES + 独立桌面。
        let mut si_ex: STARTUPINFOEXW = std::mem::zeroed();
        si_ex.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        si_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        si_ex.StartupInfo.hStdOutput = out_w;
        si_ex.StartupInfo.hStdError = err_w;
        si_ex.StartupInfo.hStdInput = std::ptr::null_mut(); // null stdin（INVALID_HANDLE_VALUE 会 GLE=87）
        if sbx_desktop().is_some() {
            let w = SBX_DESKTOP_W.get_or_init(|| {
                SBX_DESKTOP.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>().leak()
            });
            si_ex.StartupInfo.lpDesktop = w.as_ptr() as *mut _;
        }
        si_ex.lpAttributeList = attr_list;

        // ⑦ 环境块（先绑定变量防悬垂）与命令行（cwd_w 已在函数头前置构建）。
        let env_block = if env_overrides.is_empty() { None } else { Some(build_env_block(env_overrides)) };
        let env = match &env_block {
            Some(b) => b.as_ptr() as *const core::ffi::c_void,
            None => std::ptr::null(),
        };
        let mut cmdline = build_cmdline(program, args);

        let mut flags = CREATE_NO_WINDOW;
        if env_block.is_some() {
            // 环境块是 UTF-16：必须显式声明（缺省按 ANSI 解析遇内嵌 NUL → GLE=87；std 自动加此旗标）。
            flags |= CREATE_UNICODE_ENVIRONMENT;
        }
        flags |= EXTENDED_STARTUPINFO_PRESENT;

        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();
        // lpApplicationName 传 null：该参数不做 PATH 搜索（"cmd" 会 GLE=2）；
        // 搜索语义全在 lpCommandLine（PATH + PATHEXT 解析，对齐 std::process 行为）。
        let ok = CreateProcessAsUserW(
            token.0 as _,
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1, // bInheritHandles：管道写端必须可继承（HANDLE_LIST 精确限定继承面）
            flags,
            env,
            cwd_w.as_ptr(),
            &si_ex.StartupInfo,
            &mut pi,
        );
        if ok == 0 {
            let gle = windows_sys::Win32::Foundation::GetLastError();
            DeleteProcThreadAttributeList(attr_list);
            CloseHandle(out_r);
            CloseHandle(err_r);
            CloseHandle(out_w);
            CloseHandle(err_w);
            CloseHandle(h(job));
            // [spawn] 分类：普通启动失败（程序不存在 GLE=2 / cwd 无效 GLE=267 等）——
            // 不加沙箱话术，避免模型误归因隔离策略（红队2 P2-4）。
            return Err(format!(
                "[spawn] CreateProcessAsUserW({program}) 失败（GLE={gle}）——程序不存在或路径无效？"
            ));
        }

        DeleteProcThreadAttributeList(attr_list);
        // 父侧关闭管道写端（不关读端永不 EOF）。
        CloseHandle(out_w);
        CloseHandle(err_w);
        // 子进程线程句柄用完即关。
        CloseHandle(pi.hThread);

        let out_file = usize_to_file(out_r as usize);
        let err_file = usize_to_file(err_r as usize);
        Ok(WinChild::new(pi.dwProcessId, pi.hProcess as usize, Some(job), out_file, err_file))
    }
}

fn usize_to_file(h: usize) -> Option<std::fs::File> {
    use std::os::windows::io::FromRawHandle;
    if h == 0 {
        return None;
    }
    Some(unsafe { std::fs::File::from_raw_handle(h as _) })
}

/// 初始化属性表缓冲（返回 Some(size)=成功）。needs_job 决定预留 2 项还是 1 项。
fn init_attr_list(buf: &mut Vec<u64>, needs_job: bool) -> Option<usize> {
    unsafe {
        let count: u32 = if needs_job { 2 } else { 1 };
        let mut size: usize = 0;
        // 首调用预期失败并回填 size（ERROR_INSUFFICIENT_BUFFER）。
        let _ = InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &mut size);
        if size == 0 {
            return None;
        }
        let words = size.div_ceil(8);
        buf.resize(words, 0);
        if InitializeProcThreadAttributeList(buf.as_mut_ptr() as *mut _, count, 0, &mut size) == 0 {
            buf.clear();
            return None;
        }
        Some(size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_and_cmdline() {
        assert_eq!(quote_arg("plain"), "plain");
        assert_eq!(quote_arg(""), "\"\"");
        assert_eq!(quote_arg("a b"), "\"a b\"");
        let line = build_cmdline("cmd", &["/C".into(), "echo hi".into()]);
        assert_eq!(
            String::from_utf16(&line[..line.len() - 1]).unwrap(),
            "cmd /C \"echo hi\""
        );
    }

    #[test]
    fn env_block_overrides_replace_case_insensitive() {
        let block = build_env_block(&[("HTTP_PROXY".into(), "http://127.0.0.1:9".into())]);
        let s: String = String::from_utf16_lossy(&block[..block.len() - 1]);
        assert!(s.contains("HTTP_PROXY=http://127.0.0.1:9"), "{s}");
        assert!(s.to_uppercase().contains("PATH="));
    }
}
