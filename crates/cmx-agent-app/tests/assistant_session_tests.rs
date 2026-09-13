//! 「助理」统一会话的 app 层契约：create_session get-or-create 不覆盖 meta、ensure_session
//! 仅新建时采用给定 workspace/title、桌面入口命令 `open_assistant_session` 双壳派发可用。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::{DesktopAppBuilder, dispatch_json};
use cmx_agent_core::MockModel;

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-assistant-app-{tag}-{n}"));
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

fn app_with(tmp: &TempDir) -> cmx_agent_app::AgentApp {
    DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(MockModel::saying("好的")))
        .build()
        .unwrap()
}

fn meta_of(app: &cmx_agent_app::AgentApp, id: &str) -> cmx_agent_app::SessionMeta {
    app.list_sessions()
        .unwrap()
        .into_iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("会话 {id} 应存在"))
}

#[tokio::test]
async fn create_session_is_get_or_create_and_never_clobbers_meta() {
    let tmp = TempDir::new("goc");
    let app = app_with(&tmp);

    app.create_session("s1").unwrap();
    // 二次调用（IM 桥每条消息都会调）：原样返回，不重置 meta。
    app.create_session("s1").unwrap();

    // ensure_session 对已存在会话：给定 workspace/title 均不生效。
    app.ensure_session("s1", Some("other".into()), Some("不该出现".into()))
        .unwrap();
    let meta = meta_of(&app, "s1");
    assert_ne!(meta.title.as_deref(), Some("不该出现"), "已存在会话标题不被覆盖");
    assert_eq!(
        meta.workspace_id.as_deref(),
        Some("default"),
        "已存在会话空间不被改写（新建时当前空间即 default）"
    );
}

#[tokio::test]
async fn ensure_session_adopts_given_workspace_and_title_only_on_create() {
    let tmp = TempDir::new("ensure");
    let app = app_with(&tmp);

    let id = app
        .ensure_session("s2", Some("default".into()), Some("IM 助理".into()))
        .unwrap();
    assert_eq!(id, "s2");
    let meta = meta_of(&app, "s2");
    assert_eq!(meta.workspace_id.as_deref(), Some("default"));
    assert_eq!(meta.title.as_deref(), Some("IM 助理"));
}

#[tokio::test]
async fn open_assistant_session_creates_default_pinned_and_is_idempotent() {
    let tmp = TempDir::new("open");
    let app = app_with(&tmp);

    let id = app.open_assistant_session().unwrap();
    assert_eq!(id, cmx_agent_app::ASSISTANT_SESSION_ID);
    // 再走一次（含 create_session 旧调用形态）：空间/标题不被漂移。
    app.create_session(cmx_agent_app::ASSISTANT_SESSION_ID).unwrap();
    app.open_assistant_session().unwrap();

    let meta = meta_of(&app, cmx_agent_app::ASSISTANT_SESSION_ID);
    assert_eq!(meta.workspace_id.as_deref(), Some("default"));
    assert_eq!(meta.title.as_deref(), Some(cmx_agent_app::ASSISTANT_SESSION_TITLE));
}

#[tokio::test]
async fn get_events_on_fresh_assistant_session_returns_title_not_found() {
    let tmp = TempDir::new("empty");
    let app = Arc::new(app_with(&tmp));

    // 刚 ensure、还没有任何日志：get_events 不应报 NotFound——前端首开 tab 靠响应里的 title 命名
    //（此前空会话返回错误，tab 停留在英文会话 id "im-assistant" 上）。
    let id = app.open_assistant_session().unwrap();
    let raw = dispatch_json(
        &app,
        format!(r#"{{"cmd":"get_events","session_id":"{id}","limit":50}}"#).as_str(),
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v["ok"].as_bool().unwrap(), "空会话 get_events 应成功：{raw}");
    assert_eq!(v["data"]["total"].as_u64(), Some(0));
    assert_eq!(
        v["data"]["title"].as_str(),
        Some(cmx_agent_app::ASSISTANT_SESSION_TITLE),
        "空会话也要带 meta 标题"
    );
}

#[tokio::test]
async fn open_assistant_session_roundtrips_through_json_protocol() {
    let tmp = TempDir::new("proto");
    let app = Arc::new(app_with(&tmp));

    let raw = dispatch_json(&app, r#"{"cmd":"open_assistant_session"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert!(v["ok"].as_bool().unwrap(), "应成功：{raw}");
    assert_eq!(
        v["data"]["session_id"].as_str(),
        Some(cmx_agent_app::ASSISTANT_SESSION_ID)
    );
}
