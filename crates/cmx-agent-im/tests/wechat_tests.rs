//! 微信（ClawBot/iLink）provider × ImBridge 集成：用 WechatProvider + 手动注入 inbox
//! 驱动 `bridge.tick`，验证鉴权 / 会话映射 / 绑定模式——口径对齐 `qq_tests.rs`。
//! 无网络、确定性：不调 `start()`（本就 no-op），inject 塞队列；桥回复走 `send`，
//! 未登录 token 为空时在触网前 fail-fast，失败被桥吞掉，不阻断流程。

use std::collections::HashSet;
use std::sync::Arc;

use cmx_agent_app::{AgentApp, DesktopAppBuilder};
use cmx_agent_connectors::im_binding::BoundIdentity;
use cmx_agent_core::MockModel;
use cmx_agent_im::{ImBridge, MockBindingResolver, WechatProvider};

fn temp_app(model: MockModel) -> Arc<AgentApp> {
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    // 纳秒 + 进程内自增：并行测试可能落在同一时钟刻度，纯纳秒名会撞目录共享存储。
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let k = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("cmx-wechat-test-{k}-{n}"));
    let app = DesktopAppBuilder::new(base.join("ws"), base.join("data"), Arc::new(model))
        .build()
        .unwrap();
    Arc::new(app)
}

fn allow(ids: &[&str]) -> Option<HashSet<String>> {
    Some(ids.iter().map(|s| s.to_string()).collect())
}

fn bound_id(user: &str, roles: &[&str]) -> BoundIdentity {
    BoundIdentity {
        user_id: user.into(),
        username: user.into(),
        roles: roles.iter().map(|s| s.to_string()).collect(),
    }
}

#[tokio::test]
async fn wechat_bridge_runs_turn_and_replies() {
    let app = temp_app(MockModel::saying("微信回复"));
    let prov = Arc::new(WechatProvider::new("", None)); // 空 token：send 在触网前 fail-fast
    prov.inject("usr_wx_1", "在吗").await;
    let bridge = ImBridge::new(app, prov.clone(), "wechat", allow(&["usr_wx_1"]));

    let n = bridge.tick().await.unwrap();
    assert_eq!(n, 1);
}

#[tokio::test]
async fn wechat_bridge_blocks_unauthorized() {
    let app = temp_app(MockModel::saying("不该跑"));
    let prov = Arc::new(WechatProvider::new("", None));
    prov.inject("usr_wx_999", "hi").await;
    let bridge = ImBridge::new(app.clone(), prov.clone(), "wechat", allow(&["usr_wx_1"]));

    bridge.tick().await.unwrap();
    // 非白名单：不跑回合、不创建会话（仅尝试 send 未授权提示，失败被吞）。
    let evs = app.get_events("im-assistant").unwrap_or_default();
    assert!(evs.is_empty(), "未授权 chat 不应建会话");
}

#[tokio::test]
async fn wechat_messages_land_in_unified_assistant_session() {
    let app = temp_app(MockModel::saying("好的"));
    let prov = Arc::new(WechatProvider::new("", None));
    let bridge = ImBridge::new(app.clone(), prov.clone(), "wechat", allow(&["usr_wx_42"]));

    prov.inject("usr_wx_42", "第一句").await;
    bridge.tick().await.unwrap();
    prov.inject("usr_wx_42", "第二句").await;
    bridge.tick().await.unwrap();

    // 统一会话：三通道消息都落 im-assistant（桌面「助理」入口打开的同一会话）。
    let evs = app.get_events("im-assistant").unwrap();
    let turns = evs
        .iter()
        .filter(|e| matches!(e.kind, cmx_agent_core::event::EventKind::UserMessage { .. }))
        .count();
    assert_eq!(turns, 2, "同 chat 两条消息应落到同一会话两轮");
}

// ── 用户绑定模式（微信 from_user_id ↔ 门户用户；provider 标签 "wechat" 原样入绑定链路）──

#[tokio::test]
async fn wechat_binding_mode_unbound_sender_gets_prompt_no_turn() {
    let app = temp_app(MockModel::saying("不该跑"));
    let prov = Arc::new(WechatProvider::new("", None));
    let resolver = Arc::new(MockBindingResolver::new()); // 无绑定
    let bridge = ImBridge::new(app.clone(), prov.clone(), "wechat", None).with_bindings(resolver);

    prov.inject_with_sender("usr_wx_1", "hello", "wx_stranger@im.wechat").await;
    bridge.tick().await.unwrap();

    let evs = app.get_events("im-assistant").unwrap_or_default();
    assert!(evs.is_empty(), "未绑定 sender 不应跑回合");
}

#[tokio::test]
async fn wechat_binding_mode_code_message_binds_then_runs() {
    let app = temp_app(MockModel::saying("回复"));
    let prov = Arc::new(WechatProvider::new("", None));
    let resolver = Arc::new(MockBindingResolver::new().with_code("888888", bound_id("u9", &["user"])));
    let bridge = ImBridge::new(app.clone(), prov.clone(), "wechat", None).with_bindings(resolver);

    // 发验证码 → 完成绑定（不跑回合，无会话事件）
    prov.inject_with_sender("usr_wx_1", "888888", "wx_new@im.wechat").await;
    bridge.tick().await.unwrap();
    let evs = app.get_events("im-assistant").unwrap_or_default();
    assert!(evs.is_empty(), "验证码消息本身不跑回合");

    // 绑定后正常消息 → 跑回合
    prov.inject_with_sender("usr_wx_1", "你好", "wx_new@im.wechat").await;
    bridge.tick().await.unwrap();
    let evs = app.get_events("im-assistant").unwrap_or_default();
    assert!(!evs.is_empty(), "绑定后应跑回合");
}

#[tokio::test]
async fn wechat_binding_mode_bound_sender_runs_turn() {
    let app = temp_app(MockModel::saying("已绑定回复"));
    let prov = Arc::new(WechatProvider::new("", None));
    let resolver =
        Arc::new(MockBindingResolver::new().with_bound("wx_vip@im.wechat", bound_id("u1", &["admin"])));
    let bridge = ImBridge::new(app.clone(), prov.clone(), "wechat", None).with_bindings(resolver);

    prov.inject_with_sender("usr_wx_2", "在吗", "wx_vip@im.wechat").await;
    bridge.tick().await.unwrap();

    let evs = app.get_events("im-assistant").unwrap_or_default();
    assert!(!evs.is_empty(), "已绑定 sender 应跑回合");
}
