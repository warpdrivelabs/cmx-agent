//! 连接器测试。单测走离线（工具 spec / 信封 / 降级）；`#[ignore]` 的集成测试需真 live 服务手动跑。

use cmx_agent_connectors::{ConnectorConfig, ConnectorRegistry};
use cmx_agent_core::{SandboxMode, Tool, ToolCtx};
use std::path::PathBuf;

fn dummy_ctx_roots() -> Vec<PathBuf> {
    vec![]
}

#[test]
fn registry_exposes_three_connectors_with_tools() {
    let reg = ConnectorRegistry::new(ConnectorConfig::default());
    let descs = reg.descriptors();
    assert_eq!(descs.len(), 3);
    let ids: Vec<&str> = descs.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, vec!["flow", "onto", "report"]);
    // 每个连接器至少暴露一个工具
    for d in descs {
        assert!(!d.tools.is_empty(), "connector {} exposes no tools", d.id);
    }
}

#[test]
fn register_into_adds_connector_tools_to_registry() {
    let mut registry = cmx_agent_core::ToolRegistry::new();
    let reg = ConnectorRegistry::new(ConnectorConfig::default());
    reg.register_into(&mut registry);
    for t in [
        "flow_list_definitions",
        "onto_list_object_types",
        "report_list_reports",
    ] {
        assert!(registry.contains(t), "tool {t} not registered");
    }
}

#[test]
fn connector_tools_carry_read_guard_hints() {
    use cmx_agent_connectors::{CmxServiceClient, FlowConnector, OntoConnector};
    let flow = FlowConnector {
        client: CmxServiceClient::new("http://127.0.0.1:8091"),
    };
    let spec = flow.spec();
    assert_eq!(spec.guard.requires_auth.as_deref(), Some("flow:read"));
    assert!(spec.guard.idempotent);

    let onto = OntoConnector {
        client: CmxServiceClient::new("http://127.0.0.1:8097"),
    };
    assert_eq!(
        onto.spec().guard.requires_auth.as_deref(),
        Some("onto:read")
    );
}

/// 降级：指向一个**不存在**的端口，工具调用应返回 ok=false 的错误结果（不 panic）。
#[tokio::test]
async fn connector_tool_gracefully_degrades_when_service_down() {
    use cmx_agent_connectors::{CmxServiceClient, FlowConnector};
    // 指向一个几乎不可能有服务的端口
    let flow = FlowConnector {
        client: CmxServiceClient::new("http://127.0.0.1:9"),
    };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx {
        sandbox: SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
    };
    let r = flow.invoke(serde_json::json!({}), &ctx).await.unwrap();
    assert!(
        !r.ok,
        "unreachable service must yield error result, not panic"
    );
    assert!(r.output["error"].as_str().unwrap().contains("失败"));
}

/// 健康探测：不可达服务 → offline，不报错。
#[tokio::test]
async fn probe_offline_service_returns_offline_status() {
    use cmx_agent_connectors::{CmxServiceClient, probe};
    let client = CmxServiceClient::new("http://127.0.0.1:9");
    let status = probe(&client).await;
    assert!(!status.online);
    assert!(status.service_name.is_none());
    assert!(status.detail.is_some());
}

// ───────────────────── LIVE 集成测试（需真服务，默认 #[ignore]）─────────────────────
// 手动跑：先确保 cmx-flow 在 :8091 运行，再 `cargo test --offline -p cmx-agent-connectors -- --ignored`

#[tokio::test]
#[ignore = "需要 live cmx-flow 服务在 :8091"]
async fn live_flow_lists_real_definitions() {
    use cmx_agent_connectors::{CmxServiceClient, FlowConnector};
    let flow = FlowConnector {
        client: CmxServiceClient::new("http://127.0.0.1:8091").with_identity("default", "admin"),
    };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx {
        sandbox: SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
    };
    let r = flow.invoke(serde_json::json!({}), &ctx).await.unwrap();
    assert!(r.ok, "live flow call failed: {:?}", r.output);
    assert_eq!(r.output["service"], "cmx-flow");
    assert!(
        r.output["count"].as_u64().unwrap() >= 1,
        "expected ≥1 process definition"
    );
}

#[tokio::test]
#[ignore = "需要 live cmx-flow 服务在 :8091"]
async fn live_probe_flow_online() {
    use cmx_agent_connectors::{CmxServiceClient, probe};
    let status = probe(&CmxServiceClient::new("http://127.0.0.1:8091")).await;
    assert!(status.online, "flow should be online: {:?}", status.detail);
    assert!(status.service_name.unwrap().contains("flow"));
}
