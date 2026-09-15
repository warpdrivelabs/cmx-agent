//! 后台子任务回执折尾（方案 20260914 改造二）app 层契约测试（crate 内可触私有字段）：
//! - 注入器路由：父空闲 → 立即 `send` 开独立回执回合；父忙 → 挂 `pending_receipts` 不打扰在途回合；
//! - 收口点吸收（deferred）+ 收尾兜底 drain：pending 清空、回执按「1 条折尾 + 剩余独立回合」落库。

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use cmx_agent_core::event::EventKind;
use cmx_agent_core::model::{ModelContext, ModelError, ModelResponse, ModelSeam};
use cmx_agent_core::{ToolCtx, TurnCancel};
use serde_json::json;

use crate::builder::DesktopAppBuilder;

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-receipt-{tag}-{n}"));
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

struct InstantModel;
#[async_trait::async_trait]
impl ModelSeam for InstantModel {
    async fn complete(&self, _ctx: &ModelContext) -> Result<ModelResponse, ModelError> {
        Ok(ModelResponse::text("好的"))
    }
}

fn build_app(tag: &str) -> (TempDir, Arc<crate::AgentApp>) {
    let tmp = TempDir::new(tag);
    // 与双壳装配同规：build 后必须 into_shared——后台注入器在这一步 attach（缺它
    // background=true fail-closed 拒绝）。
    let app = DesktopAppBuilder::new(tmp.path().join("workspace"), tmp.path(), Arc::new(InstantModel))
        .build()
        .unwrap()
        .into_shared();
    (tmp, app)
}

/// 后台派一个子任务（注入器已随 DesktopAppBuilder 装配，background=true 可用）。
async fn dispatch_background(app: &crate::AgentApp, tmp: &TempDir, parent: &str) {
    let reg = app.agent().tools();
    let task = reg.get("task").expect("task 工具应注册");
    let roots = vec![tmp.path().join("workspace")];
    let ctx = ToolCtx {
        sandbox: cmx_agent_core::SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
        session_id: parent,
    };
    let r = task
        .invoke(json!({"prompt": "为「服务器运维」写一句职责描述", "background": true}), &ctx)
        .await
        .expect("工具调用永不 Err");
    assert!(r.ok, "后台派发应成功：{}", r.output);
}

fn user_texts(evs: &[cmx_agent_core::SessionEvent]) -> Vec<String> {
    evs.iter()
        .filter_map(|e| match &e.kind {
            EventKind::UserMessage { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// 轮询等待条件成立（后台 spawn 的收尾/注入是异步的）。
async fn wait_for(mut cond: impl FnMut() -> bool, what: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !cond() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "等待超时：{what}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// 父空闲：注入器立即 `send` → 父会话自动出现独立回执回合（图三形态）。
#[tokio::test]
async fn idle_parent_gets_instant_receipt_turn() {
    let (tmp, app) = build_app("idle");
    dispatch_background(&app, &tmp, "task-parent").await;
    wait_for(
        || {
            app.get_events("task-parent")
                .map(|evs| {
                    user_texts(&evs)
                        .iter()
                        .any(|t| t.starts_with("<task_result"))
                })
                .unwrap_or(false)
        },
        "空闲父会话应收到独立回执回合",
    )
    .await;
    let evs = app.get_events("task-parent").unwrap();
    let users = user_texts(&evs);
    assert_eq!(users.len(), 1, "父会话只有回执这一条注入消息");
    assert!(users[0].starts_with("<task_result"));
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e.kind, EventKind::TurnStarted { .. }))
            .count(),
        1,
        "回执自立回合：一次 TurnStarted"
    );
    assert!(
        app.pending_receipts
            .lock()
            .unwrap()
            .get("task-parent")
            .is_none(),
        "空闲路径不经过 pending 队列"
    );
}

/// 父忙：回执挂 pending 不打扰在途回合；父回合结束后收口点吸收一条、兜底 drain 收走剩余，
/// pending 清空不丢。
#[tokio::test]
async fn busy_parent_parks_receipt_then_drains_on_next_turn() {
    let (tmp, app) = build_app("busy");
    let sid = "task-busy";
    // 制造「父忙」：插一个未取消的旗标（in-crate 直触私有字段），注入器应路由到 pending。
    app.active_turns
        .lock()
        .unwrap()
        .insert(sid.to_string(), TurnCancel::new());
    dispatch_background(&app, &tmp, sid).await;
    dispatch_background(&app, &tmp, sid).await;
    wait_for(
        || {
            app.pending_receipts
                .lock()
                .unwrap()
                .get(sid)
                .map(|q| q.len())
                .unwrap_or(0)
                >= 2
        },
        "两条回执应都挂在 pending 队列",
    )
    .await;
    assert!(
        app.get_events(sid).is_err(),
        "父会话尚不存在：忙时不该落库/开回合"
    );

    // 摘掉「父忙」→ 正常发一条消息：收口点把积压回执**逐条连续吸收**（每条跟一轮模型
    // 调用，全部折进同一回复，直到 pending 空）；兜底 drain 只收 race 窗口漏网。
    app.active_turns.lock().unwrap().remove(sid);
    let outcome = app.send(sid, "你好").await.unwrap();
    {
        let g = app.pending_receipts.lock().unwrap();
        assert!(
            g.get(sid).map(|q| q.is_empty()).unwrap_or(true),
            "收口点吸收后 pending 必须清空"
        );
    }
    let evs = app.get_events(sid).unwrap();
    let users = user_texts(&evs);
    assert_eq!(users.len(), 3, "原始消息 + 两条回执都落库");
    assert_eq!(users[0], "你好");
    assert!(users[1].starts_with("<task_result"), "收口点吸收的回执 1");
    assert!(users[2].starts_with("<task_result"), "收口点吸收的回执 2");
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e.kind, EventKind::TurnStarted { .. }))
            .count(),
        1,
        "两条回执都折进同一回复：仍是一个回合"
    );
    assert_eq!(outcome.steps, 3, "1 轮原始 + 2 轮折尾续写");
    assert!(outcome.final_text.unwrap().contains("好的"));
}
