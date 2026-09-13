//! 会话删除 vs 在途回合的竞态契约：回合期间删除 → 回合收尾**不得**把会话文件写回来
//! （僵尸复活防线：墓碑 + 取消）。回归用例对应红蓝对抗 P1 项「delete_session 僵尸复活」。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::{DesktopAppBuilder, SendOutcome};
use cmx_agent_core::{ModelError, ModelResponse, ModelSeam, ModelContext, StopReason};

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-del-app-{tag}-{n}"));
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

/// 回合中途悬挂的模型：给删除留出「回合在飞」的时间窗。
struct SlowModel;
#[async_trait::async_trait]
impl ModelSeam for SlowModel {
    async fn complete(&self, _ctx: &ModelContext) -> Result<ModelResponse, ModelError> {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok(ModelResponse::text("好的"))
    }
}

#[tokio::test]
async fn delete_during_turn_leaves_no_session_file_behind() {
    let tmp = TempDir::new("zombie");
    let app = Arc::new(
        DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(SlowModel))
            .build()
            .unwrap(),
    );

    // 在途回合：发送挂在模型的 500ms 睡眠上。
    let app2 = app.clone();
    let turn = tokio::spawn(async move { app2.send("s1", "你好").await });
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;

    // 回合在飞时删除：应取消回合 + 落墓碑。
    app.delete_session("s1").unwrap();

    let outcome: SendOutcome = turn.await.unwrap().unwrap();
    assert!(matches!(outcome.reason, StopReason::Stopped | StopReason::Completed));

    // 僵尸防线：删除后无论回合如何收尾，磁盘上不得再有该会话（meta 与事件文件皆无）。
    let metas = app.list_sessions().unwrap();
    assert!(metas.iter().all(|m| m.id != "s1"), "会话列表不应有 s1：{metas:?}");
    assert!(
        !tmp.path().join("sessions").join("s1").exists(),
        "sessions/s1 目录不应复活"
    );
}

#[tokio::test]
async fn new_send_after_delete_revives_session_normally() {
    // 显式再发消息 = 复活意图：墓碑摘除，回合照常落库（不误伤正常使用）。
    let tmp = TempDir::new("revive");
    let app = Arc::new(
        DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(SlowModel))
            .build()
            .unwrap(),
    );
    app.create_session("s2").unwrap();
    app.delete_session("s2").unwrap();
    app.send("s2", "重新开始").await.unwrap();
    assert!(
        app.list_sessions().unwrap().iter().any(|m| m.id == "s2"),
        "删除后新发消息应正常重建会话"
    );
}
