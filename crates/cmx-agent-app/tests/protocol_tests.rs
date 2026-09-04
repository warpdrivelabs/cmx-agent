//! 前门协议测试（= Tauri invoke 边界 = Headless 请求体）。验证 JSON 请求→派发→JSON 响应闭环，
//! 以及错误进信封（永不 panic）、稳定错误码。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::{DesktopAppBuilder, dispatch_json};
use cmx_agent_core::{MockModel, ModelResponse, ToolCall};

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

fn app_with(tmp: &TempDir, model: MockModel) -> cmx_agent_app::AgentApp {
    DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(model))
        .build()
        .unwrap()
}

#[tokio::test]
async fn send_command_roundtrips_through_json() {
    let tmp = TempDir::new("proto-send");
    let app = app_with(
        &tmp,
        MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id(
                "c1",
                "add",
                serde_json::json!({"a":40,"b":2}),
            )]),
            ModelResponse::text("42"),
        ]),
    );
    let req = r#"{"cmd":"send","session_id":"s1","text":"算 40+2"}"#;
    let resp = dispatch_json(&app, req).await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["turn"], 1);
    assert_eq!(v["data"]["final_text"], "42");
    // new_events 里带 add 结果
    let has_sum = v["data"]["new_events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|e| e["output"]["sum"] == 42.0);
    assert!(has_sum);
}

#[tokio::test]
async fn create_list_get_delete_lifecycle() {
    let tmp = TempDir::new("proto-life");
    let app = app_with(&tmp, MockModel::saying("hi"));

    // create
    let r = dispatch_json(&app, r#"{"cmd":"create_session","id":"c1"}"#).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"],
        true
    );

    // send one turn so it has events
    let _ = dispatch_json(&app, r#"{"cmd":"send","session_id":"c1","text":"yo"}"#).await;

    // list
    let r = dispatch_json(&app, r#"{"cmd":"list_sessions"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    let sessions = v["data"]["sessions"].as_array().unwrap();
    assert!(sessions.iter().any(|m| m["id"] == "c1"));

    // get_events
    let r = dispatch_json(&app, r#"{"cmd":"get_events","session_id":"c1"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    assert!(!v["data"]["events"].as_array().unwrap().is_empty());

    // delete
    let r = dispatch_json(&app, r#"{"cmd":"delete_session","session_id":"c1"}"#).await;
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&r).unwrap()["ok"],
        true
    );

    // get after delete → not_found
    let r = dispatch_json(&app, r#"{"cmd":"get_events","session_id":"c1"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&r).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "not_found");
}

#[tokio::test]
async fn invalid_json_returns_bad_request_envelope_not_panic() {
    let tmp = TempDir::new("proto-badjson");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let resp = dispatch_json(&app, "{ this is not json").await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn unknown_command_is_bad_request() {
    let tmp = TempDir::new("proto-unknown");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let resp = dispatch_json(&app, r#"{"cmd":"frobnicate"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "bad_request");
}

#[tokio::test]
async fn bad_session_id_is_rejected_via_envelope() {
    let tmp = TempDir::new("proto-badid");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let resp = dispatch_json(&app, r#"{"cmd":"send","session_id":"../etc","text":"x"}"#).await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "bad_request");
}
