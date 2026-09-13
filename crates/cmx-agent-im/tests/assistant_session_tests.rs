//! IM 统一「助理」会话语义：三通道共用同一会话 id（`im-assistant`）、消息带通道来源前缀、
//! 会话空间恒 default（桌面当前在别的空间也不影响）。口径对齐 `qq/wechat/feishu_tests.rs`。

use std::collections::HashSet;
use std::sync::Arc;

use cmx_agent_app::{AgentApp, DesktopAppBuilder};
use cmx_agent_core::event::EventKind;
use cmx_agent_core::MockModel;
use cmx_agent_im::{FeishuProvider, ImBridge, WechatProvider};

fn temp_app(model: MockModel) -> Arc<AgentApp> {
    // 纳秒 + 进程内自增：同进程并行测试可能落在同一时钟刻度，纯纳秒名会撞目录共享存储。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let base = std::env::temp_dir().join(format!("cmx-assistant-test-{k}-{n}"));
    let app = DesktopAppBuilder::new(base.join("ws"), base.join("data"), Arc::new(model))
        .build()
        .unwrap();
    Arc::new(app)
}

fn allow(ids: &[&str]) -> Option<HashSet<String>> {
    Some(ids.iter().map(|s| s.to_string()).collect())
}

fn user_texts(app: &AgentApp, sid: &str) -> Vec<String> {
    app.get_events(sid)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|e| match e.kind {
            EventKind::UserMessage { text } => Some(text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn two_channels_share_one_assistant_session_with_source_prefix() {
    let app = temp_app(MockModel::saying("好的"));
    let feishu = Arc::new(FeishuProvider::new("id", "secret", None));
    let wechat = Arc::new(WechatProvider::new("", None)); // 空 token：send 在触网前 fail-fast
    feishu.inject("oc_a", "飞书消息").await;
    wechat.inject("usr_wx_a", "微信消息").await;
    let b1 = ImBridge::new(app.clone(), feishu, "feishu", allow(&["oc_a"]));
    let b2 = ImBridge::new(app.clone(), wechat, "wechat", allow(&["usr_wx_a"]));

    b1.tick().await.unwrap();
    b2.tick().await.unwrap();

    let sid = cmx_agent_app::ASSISTANT_SESSION_ID;
    let texts = user_texts(&app, sid);
    assert_eq!(
        texts,
        vec!["【飞书】飞书消息", "【微信】微信消息"],
        "两通道落同一会话且带来源前缀"
    );
    // 旧式 per-chat 会话名（im-<kind>-<chat>）不再产生。
    assert!(app.get_events("im-feishu-oc_a").unwrap_or_default().is_empty());
    assert!(app.get_events("im-wechat-usr_wx_a").unwrap_or_default().is_empty());
}

#[tokio::test]
async fn assistant_session_workspace_stays_default_when_desktop_elsewhere() {
    let app = temp_app(MockModel::saying("好的"));
    // 桌面切到非 default 空间（create_managed 创建即选中，返回 list() 含 current）。
    let ws = app.create_workspace("别的空间").unwrap();
    let wid = ws["current"]["id"]
        .as_str()
        .expect("current workspace id")
        .to_string();
    assert_ne!(wid, "default");
    app.select_workspace(Some(&wid)).unwrap();

    let prov = Arc::new(WechatProvider::new("", None));
    prov.inject("usr_wx_b", "在吗").await;
    let bridge = ImBridge::new(app.clone(), prov, "wechat", allow(&["usr_wx_b"]));
    bridge.tick().await.unwrap();

    let meta = app
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|m| m.id == cmx_agent_app::ASSISTANT_SESSION_ID)
        .expect("助理会话应已创建");
    assert_eq!(
        meta.workspace_id.as_deref(),
        Some("default"),
        "IM 会话空间恒 default，不随桌面当前空间漂移"
    );
    assert_eq!(
        meta.title.as_deref(),
        Some(cmx_agent_app::ASSISTANT_SESSION_TITLE),
        "标题固定，不被首条消息改写"
    );
}
