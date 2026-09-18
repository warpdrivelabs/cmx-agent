//! 桌面文件工具按工作目录解析相对路径，绝对路径不受工作目录限制。

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
async fn desktop_reads_file_outside_workspace_and_logs_result() {
    let work = TempDir::new("fs-work2");
    let data = TempDir::new("fs-data2");
    let outside = TempDir::new("fs-outside");
    let file = outside.path().join("notes.txt");
    std::fs::write(&file, b"outside workspace").unwrap();

    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "fs_read",
            serde_json::json!({ "path": file.to_str().unwrap() }),
        )]),
        ModelResponse::text("done"),
    ]));
    let app = DesktopAppBuilder::new(work.path(), data.path(), model)
        .build()
        .unwrap();
    let out = app.send("s1", "读取指定文件").await.unwrap();
    let read_ok = |e: &cmx_agent_core::event::SessionEvent| {
        matches!(&e.kind,
            EventKind::ToolResult { ok: true, output, .. }
                if output["text"].as_str().unwrap_or("").contains("outside workspace"))
    };
    assert!(out.new_events.iter().any(read_ok));
    assert!(app.get_events("s1").unwrap().iter().any(read_ok));
    assert!(!data.path().join("settings.json").exists());
    assert!(!data.path().join("sandbox_sid.json").exists());
}

#[tokio::test]
async fn desktop_auto_executes_shell_without_sandbox_metadata() {
    let work = TempDir::new("shell-work");
    let data = TempDir::new("shell-data");
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "shell-1",
            "shell",
            serde_json::json!({"cmd":"echo direct-execution"}),
        )]),
        ModelResponse::text("done"),
    ]));
    let app = DesktopAppBuilder::new(work.path(), data.path(), model)
        .approval(cmx_agent_core::ApprovalPolicy::Auto)
        .build()
        .unwrap();
    let out = app.send("shell-session", "执行命令").await.unwrap();
    let output = out.new_events.iter().find_map(|event| match &event.kind {
        EventKind::ToolResult { call_id, ok: true, output } if call_id == "shell-1" => Some(output),
        _ => None,
    }).expect("shell should execute under automatic approval");
    assert_eq!(output["exit_code"], 0);
    assert!(output["stdout"].as_str().unwrap().contains("direct-execution"));
    assert!(output.get("sandbox").is_none());
    assert!(out.new_events.iter().any(|event| matches!(
        &event.kind,
        EventKind::ApprovalResolved { approved: true, by, .. } if by == "policy:auto"
    )));
}

// ── 审批分级回归（工作目录内放行 / 目录外与命令类审批，沙箱移除后补位）──

use std::sync::atomic::{AtomicUsize, Ordering};

struct RejectAndCount(Arc<AtomicUsize>);
#[async_trait::async_trait]
impl cmx_agent_core::Approver for RejectAndCount {
    async fn resolve(&self, _c: &ToolCall, _r: &str) -> (bool, String) {
        self.0.fetch_add(1, Ordering::SeqCst);
        (false, "audit-reject".into())
    }
}

fn write_case_model(path: &str) -> Arc<MockModel> {
    Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "w1",
            "fs_write",
            serde_json::json!({"path": path, "content": "after"}),
        )]),
        ModelResponse::text("done"),
    ]))
}

#[tokio::test]
async fn in_workspace_write_skips_approval_but_outside_requires_it() {
    let work = TempDir::new("ws-approve");
    let data = TempDir::new("ws-approve-data");
    let outside = TempDir::new("ws-approve-out");
    let secret = outside.path().join("outside.txt");
    std::fs::write(&secret, "before").unwrap();
    for (name, path, expect_approvals) in [
        ("inside", "notes.txt", 0usize),
        ("outside-rel", "../outside.txt", 1),
        ("outside-abs", secret.to_str().unwrap(), 1),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let app = DesktopAppBuilder::new(work.path(), data.path(), write_case_model(path))
            .approver(Arc::new(RejectAndCount(calls.clone())))
            .build()
            .unwrap();
        let _ = app.send(name, "写文件").await.unwrap();
        assert_eq!(
            calls.load(Ordering::SeqCst),
            expect_approvals,
            "用例 {name}：目录内写免审批、目录外写需审批"
        );
    }
    assert_eq!(
        std::fs::read_to_string(&secret).unwrap(),
        "before",
        "审批被拒后目录外文件不得被改动"
    );
    assert!(work.path().join("notes.txt").exists(), "目录内写已执行");
}

#[tokio::test]
async fn run_tests_custom_command_requires_approval_like_shell() {
    let work = TempDir::new("cmd-approve");
    let data = TempDir::new("cmd-approve-data");
    let calls = Arc::new(AtomicUsize::new(0));
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "t1",
            "run_tests",
            serde_json::json!({"command": "echo marker"}),
        )]),
        ModelResponse::text("done"),
    ]));
    let app = DesktopAppBuilder::new(work.path(), data.path(), model)
        .approver(Arc::new(RejectAndCount(calls.clone())))
        .build()
        .unwrap();
    let out = app.send("s-cmd", "跑命令").await.unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "自定义命令须与 shell 同级审批");
    assert!(out.new_events.iter().any(|e| matches!(
        &e.kind,
        EventKind::ToolResult { ok: false, .. }
    )));
}

#[tokio::test]
async fn unattended_auto_approves_commands_but_rejects_out_of_workspace_write() {
    let work = TempDir::new("auto-ws");
    let data = TempDir::new("auto-ws-data");
    // 命令（Conditional 非高危）：Auto 自动批准执行。
    let cmd_model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "a1",
            "run_tests",
            serde_json::json!({"command": "echo auto-ok"}),
        )]),
        ModelResponse::text("done"),
    ]));
    let app = DesktopAppBuilder::new(work.path(), data.path(), cmd_model)
        .approval(cmx_agent_core::ApprovalPolicy::Auto)
        .build()
        .unwrap();
    let out = app.send("s-auto-cmd", "跑").await.unwrap();
    let cmd_ok = out.new_events.iter().any(|e| matches!(
        &e.kind,
        EventKind::ToolResult { ok: true, output, .. }
            if output["result"]["stdout"].as_str().unwrap_or("").contains("auto-ok")
    ));
    assert!(cmd_ok, "Auto 应自动批准普通命令：{:?}", out.new_events.iter().map(|e| &e.kind).collect::<Vec<_>>());

    // 越出工作目录的写（requires_approval=Never，仅靠 WorkspaceWriteGuard 升级）：Auto 拒绝。
    let outside = TempDir::new("auto-ws-out");
    let target = outside.path().join("x.txt");
    std::fs::write(&target, "before").unwrap();
    let write_model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "a2",
            "fs_write",
            serde_json::json!({"path": target.to_str().unwrap(), "content": "after"}),
        )]),
        ModelResponse::text("done"),
    ]));
    let app2 = DesktopAppBuilder::new(work.path(), data.path(), write_model)
        .approval(cmx_agent_core::ApprovalPolicy::Auto)
        .build()
        .unwrap();
    let out2 = app2.send("s-auto-write", "写").await.unwrap();
    assert!(out2.new_events.iter().any(|e| matches!(
        &e.kind,
        EventKind::ApprovalResolved { approved: false, by, .. } if by == "policy:auto"
    )), "无人值守不得自动批准越界写");
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "before");
}

#[tokio::test]
async fn plan_mode_from_home_create_then_set_is_effective() {
    let work = TempDir::new("plan-home");
    let data = TempDir::new("plan-home-data");
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "p1",
            "fs_write",
            serde_json::json!({"path": "x.txt", "content": "x"}),
        )]),
        ModelResponse::text("done"),
    ]));
    let calls = Arc::new(AtomicUsize::new(0));
    let app = DesktopAppBuilder::new(work.path(), data.path(), model)
        .approver(Arc::new(RejectAndCount(calls.clone())))
        .build()
        .unwrap();
    // 首页新会话序：先 create（get-or-create）再 set_plan_mode —— set_plan_mode 对不存在的
    // 会话报 NotFound，旧序静默失败导致「以为在计划模式、实际不是」（确定性缺口回归锚）。
    let created = app.create_session("task-plan-home");
    assert!(created.is_ok(), "create_session 应先于 set_plan_mode 成功");
    app.set_plan_mode("task-plan-home", true).await.unwrap();
    let out = app.send("task-plan-home", "写点东西").await.unwrap();
    assert!(
        out.new_events.iter().any(|e| matches!(
            &e.kind,
            EventKind::ToolResult { ok: false, output, .. }
                if output["error"].as_str().unwrap_or("").contains("计划模式")
        )),
        "计划模式下 fs_write 应被白名单拒绝"
    );
}
