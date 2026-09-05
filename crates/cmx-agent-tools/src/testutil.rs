//! 测试专用：全进程唯一的临时目录生成器。
//!
//! 各工具单测都需要隔离的临时工作区。此前用 `process_id + nanos`——同一测试二进制内多测试
//! **并行**运行时纳秒可能撞车 → 共享目录 → 一个测试的 `remove_dir_all` 误删另一个 → flaky。
//! 用一个**全局原子计数**保证单调唯一（单测试二进制单进程内绝不重复）。

#![cfg(test)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

/// 返回一个唯一且已创建的临时目录（`<tmp>/<prefix>-<pid>-<nanos>-<seq>`）。
pub(crate) fn unique_dir(prefix: &str) -> PathBuf {
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!("{prefix}-{}-{}-{}", std::process::id(), nanos, seq));
    std::fs::create_dir_all(&root).unwrap();
    root
}
