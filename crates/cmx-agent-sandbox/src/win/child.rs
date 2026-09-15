//! 受限子进程的阻塞执行模型（`read_wait` / `terminate_tree`，均为 `&self`——Arc 内核支持
//! 超时杀树与阻塞读双线程并存）。
//!
//! 供 tools/proc.rs 的 `spawn_blocking` + `tokio::time::timeout` 适配：本类型无 tokio 依赖。
//! 超时路径从另一线程调 [`ChildShared::terminate`]（Job Terminate 整树 + 进程 Terminate 兜底）；
//! 句柄以 usize 持有（`HANDLE` 裸指针会让类型 !Send——对齐 tools/job.rs 的既有做法），
//! 最后一个 Arc 归零时 Drop 关句柄。

#![cfg(windows)]

use std::fs::File;
use std::io::Read;
use std::sync::{Arc, Mutex};

use windows_sys::Win32::System::JobObjects::TerminateJobObject;
use windows_sys::Win32::System::Threading::{GetExitCodeProcess, TerminateProcess, WaitForSingleObject};

/// 进程/Job 句柄的共享内核（超时杀树用；跨线程安全——HANDLE 进程级有效，CloseHandle 仅在 Drop）。
pub(crate) struct ChildShared {
    process: usize,
    job: Option<usize>,
}

impl ChildShared {
    /// 整树终止：Job Terminate（kill-on-close 语义下整棵树）优先，进程 Terminate 兜底。
    /// 与 [`WinChild::read_wait`] 并发调用安全（互不 close 句柄）。
    pub fn terminate(&self) {
        unsafe {
            if let Some(j) = self.job {
                TerminateJobObject(j as _, 1);
            }
            TerminateProcess(self.process as _, 1);
        }
    }
}

impl Drop for ChildShared {
    fn drop(&mut self) {
        unsafe {
            // Job 带 KILL_ON_JOB_CLOSE：关句柄即整树收尸（含孙进程）。
            if let Some(j) = self.job {
                windows_sys::Win32::Foundation::CloseHandle(j as _);
            }
            windows_sys::Win32::Foundation::CloseHandle(self.process as _);
        }
    }
}

/// 一个受 OS 沙箱约束的子进程。stdout/stderr 是父侧读端（File，经 Mutex 单次取用）。
pub(crate) struct WinChild {
    pid: u32,
    shared: Arc<ChildShared>,
    pipes: Arc<Mutex<Option<(File, File)>>>,
}

impl WinChild {
    pub(crate) fn new(
        pid: u32,
        process: usize,
        job: Option<usize>,
        out_read: Option<File>,
        err_read: Option<File>,
    ) -> Self {
        Self {
            pid,
            shared: Arc::new(ChildShared { process, job }),
            pipes: Arc::new(Mutex::new(out_read.zip(err_read))),
        }
    }

    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// 阻塞：并发读 stdout/stderr 至 EOF（读线程，防单管道写满死锁——对齐 proc.rs 语义），
    /// 再等进程退出。返回 (exit_code, stdout, stderr)。管道单次取用（二次调用返回空）。
    /// 超时由外层控制：外层先 `terminate_tree()`，随后本调用自然收敛（进程亡→句柄关→管道 EOF）。
    pub fn read_wait(&self) -> (i32, Vec<u8>, Vec<u8>) {
        let pipes = self.pipes.lock().expect("sandbox child pipes").take();
        let (out_buf, err_buf) = read_both(pipes);
        unsafe {
            WaitForSingleObject(self.shared.process as _, u32::MAX); // INFINITE：读尽后进程必已退或即将退
            let mut code: u32 = u32::MAX;
            GetExitCodeProcess(self.shared.process as _, &mut code);
            (code as i32, out_buf, err_buf)
        }
    }

    /// 整树终止（转发共享内核）。
    pub fn terminate_tree(&self) {
        self.shared.terminate();
    }
}

/// 两条管道并发读（各自线程 read_to_end；管道已取空（二次调用）则直接返回空缓冲）。
fn read_both(pipes: Option<(File, File)>) -> (Vec<u8>, Vec<u8>) {
    let (out, err) = match pipes {
        Some((o, e)) => (Some(o), Some(e)),
        None => (None, None),
    };
    let t_out = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut f) = out {
            let _ = f.read_to_end(&mut buf);
        }
        buf
    });
    let t_err = std::thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut f) = err {
            let _ = f.read_to_end(&mut buf);
        }
        buf
    });
    let out_buf = t_out.join().unwrap_or_default();
    let err_buf = t_err.join().unwrap_or_default();
    (out_buf, err_buf)
}
