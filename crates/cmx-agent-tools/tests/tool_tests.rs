//! 工具集成回归：普通工作目录解析、目录外文件访问与守卫标注。

use std::path::PathBuf;

use cmx_agent_core::tool::Approval;
use cmx_agent_core::{Tool, ToolCtx};
use cmx_agent_tools::{
    AddTool, ChartTool, ClockTool, DangerRmTool, DataDescribeTool, EchoTool, FsEditTool,
    FsReadTool, FsWriteTool, GlobTool, GrepTool, RepoMapTool,
};
use serde_json::json;

fn tmp() -> PathBuf {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("cmx-tools-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn echo_roundtrips_text() {
    let ctx = ToolCtx {
        workspace_roots: &[],
        session_id: "test",
    };
    let r = EchoTool
        .invoke(json!({"text":"hello"}), &ctx)
        .await
        .unwrap();
    assert!(r.ok);
    assert_eq!(r.output["text"], "hello");
}

#[tokio::test]
async fn add_handles_missing_args_gracefully() {
    let ctx = ToolCtx {
        workspace_roots: &[],
        session_id: "test",
    };
    let ok = AddTool.invoke(json!({"a":2,"b":3}), &ctx).await.unwrap();
    assert_eq!(ok.output["sum"], 5.0);
    let bad = AddTool.invoke(json!({"a":2}), &ctx).await.unwrap();
    assert!(!bad.ok);
}

#[tokio::test]
async fn clock_fixed_is_deterministic() {
    let t = chrono::DateTime::parse_from_rfc3339("2026-09-02T00:00:00Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let ctx = ToolCtx {
        workspace_roots: &[],
        session_id: "test",
    };
    let r = ClockTool::fixed(t).invoke(json!({}), &ctx).await.unwrap();
    assert_eq!(r.output["now"], "2026-09-02T00:00:00+00:00");
}

#[tokio::test]
async fn file_tools_read_write_and_edit_outside_workspace() {
    let base = tmp();
    let roots = vec![base.join("workspace")];
    std::fs::create_dir_all(&roots[0]).unwrap();
    let ctx = ToolCtx {
        workspace_roots: &roots,
        session_id: "test",
    };
    let file = base.join("outside.txt");
    let r = FsWriteTool
        .invoke(json!({"path":file,"content":"before"}), &ctx)
        .await
        .unwrap();
    assert!(r.ok, "{r:?}");
    let r = FsEditTool
        .invoke(
            json!({"path":"../outside.txt","old_string":"before","new_string":"after"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(r.ok, "{r:?}");
    let r = FsReadTool.invoke(json!({"path":file}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["text"], "after");
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn relative_file_paths_use_first_workspace_root() {
    let base = tmp();
    let roots = vec![base.join("first"), base.join("second")];
    std::fs::create_dir_all(&roots[0]).unwrap();
    std::fs::create_dir_all(&roots[1]).unwrap();
    std::fs::write(roots[1].join("value.txt"), "second").unwrap();
    let ctx = ToolCtx {
        workspace_roots: &roots,
        session_id: "test",
    };
    let r = FsWriteTool
        .invoke(json!({"path":"value.txt","content":"first"}), &ctx)
        .await
        .unwrap();
    assert!(r.ok, "{r:?}");
    let r = FsReadTool
        .invoke(json!({"path":"value.txt"}), &ctx)
        .await
        .unwrap();
    assert_eq!(r.output["text"], "first");
    assert_eq!(
        std::fs::read_to_string(roots[1].join("value.txt")).unwrap(),
        "second"
    );
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn searches_and_maps_accept_directories_outside_workspace() {
    let base = tmp();
    let roots = vec![base.join("workspace")];
    let outside = base.join("outside");
    std::fs::create_dir_all(&roots[0]).unwrap();
    std::fs::create_dir_all(outside.join(".git")).unwrap();
    std::fs::write(outside.join(".git/marker.txt"), "outside-marker").unwrap();
    let ctx = ToolCtx {
        workspace_roots: &roots,
        session_id: "test",
    };
    for path in ["../outside", outside.to_str().unwrap()] {
        let r = GlobTool
            .invoke(json!({"path":path,"pattern":"**/*.txt"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["count"], 1);
        let r = GrepTool
            .invoke(json!({"path":path,"pattern":"outside-marker"}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["count"], 1);
        let r = RepoMapTool
            .invoke(json!({"path":path,"include_ignored":true}), &ctx)
            .await
            .unwrap();
        assert!(r.ok, "{r:?}");
        assert!(r.output["tree"].as_str().unwrap().contains("marker.txt"));
    }
    // 无工作根也能以显式绝对路径搜索，不扫描测试目录之外的文件。
    let ctx = ToolCtx {
        workspace_roots: &[],
        session_id: "test",
    };
    let r = GlobTool
        .invoke(json!({"path":outside,"pattern":"**/*.txt"}), &ctx)
        .await
        .unwrap();
    assert_eq!(r.output["count"], 1);
    std::fs::remove_dir_all(base).unwrap();
}

#[tokio::test]
async fn data_and_chart_tools_access_files_outside_workspace() {
    let base = tmp();
    let roots = vec![base.join("workspace")];
    std::fs::create_dir_all(&roots[0]).unwrap();
    std::fs::write(base.join("data.csv"), "name,value\na,3\n").unwrap();
    let ctx = ToolCtx {
        workspace_roots: &roots,
        session_id: "test",
    };
    let r = DataDescribeTool
        .invoke(json!({"path":"../data.csv"}), &ctx)
        .await
        .unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["rows"], 1);
    let file = base.join("chart.svg");
    let r = ChartTool
        .invoke(
            json!({"type":"bar","labels":["a"],"series":[{"values":[3]}],"save_as":file}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(r.ok, "{r:?}");
    assert!(std::fs::read_to_string(file).unwrap().starts_with("<svg"));
    std::fs::remove_dir_all(base).unwrap();
}

#[test]
fn danger_rm_is_flagged_high_risk_and_always_approval() {
    let spec = DangerRmTool.spec();
    assert!(spec.guard.high_risk);
    assert_eq!(spec.guard.requires_approval, Approval::Always);
    assert!(!spec.guard.idempotent);
}

#[test]
fn default_registry_exposes_all_tools_sorted() {
    let reg = cmx_agent_tools::default_registry();
    let names: Vec<String> = reg.specs().into_iter().map(|s| s.name).collect();
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
    for n in &names {
        assert!(reg.contains(n));
    }
}
