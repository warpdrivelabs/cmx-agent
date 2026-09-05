//! 共享子进程执行器（bash / git / run_tests 复用）：cwd 限定 + 超时 + 输出截断 + 并发读防死锁。

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::AsyncReadExt;

pub const MAX_OUT: usize = 64 * 1024;
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;
pub const MAX_TIMEOUT_MS: u64 = 120_000;

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

/// 运行 `program args...`，cwd=`cwd`，超时 `timeout_ms`。返回结构化 JSON（exit_code/stdout/stderr/…）。
/// 永不 Err——错误也编码进返回值，供模型自愈。
pub async fn run(program: &str, args: &[String], cwd: &Path, timeout_ms: u64) -> Value {
    let timeout_ms = timeout_ms.clamp(1, MAX_TIMEOUT_MS);
    let mut child = match tokio::process::Command::new(program)
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return json!({"ok": false, "error": format!("启动 {program} 失败: {e}")}),
    };
    let mut out = String::new();
    let mut err = String::new();
    let mut so = child.stdout.take();
    let mut se = child.stderr.take();
    let read_fut = async {
        if let Some(s) = so.as_mut() {
            let _ = s.read_to_string(&mut out).await;
        }
        if let Some(s) = se.as_mut() {
            let _ = s.read_to_string(&mut err).await;
        }
        child.wait().await
    };
    match tokio::time::timeout(Duration::from_millis(timeout_ms), read_fut).await {
        Ok(Ok(status)) => {
            let (out, ot) = truncate(out);
            let (err, et) = truncate(err);
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
            let _ = child.start_kill();
            let (out, _) = truncate(out);
            let (err, _) = truncate(err);
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
