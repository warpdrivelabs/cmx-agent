//! 工具单元测试，重点：fs_read 的沙箱路径围栏（防 `../` 逃逸）与高危工具的守卫标注正确性。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_core::tool::Approval;
use cmx_agent_core::{SandboxMode, Tool, ToolCtx};
use cmx_agent_tools::{AddTool, ClockTool, DangerRmTool, EchoTool, FsReadTool};

fn ctx_with_roots(roots: Vec<PathBuf>) -> (SandboxMode, Vec<PathBuf>) {
    (SandboxMode::WorkspaceWrite, roots)
}

#[tokio::test]
async fn echo_roundtrips_text() {
    let (sb, roots) = ctx_with_roots(vec![]);
    let ctx = ToolCtx {
         sandbox: sb,
         allowed_roots: &roots,
         session_id: "test",
     };
    let r = EchoTool
        .invoke(serde_json::json!({"text":"hello"}), &ctx)
        .await
        .unwrap();
    assert!(r.ok);
    assert_eq!(r.output["text"], "hello");
}

#[tokio::test]
async fn add_handles_missing_args_gracefully() {
    let (sb, roots) = ctx_with_roots(vec![]);
    let ctx = ToolCtx {
         sandbox: sb,
         allowed_roots: &roots,
         session_id: "test",
     };
    let ok = AddTool
        .invoke(serde_json::json!({"a":2,"b":3}), &ctx)
        .await
        .unwrap();
    assert_eq!(ok.output["sum"], 5.0);
    let bad = AddTool
        .invoke(serde_json::json!({"a":2}), &ctx)
        .await
        .unwrap();
    assert!(
        !bad.ok,
        "missing 'b' must yield an error result, not a panic"
    );
}

#[tokio::test]
async fn clock_fixed_is_deterministic() {
    let t = chrono::DateTime::parse_from_rfc3339("2026-09-02T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let clock = ClockTool::fixed(t);
    let (sb, roots) = ctx_with_roots(vec![]);
    let ctx = ToolCtx {
         sandbox: sb,
         allowed_roots: &roots,
         session_id: "test",
     };
    let r = clock.invoke(serde_json::json!({}), &ctx).await.unwrap();
    assert_eq!(r.output["now"], "2026-09-02T00:00:00+00:00");
}

#[tokio::test]
async fn fs_read_denies_empty_roots() {
    let (sb, roots) = ctx_with_roots(vec![]);
    let ctx = ToolCtx {
         sandbox: sb,
         allowed_roots: &roots,
         session_id: "test",
     };
    let r = FsReadTool
        .invoke(serde_json::json!({"path":"/etc/hosts"}), &ctx)
        .await
        .unwrap();
    assert!(!r.ok, "no allowed_roots must deny all fs access");
}

#[tokio::test]
async fn fs_read_reads_within_root() {
    let dir = std::env::temp_dir().join(format!("cmx-agent-test-{}", uuid_like()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("hello.txt");
    std::fs::write(&file, b"cmx-agent works").unwrap();

    let roots = vec![dir.clone()];
    let ctx = ToolCtx {
         sandbox: SandboxMode::WorkspaceWrite,
         allowed_roots: &roots,
         session_id: "test",
     };
    let r = FsReadTool
        .invoke(serde_json::json!({"path": file.to_str().unwrap()}), &ctx)
        .await
        .unwrap();
    assert!(
        r.ok,
        "reading within allowed root must succeed: {:?}",
        r.output
    );
    assert!(
        r.output["text"]
            .as_str()
            .unwrap()
            .contains("cmx-agent works")
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn fs_read_blocks_parent_dir_escape() {
    let base = std::env::temp_dir().join(format!("cmx-agent-jail-{}", uuid_like()));
    let inside = base.join("inside");
    std::fs::create_dir_all(&inside).unwrap();
    // 在 base 的**兄弟**处放一个"机密"文件，尝试用 ../ 逃逸读取
    let secret = base.join("secret.txt");
    std::fs::write(&secret, b"TOP SECRET").unwrap();

    let roots = vec![inside.clone()]; // 只允许 inside/
    let ctx = ToolCtx {
         sandbox: SandboxMode::WorkspaceWrite,
         allowed_roots: &roots,
         session_id: "test",
     };
    let escape = format!("{}/../secret.txt", inside.to_str().unwrap());
    let r = FsReadTool
        .invoke(serde_json::json!({"path": escape}), &ctx)
        .await
        .unwrap();
    assert!(!r.ok, "../ escape out of allowed root must be denied");
    assert!(
        r.output["error"]
            .as_str()
            .unwrap()
            .contains("escapes sandbox")
    );

    std::fs::remove_dir_all(&base).ok();
}

#[test]
fn danger_rm_is_flagged_high_risk_and_always_approval() {
    let spec = DangerRmTool.spec();
    assert!(spec.guard.high_risk, "danger_rm must be flagged high-risk");
    assert_eq!(spec.guard.requires_approval, Approval::Always);
    assert!(!spec.guard.idempotent);
}

#[test]
fn default_registry_exposes_all_tools_sorted() {
    let reg = cmx_agent_tools::default_registry();
    let names: Vec<String> = reg.specs().into_iter().map(|s| s.name).collect();
    // 基础工具 + 编码面（E1）+ 编码面进阶（E2）工具，按名排序。
    assert_eq!(
        names,
        vec![
            "add",
            "apply_patch",
            "chart",
            "clock",
            "danger_rm",
            "data_describe",
            "echo",
            "fs_edit",
            "fs_read",
            "fs_write",
            "git",
            "glob",
            "grep",
            "repo_map",
            "run_tests",
            "shell",
            "update_plan",
        ]
    );
    // 每个工具都能被路由取到
    for n in &names {
        assert!(reg.contains(n));
    }
    // Arc 化取用不 panic
    assert!(reg.get("add").is_some());
    let _ = Arc::new(EchoTool); // 触发 Arc 用法覆盖
}

/// 简易唯一后缀（避免引 uuid 到 dev-deps 之外的地方；这里只需目录名唯一）。
fn uuid_like() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos()
}
