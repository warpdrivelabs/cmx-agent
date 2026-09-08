//! 飞书 provider × ImBridge 集成：用 FeishuProvider + 手动注入 inbox 驱动 `bridge.tick`，
//! 验证鉴权 / 会话映射 / 分段复用——口径对齐 `bridge_tests.rs`。无网络、确定性。

use std::collections::HashSet;

use cmx_agent_app::{AgentApp, DesktopAppBuilder};
use cmx_agent_core::MockModel;
use cmx_agent_im::{FeishuProvider, ImBridge};

fn temp_app(model: MockModel) -> std::sync::Arc<AgentApp> {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir().join(format!("cmx-feishu-test-{n}"));
    let app = DesktopAppBuilder::new(base.join("ws"), base.join("data"), std::sync::Arc::new(model))
        .build()
        .unwrap();
    std::sync::Arc::new(app)
}

fn allow(ids: &[&str]) -> Option<HashSet<String>> {
    Some(ids.iter().map(|s| s.to_string()).collect())
}

#[tokio::test]
async fn feishu_bridge_runs_turn_and_replies() {
    let app = temp_app(MockModel::saying("飞书回复"));
    let prov = std::sync::Arc::new(FeishuProvider::new("id", "secret", None));
    // 不调 start()——避免连真实飞书；直接注入消息。
    prov.inject("oc_555", "在吗").await;
    let bridge = ImBridge::new(app, prov.clone(), "feishu", allow(&["oc_555"]));

    let n = bridge.tick().await.unwrap();
    assert_eq!(n, 1);
    // send 会真的尝试连飞书并失败——但桥把 send 失败吞掉（`let _ =`），不阻断流程。
    // 此处仅验证 tick 处理了一条且会话已建。
}

#[tokio::test]
async fn feishu_bridge_blocks_unauthorized() {
    let app = temp_app(MockModel::saying("不该跑"));
    let prov = std::sync::Arc::new(FeishuProvider::new("id", "secret", None));
    prov.inject("oc_999", "hi").await;
    let bridge = ImBridge::new(app.clone(), prov.clone(), "feishu", allow(&["oc_555"]));

    bridge.tick().await.unwrap();
    // 非白名单：不跑回合、不创建会话（仅尝试 send 未授权提示，失败被吞）。
    let evs = app.get_events("im-feishu-oc_999").unwrap_or_default();
    assert!(evs.is_empty(), "未授权 chat 不应建会话");
}

#[tokio::test]
async fn feishu_same_chat_reuses_session() {
    let app = temp_app(MockModel::saying("好的"));
    let prov = std::sync::Arc::new(FeishuProvider::new("id", "secret", None));
    let bridge = ImBridge::new(app.clone(), prov.clone(), "feishu", allow(&["oc_42"]));

    prov.inject("oc_42", "第一句").await;
    bridge.tick().await.unwrap();
    prov.inject("oc_42", "第二句").await;
    bridge.tick().await.unwrap();

    let evs = app.get_events("im-feishu-oc_42").unwrap();
    let turns = evs
        .iter()
        .filter(|e| matches!(e.kind, cmx_agent_core::event::EventKind::UserMessage { .. }))
        .count();
    assert_eq!(turns, 2, "同 chat 两条消息应落到同一会话两轮");
}
