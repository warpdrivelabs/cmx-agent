//! 全工具扫描（smoke sweep）：用 `DesktopAppBuilder` 装配的**完整注册表**逐工具调用。
//!
//! 两档断言：①所有工具可路由、可调用、不 panic、返回结构化结果（缺参时给出可行动错误也算通过
//! ——证明注册与执行链路通）；②核心业务工具（shell/fs 读写/办公三件套/run_tests）用真实输入
//! 做端到端往返验证。联网/LSP/连接器类在无配置/无服务时按设计降级，标注即可。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::DesktopAppBuilder;
use cmx_agent_core::MockModel;
use cmx_agent_core::ToolCtx;
use serde_json::{Value, json};

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-sweep-{tag}-{n}"));
        std::fs::create_dir_all(p.join("workspace")).unwrap();
        std::fs::create_dir_all(p.join("data")).unwrap();
        TempDir(p)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// 真实样例输入（按依赖排序：先写后读）。键外的工具用 `{}` 探活。
fn sample_inputs() -> Vec<(&'static str, Value)> {
    vec![
        ("echo", json!({"text": "sweep"})),
        ("add", json!({"a": 2, "b": 3})),
        ("clock", json!({})),
        (
            "shell",
            json!({"cmd": "echo toolsweep-ok", "timeout_ms": 15000}),
        ),
        ("fs_write", json!({"path": "notes.md", "content": "hello sweep 量化"})),
        ("fs_read", json!({"path": "notes.md"})),
        ("grep", json!({"pattern": "sweep", "path": "."})),
        ("glob", json!({"pattern": "**/*.md"})),
        (
            "xlsx_write",
            json!({"path": "demo.xlsx", "sheets": [{"name": "S", "rows": [["a", 1], ["b", 2]]}]}),
        ),
        (
            "pptx_write",
            json!({"path": "demo.pptx", "slides": [{"title": "扫描页", "bullets": ["要点一", "要点二"]}]}),
        ),
        ("doc_read", json!({"path": "demo.pptx"})),
        ("doc_read_xlsx", json!({"path": "demo.xlsx"})), // 特例：同名工具第二次调用（见下）
        ("run_tests", json!({"command": "echo tests-ok", "timeout_ms": 20000})),
        ("repo_map", json!({"path": "."})),
    ]
}

#[tokio::test]
async fn all_tools_sweep() {
    let tmp = TempDir::new("app");
    let ws = tmp.0.join("workspace");
    let data = tmp.0.join("data");
    let app = DesktopAppBuilder::new(ws.clone(), data, Arc::new(MockModel::new([])))
        .build()
        .unwrap();
    let reg = app.agent().tools();
    let all_names: Vec<String> = reg.specs().into_iter().map(|s| s.name).collect();
    println!("注册表共 {} 个工具：{:?}", all_names.len(), all_names);
    assert!(all_names.len() >= 20, "完整注册表应 ≥20 个工具");

    let roots = vec![ws.clone()];
    let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
    let mut results: Vec<(String, bool, String)> = Vec::new();

    // —— ① 真实样例：核心链路端到端 ——
    for (name, input) in sample_inputs() {
        // 特例：doc_read 读 xlsx 复用同名工具，手动分发。
        let real_name = name.strip_suffix("_xlsx").unwrap_or(name);
        let tool = reg.get(real_name).expect("样例工具应在注册表");
        let r = tool.invoke(input, &ctx).await.expect("工具调用永不 Err");
        let note = if r.ok {
            serde_json::to_string(&r.output).unwrap_or_default()
        } else {
            r.output.to_string()
        };
        let display: String = note.chars().take(160).collect();
        let _ = display;
        println!("  {real_name:<14} {}（{} chars）", if r.ok { "✓" } else { "✗" }, note.chars().count());
        results.push((real_name.to_string(), r.ok, note));
    }

    // 断言核心链路真的通了（不是"可调用"而已）。
    let out = |n: &str| results.iter().find(|(r, _, _)| r == n).unwrap();
    assert!(out("shell").1, "shell 应执行成功: {}", out("shell").2);
    assert!(out("shell").2.contains("toolsweep-ok"), "shell 应捕获输出: {}", out("shell").2);
    assert!(out("fs_write").1 && out("fs_read").1, "fs 写读往返应成功");
    assert!(out("fs_read").2.contains("量化"), "fs_read 应读回内容: {}", out("fs_read").2);
    assert!(out("xlsx_write").1, "xlsx_write: {}", out("xlsx_write").2);
    assert!(out("pptx_write").1, "pptx_write: {}", out("pptx_write").2);
    assert!(out("doc_read").1, "doc_read(pptx): {}", out("doc_read").2);
    assert!(out("doc_read").2.contains("扫描页"), "doc_read 应读回 PPT 标题: {}", out("doc_read").2);
    assert!(out("run_tests").1, "run_tests 自定义命令应成功: {}", out("run_tests").2);
    assert!(out("run_tests").2.contains("tests-ok"), "run_tests 应捕获输出");

    // —— ② 其余工具：`{}` 探活（可路由、不 panic、返回结构化结果即可）——
    let sampled: Vec<&str> = results.iter().map(|(n, _, _)| n.as_str()).collect();
    for spec in reg.specs() {
        if sampled.contains(&spec.name.as_str()) {
            continue;
        }
        let tool = reg.get(&spec.name).expect("specs 与注册一致");
        let r = tool.invoke(json!({}), &ctx).await.expect("探活调用永不 Err");
        let note: String = r.output.to_string().chars().take(120).collect();
        println!("  {:<14} {}（探活：缺参/降级提示属正常）", spec.name, if r.ok { "✓" } else { "○" });
        let _ = note;
    }
}
