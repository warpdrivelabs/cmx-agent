//! Windows Job Object（Windows 适配方案 P1.5）：kill-on-close + 超时 TerminateJobObject，
//! 把整棵进程树一并收尸。
//!
//! 背景：`Child::start_kill()` 只杀直接子进程；`sh -c <命令>` 里的真命令是孙进程，
//! 超时后被孤儿化留在系统里占端口、锁文件。Job Object 的 kill-on-close / Terminate
//! 覆盖整棵树；句柄以 usize 持有（HANDLE 裸指针会让 future 变 !Send）。
#![cfg(windows)]

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    SetInformationJobObject, TerminateJobObject,
};

/// 持有一个 Job 句柄；Drop 时关闭（配合 KILL_ON_JOB_CLOSE，句柄一关整树即收尸）。
pub(crate) struct JobHandle(usize);

impl JobHandle {
    /// 终止作业内全部进程（超时收尸用；随后再关句柄）。
    pub fn terminate(&self) {
        unsafe { TerminateJobObject(self.0 as _, 1) };
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0 as _) };
    }
}

/// 给子进程挂一个 kill-on-close 作业。失败返回 None（降级为旧行为：只杀直接子进程），
/// 绝不让沙箱工程问题打断命令执行。
pub(crate) fn attach_child_job(child_raw_handle: usize) -> Option<JobHandle> {
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        // KILL_ON_JOB_CLOSE：本进程退出（句柄全关）或显式 terminate/关闭句柄时，作业内进程树全灭。
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 || AssignProcessToJobObject(job, child_raw_handle as _) == 0 {
            CloseHandle(job);
            return None;
        }
        Some(JobHandle(job as usize))
    }
}
