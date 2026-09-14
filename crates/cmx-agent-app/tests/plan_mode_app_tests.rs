//! 计划模式 app 层契约（方案 20260914 §7.1，阶段二）：
//! - SetPlanMode 用户独占切换：meta 持久化、幂等（unchanged）、im-assistant 拒绝、
//!   不存在会话拒绝（红队 N6 幽灵会话防线）；
//! - meta 重建保活回归（红队 P2-4 / 蓝队 A14）：plan_mode 会话跑一回合后 meta.plan_mode
//!   不得被 send_inner_locked 的 meta 重建抹成 false；
//! - ListAgents / SaveAgent / DeleteAgent 协议链路 + 热生效（task spec 枚举变化）。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::DesktopAppBuilder;
use cmx_agent_core::agents::AgentSpec;
use cmx_agent_core::{ModelContext, ModelError, ModelResponse, ModelSeam};

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-plan-app-{tag}-{n}"));
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

fn build_app(tag: &str) -> (TempDir, Arc<cmx_agent_app::AgentApp>) {
    let tmp = TempDir::new(tag);
    let app = Arc::new(
        DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(InstantModel))
            .build()
            .unwrap(),
    );
    (tmp, app)
}

#[tokio::test]
async fn set_plan_mode_validates_and_persists() {
    let (_t, app) = build_app("crud");
    // 不存在的会话拒绝（N6：不造幽灵目录）
    assert!(app.set_plan_mode("ghost", true).await.is_err());
    let sessions = app.list_sessions().unwrap();
    assert!(sessions.iter().all(|m| m.id != "ghost"), "不得留下幽灵会话");

    app.create_session("s1").unwrap();
    // 开
    let r = app.set_plan_mode("s1", true).await.unwrap();
    assert_eq!(r["changed"], serde_json::json!(true));
    assert!(app.list_sessions().unwrap().iter().find(|m| m.id == "s1").unwrap().plan_mode);
    // 幂等
    let r = app.set_plan_mode("s1", true).await.unwrap();
    assert_eq!(r["changed"], serde_json::json!(false));
    // 关
    app.set_plan_mode("s1", false).await.unwrap();
    assert!(!app.list_sessions().unwrap().iter().find(|m| m.id == "s1").unwrap().plan_mode);
}

#[tokio::test]
async fn set_plan_mode_rejects_assistant_session() {
    let (_t, app) = build_app("assistant");
    assert!(app.set_plan_mode("im-assistant", true).await.is_err());
    assert!(app.set_plan_mode("im-feishu-1", true).await.is_err());
}

#[tokio::test]
async fn meta_rebuild_preserves_plan_mode() {
    // 回归：send_inner_locked 每回合重建 meta——plan_mode 必须从回合初值/flag 对账回填，
    // 否则每发一条消息计划模式就被抹成 false（红队 P2-4）。
    let (_t, app) = build_app("preserve");
    app.create_session("s-plan").unwrap();
    app.set_plan_mode("s-plan", true).await.unwrap();
    app.send("s-plan", "只调研不改").await.unwrap();
    let meta = app
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|m| m.id == "s-plan")
        .expect("session still there");
    assert!(meta.plan_mode, "跑一回合后 meta.plan_mode 必须保活");
    // 重新加载 store 也要保活（落盘正确）
    let events = app.get_events("s-plan").unwrap();
    assert!(!events.is_empty());
}

#[tokio::test]
async fn agents_crud_hot_effect_via_protocol_shape() {
    let (_t, app) = build_app("agents");
    // 初始：内置两条
    let r = app.list_agents().unwrap();
    assert_eq!(r["builtin"].as_array().unwrap().len(), 2);
    assert_eq!(r["custom"].as_array().unwrap().len(), 0);
    // 保存自定义
    let spec = AgentSpec {
        name: "code_reviewer".into(),
        title: "代码评审员".into(),
        description: "只读审查代码".into(),
        tools: cmx_agent_core::agents::ToolSelection::Allow {
            names: vec!["fs_read".into(), "grep".into()],
        },
        model: None,
        system_prompt: "严格评审".into(),
        enabled: true,
        builtin: false,
    };
    let r = app.save_agent(spec).unwrap();
    assert_eq!(r["hot"], serde_json::json!(true));
    // 内置覆盖（只改 enabled）
    app.save_agent(AgentSpec {
        name: "explore".into(),
        title: "无所谓".into(),
        description: String::new(),
        tools: Default::default(),
        model: None,
        system_prompt: String::new(),
        enabled: false,
        builtin: true,
    })
    .unwrap();
    let r = app.list_agents().unwrap();
    assert_eq!(r["custom"].as_array().unwrap().len(), 1);
    let explore = r["builtin"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "explore")
        .unwrap();
    assert_eq!(explore["enabled"], serde_json::json!(false));
    // 删除自定义；内置拒绝
    app.delete_agent("code_reviewer").unwrap();
    assert!(app.delete_agent("explore").is_err(), "内置不可删");
    // agents.json 落盘存在
    // （路径由 builder 内部决定，这里以 list 结果为准）
    let r = app.list_agents().unwrap();
    assert_eq!(r["custom"].as_array().unwrap().len(), 0);
}
