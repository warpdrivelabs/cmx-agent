//! IM 桥测试：用内存 MockProvider 驱动 `bridge.tick`，验证 鉴权 / 会话映射 / 跑回合 / 回复。
//! 无网络、确定性。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use cmx_agent_app::{AgentApp, DesktopAppBuilder};
use cmx_agent_core::MockModel;
use cmx_agent_im::{ImBridge, ImProvider, InboundMsg};

struct MockProvider {
    inbound: Mutex<Vec<InboundMsg>>, // 一次性投递（poll 后清空）
    sent: Mutex<Vec<(String, String)>>, // 记录 send
}
impl MockProvider {
    fn new(msgs: Vec<InboundMsg>) -> Arc<Self> {
        Arc::new(Self {
            inbound: Mutex::new(msgs),
            sent: Mutex::new(vec![]),
        })
    }
}
#[async_trait]
impl ImProvider for MockProvider {
    async fn poll(&self, _offset: i64) -> Result<(Vec<InboundMsg>, i64), String> {
        let msgs = std::mem::take(&mut *self.inbound.lock().unwrap());
        let next = msgs.iter().map(|m| m.update_id + 1).max().unwrap_or(0);
        Ok((msgs, next))
    }
    async fn send(&self, chat: &str, text: &str) -> Result<(), String> {
        self.sent.lock().unwrap().push((chat.to_string(), text.to_string()));
        Ok(())
    }
}

fn temp_app(model: MockModel) -> Arc<AgentApp> {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // 纳秒 + 进程内自增：并行测试可能落在同一时钟刻度，纯纳秒名会撞目录共享存储。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("cmx-im-test-{k}-{n}"));
    let app = DesktopAppBuilder::new(base.join("ws"), base.join("data"), Arc::new(model))
        .build()
        .unwrap();
    Arc::new(app)
}

fn allow(ids: &[&str]) -> Option<HashSet<String>> {
    Some(ids.iter().map(|s| s.to_string()).collect())
}

#[tokio::test]
async fn bridge_runs_turn_and_replies() {
    let app = temp_app(MockModel::saying("你好，我是回复"));
    let prov = MockProvider::new(vec![InboundMsg {
        chat_id: "555".into(),
        text: "在吗".into(),
        update_id: 1,
        sender: String::new(),
    }]);
    let bridge = ImBridge::new(app, prov.clone(), "test", allow(&["555"]));

    let n = bridge.tick().await.unwrap();
    assert_eq!(n, 1);
    let sent = prov.sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "应回一条");
    assert_eq!(sent[0].0, "555");
    assert!(sent[0].1.contains("你好，我是回复"), "{:?}", sent[0]);
}

#[tokio::test]
async fn bridge_blocks_unauthorized() {
    let app = temp_app(MockModel::saying("不该跑"));
    let prov = MockProvider::new(vec![InboundMsg {
        chat_id: "999".into(),
        text: "hi".into(),
        update_id: 1,
        sender: String::new(),
    }]);
    let bridge = ImBridge::new(app, prov.clone(), "test", allow(&["555"]));

    bridge.tick().await.unwrap();
    let sent = prov.sent.lock().unwrap();
    assert_eq!(sent.len(), 1);
    assert!(sent[0].1.contains("未授权"), "{:?}", sent[0]);
}

#[tokio::test]
async fn same_chat_reuses_session() {
    // 同一 chat 两条消息 → 同一 agent 会话（上下文续上）。用带工具回合确保有落库。
    let app = temp_app(MockModel::saying("好的"));
    let prov = MockProvider::new(vec![InboundMsg {
        chat_id: "42".into(),
        text: "第一句".into(),
        update_id: 1,
        sender: String::new(),
    }]);
    let bridge = ImBridge::new(app.clone(), prov.clone(), "test", allow(&["42"]));
    bridge.tick().await.unwrap();
    // 第二条
    *prov.inbound.lock().unwrap() = vec![InboundMsg {
        chat_id: "42".into(),
        text: "第二句".into(),
        update_id: 2,
        sender: String::new(),
    }];
    bridge.tick().await.unwrap();

    // 统一会话 im-assistant 应存在且含两轮（同 chat 同通道都落它）
    let evs = app.get_events("im-assistant").unwrap();
    let turns = evs
        .iter()
        .filter(|e| matches!(e.kind, cmx_agent_core::event::EventKind::UserMessage { .. }))
        .count();
    assert_eq!(turns, 2, "同 chat 两条消息应落到同一会话的两轮");
}
