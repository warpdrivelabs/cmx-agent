//! 桌面壳「本地文件」能力的端到端测试：通过完整 app 栈让模型调 fs_read，验证
//! 沙箱根=工作区（工作区内可读、工作区外拒绝），且全过程落库可回放。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::DesktopAppBuilder;
use cmx_agent_core::event::EventKind;
use cmx_agent_core::{MockModel, ModelResponse, ToolCall};

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-agent-{tag}-{n}"));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

#[tokio::test]
async fn desktop_reads_file_within_workspace() {
    let work = TempDir::new("fs-work");
    let data = TempDir::new("fs-data");
    // 工作区内放一个文件
    let file = work.path().join("notes.txt");
    std::fs::write(&file, b"quarterly numbers look good").unwrap();

    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "fs_read",
            serde_json::json!({ "path": file.to_str().unwrap() }),
        )]),
        ModelResponse::text("已读取"),
    ]));
    let app = DesktopAppBuilder::new(work.path(), data.path(), model)
        .build()
        .unwrap();
    let out = app.send("s1", "读一下 notes.txt").await.unwrap();

    let read_ok = out.new_events.iter().any(|e| {
        matches!(&e.kind,
        EventKind::ToolResult { ok: true, output, .. }
            if output["text"].as_str().unwrap_or("").contains("quarterly numbers"))
    });
    assert!(read_ok, "file within workspace must be readable");
}

#[tokio::test]
async fn desktop_blocks_read_outside_workspace() {
    let work = TempDir::new("fs-work2");
    let data = TempDir::new("fs-data2");
    // 在工作区**之外**放一个机密文件
    let outside = TempDir::new("fs-outside");
    let secret = outside.path().join("secret.txt");
    std::fs::write(&secret, b"TOP SECRET").unwrap();

    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "fs_read",
            serde_json::json!({ "path": secret.to_str().unwrap() }),
        )]),
        ModelResponse::text("done"),
    ]));
    let app = DesktopAppBuilder::new(work.path(), data.path(), model)
        .build()
        .unwrap();
    let out = app.send("s1", "读机密").await.unwrap();

    let blocked = out.new_events.iter().any(|e| {
        matches!(&e.kind,
        EventKind::ToolResult { ok: false, output, .. }
            if output["error"].as_str().unwrap_or("").contains("escapes sandbox"))
    });
    assert!(
        blocked,
        "file outside workspace must be denied by sandbox root"
    );
}
