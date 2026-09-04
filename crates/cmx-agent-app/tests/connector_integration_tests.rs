//! 连接器与 app 层集成测试：连接器工具挂进内核、list_connectors 前门命令、离线降级。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::{ConnectorConfig, DesktopAppBuilder, dispatch_json};
use cmx_agent_core::MockModel;

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

/// 用**不可达**端口配置连接器 → 面板仍列出三卡，但状态离线（用户选择的降级行为）。
fn offline_config() -> ConnectorConfig {
    ConnectorConfig {
        flow_base: "http://127.0.0.1:9".into(),
        onto_base: "http://127.0.0.1:9".into(),
        report_base: "http://127.0.0.1:9".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn list_connectors_command_returns_three_cards() {
    let tmp = TempDir::new("conn-list");
    let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(MockModel::saying("hi")))
        .connectors(offline_config())
        .build()
        .unwrap();

    let resp = dispatch_json(&app, r#"{"cmd":"list_connectors"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["ok"], true);
    let conns = v["data"]["connectors"].as_array().unwrap();
    assert_eq!(
        conns.len(),
        3,
        "panel must always list all three connectors"
    );
    // 不可达 → 全部离线，但仍可见（含描述与工具名）
    for c in conns {
        assert_eq!(c["status"]["online"], false);
        assert!(!c["tools"].as_array().unwrap().is_empty());
        assert!(!c["name"].as_str().unwrap().is_empty());
    }
}

#[tokio::test]
async fn without_connectors_list_is_empty() {
    let tmp = TempDir::new("conn-none");
    let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(MockModel::saying("hi")))
        .build() // 不启用连接器
        .unwrap();
    let resp = dispatch_json(&app, r#"{"cmd":"list_connectors"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["connectors"].as_array().unwrap().len(), 0);
}

/// 连接器工具挂进内核后，模型可以调它；服务不可达时回灌错误结果、agent 不崩、回合正常结束。
#[tokio::test]
async fn agent_can_invoke_connector_tool_and_degrades_gracefully() {
    use cmx_agent_core::event::{EventKind, StopReason};
    use cmx_agent_core::{ModelResponse, ToolCall};

    let tmp = TempDir::new("conn-invoke");
    // 脚本模型：先调 flow 连接器，看到结果后收尾
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "c1",
            "flow_list_definitions",
            serde_json::json!({}),
        )]),
        ModelResponse::text("好的"),
    ]));
    let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), model)
        .connectors(offline_config())
        .build()
        .unwrap();

    let out = app.send("s1", "列出流程定义").await.unwrap();
    assert_eq!(out.reason, StopReason::Completed);
    // 工具被调用了（tool_invoked），且结果是错误（服务不可达）但不 panic
    let invoked = out.new_events.iter().any(|e| {
        matches!(&e.kind,
        EventKind::ToolInvoked { call } if call.name == "flow_list_definitions")
    });
    assert!(invoked, "connector tool must be invoked");
    let errored = out
        .new_events
        .iter()
        .any(|e| matches!(&e.kind, EventKind::ToolResult { ok: false, .. }));
    assert!(
        errored,
        "unreachable connector must yield error result, agent survives"
    );
}
