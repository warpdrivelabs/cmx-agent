//! Linux OS 沙箱（方案 §五，S2）：Landlock 逐挂载规则矩阵 + 可选 seccomp 硬断网。
//!
//! 红队 B 组五个 P0 的落地：
//! 1. **规则矩阵**（B-1；口径修正——内核 `security/landlock/fs.c` 走 `follow_up()` **跨挂载
//!    向上遍历**至真实根，`/` 规则实际罩住全命名空间的只读/可执行；显式逐路径规则承担的是
//!    **写权利的精确授权**，非挂载隔离——原「mountinfo 探测挂载」表述按内核真实语义修正）：
//! 2. **ABI 分级**（B-2/B-3）：权利集按探测 ABI 掩码动态生成（REFER≥ABI2、TRUNCATE≥ABI3），
//!    不存在的权利绝不进 handled set（否则 EINVAL → fail-closed 全拒）；支持矩阵=ABI≥2
//!    （ABI1 无 REFER = 跨目录 rename/link 一律拒，npm 原子安装断——降级语义记录）；
//! 3. **NNP 前置**（B-4）：preexec 三步序列 `prctl(PR_SET_NO_NEW_PRIVS) → seccomp →
//!    landlock_restrict_self`，只设在 fork 出的 child 单线程，绝不装父线程（不可逆污染）；
//! 4. **io_uring 封堵**（B-5）：enforce 档 seccomp 除拒 INET socket 外一并禁
//!    `io_uring_setup/enter/register`（io_uring 可不经 socket(2) 建连，Codex 全模式禁）；
//! 5. **arch 校验**（B-7）：BPF 首指令校验 `AUDIT_ARCH_X86_64`，异构一律 ERRNO——
//!    防 32 位 ELF exec 整体绕过。
//!
//! Landlock 语义保证（方案 §5.2 写死）：execve 后仍生效、孙进程自动在域内、不可逆。
//! 元数据残余：chmod/chown/utimes 不受限（man page CAVEATS，DoS 级非泄露级，登记）。
//!
//! glibc 2.35（Ubuntu 22.04 构建环境）无 landlock wrapper → 全程 `libc::syscall` 裸调用。

#![cfg(target_os = "linux")]

use std::io;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use crate::ProcCtx;

/// x86_64 syscall 号（内核头；glibc 2.35 无 wrapper）。
const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;
const LANDLOCK_CREATE_RULESET_VERSION: libc::c_int = 1 << 0;

// —— Landlock 访问权利（ABI 矩阵）——
const LANDLOCK_ACCESS_FS_EXECUTE: u64 = 1 << 0;
const LANDLOCK_ACCESS_FS_WRITE_FILE: u64 = 1 << 1;
const LANDLOCK_ACCESS_FS_READ_FILE: u64 = 1 << 2;
const LANDLOCK_ACCESS_FS_READ_DIR: u64 = 1 << 3;
const LANDLOCK_ACCESS_FS_REMOVE_DIR: u64 = 1 << 4;
const LANDLOCK_ACCESS_FS_REMOVE_FILE: u64 = 1 << 5;
const LANDLOCK_ACCESS_FS_MAKE_CHAR: u64 = 1 << 6;
const LANDLOCK_ACCESS_FS_MAKE_DIR: u64 = 1 << 7;
const LANDLOCK_ACCESS_FS_MAKE_REG: u64 = 1 << 8;
const LANDLOCK_ACCESS_FS_MAKE_SOCK: u64 = 1 << 9;
const LANDLOCK_ACCESS_FS_MAKE_FIFO: u64 = 1 << 10;
const LANDLOCK_ACCESS_FS_MAKE_BLOCK: u64 = 1 << 11;
const LANDLOCK_ACCESS_FS_MAKE_SYM: u64 = 1 << 12;
const LANDLOCK_ACCESS_FS_REFER: u64 = 1 << 13; // ABI 2（内核 5.19）
const LANDLOCK_ACCESS_FS_TRUNCATE: u64 = 1 << 14; // ABI 3（内核 6.2）

/// 全量写权利（按 ABI 掩码裁剪后使用）。
fn all_write_rights(abi: u32) -> u64 {
    let mut m = LANDLOCK_ACCESS_FS_WRITE_FILE
        | LANDLOCK_ACCESS_FS_REMOVE_DIR
        | LANDLOCK_ACCESS_FS_REMOVE_FILE
        | LANDLOCK_ACCESS_FS_MAKE_CHAR
        | LANDLOCK_ACCESS_FS_MAKE_DIR
        | LANDLOCK_ACCESS_FS_MAKE_REG
        | LANDLOCK_ACCESS_FS_MAKE_SOCK
        | LANDLOCK_ACCESS_FS_MAKE_FIFO
        | LANDLOCK_ACCESS_FS_MAKE_BLOCK
        | LANDLOCK_ACCESS_FS_MAKE_SYM;
    if abi >= 2 {
        m |= LANDLOCK_ACCESS_FS_REFER; // 无 REFER = 跨目录 rename/link 一律拒（npm 原子安装断）
    }
    if abi >= 3 {
        m |= LANDLOCK_ACCESS_FS_TRUNCATE;
    }
    m
}

/// handled set（ruleset 层）：全权利 ∩ ABI。
fn handled_rights(abi: u32) -> u64 {
    LANDLOCK_ACCESS_FS_EXECUTE | LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR
        | all_write_rights(abi)
}

/// 探测 Landlock ABI（§3.3，红队 B-9：landlock_create_ruleset(VERSION)，非 prctl、非内核版本号——
/// lsm= 启动参数可整体禁用）。ENOSYS/EOPNOTSUPP = 不可用。
pub(crate) fn probe_landlock_abi() -> crate::probe::Availability {
    match landlock_abi() {
        Ok(abi) if abi >= 1 => crate::probe::Availability::Available,
        Ok(_) => crate::probe::Availability::Unavailable("Landlock ABI 0（内核未启用）".into()),
        Err(e) => crate::probe::Availability::Unavailable(format!("Landlock 不可用：{e}")),
    }
}

fn landlock_abi() -> Result<u32, String> {
    let r = unsafe {
        libc::syscall(SYS_LANDLOCK_CREATE_RULESET, std::ptr::null::<u64>(), 0usize, LANDLOCK_CREATE_RULESET_VERSION)
    };
    if r < 0 {
        Err(io::Error::last_os_error().to_string())
    } else {
        Ok(r as u32)
    }
}

/// landlock 规则结构（UAPI，repr(C) 布局对齐内核）。
#[repr(C)]
struct LandlockRulesetAttr {
    handled_access_fs: u64,
}
#[repr(C)]
struct LandlockPathBeneathAttr {
    allowed_access: u64,
    parent_fd: i32,
    reserved: u32,
}

/// 建好的沙箱计划：ruleset fd + 各 path 的 O_PATH fd（父进程预建——pre_exec 闭包零分配，B-6）。
struct LandlockPlan {
    ruleset_fd: i32,
    path_fds: Vec<(i32, u64)>, // (O_PATH fd, allowed_access)
}

impl Drop for LandlockPlan {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.ruleset_fd);
            for (fd, _) in &self.path_fds {
                libc::close(*fd);
            }
        }
    }
}

/// 逐挂载规则矩阵（§5.1）：以真实存在的路径开 O_PATH；mountinfo 探测的独立性由
/// 「每条规则各自的 O_PATH fd」天然承载（规则绑文件层级，同一 fd 不跨挂载）。
fn build_landlock_plan(roots: &[PathBuf], abi: u32) -> Result<LandlockPlan, String> {
    if abi < 1 {
        return Err(format!("Landlock ABI {abi} < 1"));
    }
    let handled = handled_rights(abi) & abi_mask(abi);
    let attr = LandlockRulesetAttr { handled_access_fs: handled };
    let fd = unsafe {
        libc::syscall(
            SYS_LANDLOCK_CREATE_RULESET,
            &attr as *const LandlockRulesetAttr,
            std::mem::size_of::<LandlockRulesetAttr>(),
            0,
        )
    };
    if fd < 0 {
        return Err(format!("landlock_create_ruleset 失败：{}", io::Error::last_os_error()));
    }
    let ruleset_fd = fd as i32;
    // 输入 path → allowed_access 矩阵（只读档与写档分开；`.git` deny 在 Linux 不落 ACL——
    // Landlock 无 deny 语义，`.git` 保护由「shell profile 把 .git 排除出放行集」实现：
    // 即对 roots 的写规则允许到 root，但 .git 目录单独挂只读规则（后加的更严规则生效于
    // ruleset 内所有规则取交集的语义下，Landlock 多规则 = 取允许并集——**无法做减法**，
    // 故 .git 只读语义用「root 不放行全树写，改为逐项放行」代价过高；v1.1 实测取舍：
    // Linux 侧不做 .git deny（Windows 专属语义），残差=Linux shell 可写 .git——
    // 登记进方案 §五 实施记录，与「chmod 元数据不受限」同类残余）。
    let ro = LANDLOCK_ACCESS_FS_READ_FILE | LANDLOCK_ACCESS_FS_READ_DIR;
    let write = all_write_rights(abi) & abi_mask(abi);
    let mut matrix: Vec<(PathBuf, u64)> = vec![
        (PathBuf::from("/"), ro | LANDLOCK_ACCESS_FS_EXECUTE),
        (PathBuf::from("/proc"), ro),
        (PathBuf::from("/sys"), ro),
        (PathBuf::from("/run"), ro),
        (PathBuf::from("/dev/null"), ro | LANDLOCK_ACCESS_FS_WRITE_FILE),
        (PathBuf::from("/dev/shm"), ro | write),
        (PathBuf::from("/tmp"), ro | write),
    ];
    if let Ok(x) = std::env::var("XDG_RUNTIME_DIR") {
        matrix.push((PathBuf::from(x), ro | write));
    }
    for r in crate::effective_write_roots(roots) {
        matrix.push((r, ro | write | LANDLOCK_ACCESS_FS_EXECUTE));
    }

    let mut path_fds = Vec::with_capacity(matrix.len());
    for (p, access) in matrix {
        if !p.exists() {
            continue; // 挂载点缺席（容器/精简系统）——不存在即无需授权
        }
        let c = path_to_cstring(&p)?;
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(format!("open(O_PATH) {} 失败：{}", p.display(), io::Error::last_os_error()));
        }
        path_fds.push((fd, access));
    }
    // 逐条 landlock_add_rule。
    for (fd, access) in &path_fds {
        let rule = LandlockPathBeneathAttr { allowed_access: *access, parent_fd: *fd, reserved: 0 };
        let r = unsafe {
            libc::syscall(
                SYS_LANDLOCK_ADD_RULE,
                ruleset_fd,
                LANDLOCK_RULE_PATH_BENEATH,
                &rule as *const LandlockPathBeneathAttr,
                0,
            )
        };
        if r < 0 {
            return Err(format!("landlock_add_rule 失败：{}", io::Error::last_os_error()));
        }
    }
    Ok(LandlockPlan { ruleset_fd, path_fds })
}

const LANDLOCK_RULE_PATH_BENEATH: libc::c_int = 1;

fn path_to_cstring(p: &Path) -> Result<std::ffi::CString, String> {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(p.as_os_str().as_bytes()).map_err(|e| format!("路径含内嵌 NUL：{e}"))
}

/// ABI 全权利掩码（哪些位在给定 ABI 上存在）。
fn abi_mask(abi: u32) -> u64 {
    let mut m = (1 << 13) - 1; // ABI 1：bit 0..12
    if abi >= 2 {
        m |= LANDLOCK_ACCESS_FS_REFER;
    }
    if abi >= 3 {
        m |= LANDLOCK_ACCESS_FS_TRUNCATE;
    }
    m
}

// —— seccomp（enforce 档；红队 B-5/B-7/B-8）——
const SECCOMP_RET_ALLOW: u32 = 0x7fff_0000;
const SECCOMP_RET_ERRNO: u32 = 0x0005_0000 | (libc::EPERM as u32 & 0xffff);
const AUDIT_ARCH_X86_64: u32 = 0xc000_003e;
const BPF_LD_W_ABS: u16 = 0x20;
const BPF_JMP_JEQ_K: u16 = 0x15; // BPF_JMP|BPF_JEQ|BPF_K（0x10 是 LD|B|IMM——内核 check_filter 直接 EINVAL，红队3 P0-1 实锚）
const BPF_RET_K: u16 = 0x06;
const SECCOMP_DATA_ARCH_OFF: u8 = 4;
const SECCOMP_DATA_NR_OFF: u8 = 0;
const SECCOMP_DATA_ARG0_OFF: u8 = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct SockFilter {
    code: u16,
    jt: u8,
    jf: u8,
    k: u32,
}
#[repr(C)]
struct SockFprog {
    len: u16,
    filter: *const SockFilter,
}

/// 手写最小 BPF（一页规则 + arch 校验；红队 B-7 的两道必答题：arch 校验与安装位置唯一性）：
/// 非 x86_64 → ERRNO；io_uring_setup/enter/register → ERRNO；socket 且 arg0 ∈ {AF_INET, AF_INET6} → ERRNO；
/// 其余 ALLOW。AF_UNIX 不动（本地 IPC）——127.0.0.1 一并断为**预期语义**（§3.4）。
fn build_seccomp_filter() -> Vec<SockFilter> {
    let mut f = Vec::new();
    // load arch; jeq x86_64 ? +1 : +0（不匹配走下一条 ERRNO；匹配跳过它）
    f.push(sf(BPF_LD_W_ABS, 0, 0, u32::from(SECCOMP_DATA_ARCH_OFF)));
    f.push(sf(BPF_JMP_JEQ_K, 1, 0, AUDIT_ARCH_X86_64));
    f.push(sf(BPF_RET_K, 0, 0, SECCOMP_RET_ERRNO));
    for nr in [libc::SYS_io_uring_setup, libc::SYS_io_uring_enter, libc::SYS_io_uring_register] {
        f.push(sf(BPF_LD_W_ABS, 0, 0, u32::from(SECCOMP_DATA_NR_OFF)));
        f.push(sf(BPF_JMP_JEQ_K, 0, 1, nr as u32)); // 命中→下一条 ERRNO；未命中跳过
        f.push(sf(BPF_RET_K, 0, 0, SECCOMP_RET_ERRNO));
    }
    // socket：先验 nr 再验 arg0 family。
    f.push(sf(BPF_LD_W_ABS, 0, 0, u32::from(SECCOMP_DATA_NR_OFF)));
    let not_socket_jump = f.len();
    f.push(sf(BPF_JMP_JEQ_K, 0, 0, libc::SYS_socket as u32)); // jf 稍后回填=跳过 socket 检查
    f.push(sf(BPF_LD_W_ABS, 0, 0, u32::from(SECCOMP_DATA_ARG0_OFF)));
    for fam in [libc::AF_INET, libc::AF_INET6] {
        f.push(sf(BPF_JMP_JEQ_K, 0, 1, fam as u32)); // 命中→下一条 ERRNO；未命中跳过它
        f.push(sf(BPF_RET_K, 0, 0, SECCOMP_RET_ERRNO));
    }
    let allow_idx = f.len();
    f.push(sf(BPF_RET_K, 0, 0, SECCOMP_RET_ALLOW));
    // 回填 socket nr 跳转：未命中 socket → 直达 ALLOW。
    f[not_socket_jump].jf = (allow_idx - not_socket_jump - 1) as u8;
    f
}

fn sf(code: u16, jt: u8, jf: u8, k: u32) -> SockFilter {
    SockFilter { code, jt, jf, k }
}

/// 装载 seccomp 过滤器（只应在 fork 后 exec 前的 child 单线程上下文调用——B-7 安装位置唯一性）。
/// 过滤器由**父进程预建**后 move 进来：pre_exec 闭包内零堆分配（红队3 P1-1，fork 后 malloc 死锁面）。
/// fprog 在 child 栈上现构（两字结构体，非堆分配）——闭包因此不捕获裸指针（pre_exec 要求
/// 闭包 Send+Sync，`*const SockFilter` 不满足，跨线程捕获编译即拒）。
fn install_seccomp(filter: &[SockFilter]) -> Result<(), String> {
    let mut prog = SockFprog { len: filter.len() as u16, filter: filter.as_ptr() };
    let r = unsafe {
        libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &mut prog as *mut SockFprog)
    };
    if r < 0 {
        return Err(format!("prctl(PR_SET_SECCOMP) 失败：{}", io::Error::last_os_error()));
    }
    Ok(())
}

// —— Linux 子进程（std Command + pre_exec；进程组杀树）——
pub(crate) struct LinuxChild {
    pid: u32,
    child: Arc<Mutex<Option<std::process::Child>>>,
}

impl LinuxChild {
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 阻塞：并发读管道 + 等退出（对齐 WinChild 语义；Child 单次取用，二次调用返回空）。
    pub fn read_wait(&self) -> (i32, Vec<u8>, Vec<u8>) {
        let mut guard = self.child.lock().expect("linux child lock");
        let Some(mut child) = guard.take() else {
            return (-1, Vec::new(), Vec::new());
        };
        drop(guard);
        let out = child.stdout.take();
        let err = child.stderr.take();
        let t_out = std::thread::spawn(move || {
            use std::io::Read;
            let mut b = Vec::new();
            if let Some(mut f) = out {
                let _ = f.read_to_end(&mut b);
            }
            b
        });
        let t_err = std::thread::spawn(move || {
            use std::io::Read;
            let mut b = Vec::new();
            if let Some(mut f) = err {
                let _ = f.read_to_end(&mut b);
            }
            b
        });
        let status = child.wait();
        let out = t_out.join().unwrap_or_default();
        let err = t_err.join().unwrap_or_default();
        (status.ok().and_then(|s| s.code()).unwrap_or(-1), out, err)
    }

    /// 整树终止：pre_exec 里 setpgid(0,0) → 杀进程组（孙进程一并）。
    pub fn terminate_tree(&self) {
        unsafe {
            libc::kill(-(self.pid as i32), libc::SIGKILL);
            libc::kill(self.pid as i32, libc::SIGKILL);
        }
    }
}

/// 主入口：std Command + pre_exec（B-6：父进程预建 ruleset/O_PATH fd，闭包零分配；
/// pre_exec 闭包内只跑裸 syscall：NNP → seccomp(仅 enforce) → landlock_restrict_self）。
/// 设置 pre_exec 后 std 自动回退 fork+exec（posix_spawn 无法执行闭包）。
pub(crate) fn spawn(
    ctx: &ProcCtx<'_>,
    program: &str,
    args: &[String],
    cwd: &Path,
    env_overrides: &[(String, String)],
) -> Result<LinuxChild, String> {
    let abi = landlock_abi()?;
    if abi < 2 {
        eprintln!("[sandbox] Landlock ABI {abi} < 2：跨目录 rename/link 一律拒（REFER 缺失），npm 原子安装类工作流将失败——建议 HWE 内核（≥5.19）");
    }
    let plan = build_landlock_plan(ctx.roots, abi)?;
    let enforce = crate::settings::get().net == crate::NetMode::Enforce;
    // seccomp 过滤器父进程预建（B-6：pre_exec 闭包零分配——fork 后 malloc 死锁面）。
    let filter = if enforce { build_seccomp_filter() } else { Vec::new() };
    let plan_fd = plan.ruleset_fd;

    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env_overrides {
        cmd.env(k, v);
    }
    // SAFETY（unsafe 源自闭包运行在 fork 后 child 单线程上下文）：闭包内零分配、零 libc 非
    // async-signal-safe 调用——fd 与标志全部预捕获值，仅三个裸 syscall。
    unsafe {
        cmd.pre_exec(move || {
            // ① 进程组自立（terminate_tree 杀组语义）。
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            // ② NNP（landlock/seccomp 共同前置；只设 child）。
            if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            // ③ seccomp（仅 enforce 档；过滤器已预建，闭包内零堆分配，fprog 栈上现构）。
            // 闭包只能回 io::Error——失败时把上下文编码进 errno 语义（EINVAL）+ errno 保留。
            if enforce
                && let Err(_e) = install_seccomp(&filter)
            {
                return Err(io::Error::from_raw_os_error(libc::EINVAL));
            }
            // ④ landlock_restrict_self（execve 后仍生效、孙进程自动在域内、不可逆）。
            if libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, plan_fd, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("受限 spawn {program} 失败（fail-closed）：{e}"))?;
    let pid = child.id();
    // 父侧补 setpgid（红队3 P2-6）：child 自立组前的极小窗口内 kill(-pid) 会 ESRCH——双侧对齐。
    unsafe { libc::setpgid(pid as i32, pid as i32) }; // 失败容忍（child 已自组时 EACCES/ESRCH 属预期）
    Ok(LinuxChild { pid, child: Arc::new(Mutex::new(Some(child))) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_matrix_and_handled_rights() {
        // ABI 1：无 REFER/TRUNCATE。
        assert_eq!(abi_mask(1), (1 << 13) - 1);
        assert_eq!(handled_rights(1) & LANDLOCK_ACCESS_FS_REFER, 0);
        assert_eq!(handled_rights(1) & LANDLOCK_ACCESS_FS_TRUNCATE, 0);
        // ABI 3：全量。
        assert_ne!(handled_rights(3) & LANDLOCK_ACCESS_FS_REFER, 0);
        assert_ne!(handled_rights(3) & LANDLOCK_ACCESS_FS_TRUNCATE, 0);
    }

    #[test]
    fn seccomp_filter_shape_and_arch_gate() {
        let f = build_seccomp_filter();
        // 首指令加载 arch；次指令校验 x86_64（**JEQ 操作码 0x15**——0x10 是 LD|B|IMM，
        // 内核 check_filter 白名单外直接 EINVAL，红队3 P0-1 回归锚）；第三条为异构 ERRNO。
        assert_eq!(f[0].code, BPF_LD_W_ABS);
        assert_eq!(f[1].code, 0x15, "JEQ 操作码必须是 BPF_JMP|BPF_JEQ|BPF_K=0x15");
        assert_eq!(f[1].k, AUDIT_ARCH_X86_64);
        assert_eq!(f[2].code, BPF_RET_K);
        // 末指令 ALLOW。
        assert_eq!(f.last().unwrap().k, SECCOMP_RET_ALLOW);
        // 长度合理（io_uring×3 + socket family×2 + 框架）。
        assert!(f.len() >= 12, "len={}", f.len());
    }
}
