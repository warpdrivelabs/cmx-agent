//! 自助注册回归：前门免登录白名单（该拒被拒负例）、注册入口显隐开关、本地密码策略快速失败。
//!
//! 门户不可达的负例用 `http://127.0.0.1:1`（连接拒绝，离线快速）——注册命令应**穿过登录门**
//! 到达网络层（错误是「无法连接认证服务」而非「未登录」），证明白名单放行正确。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::{DesktopAppBuilder, dispatch_json};
use cmx_agent_core::MockModel;

struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-agent-reg-{tag}-{n}"));
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

/// 带登录门的 app：门户指向本地拒绝端口（离线、连接即拒），未认证。
fn app_with_auth(tmp: &TempDir) -> cmx_agent_app::AgentApp {
    DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(MockModel::saying("hi")))
        .auth(cmx_agent_app::AuthConfig {
            base_url: "http://127.0.0.1:1".into(),
        })
        .build()
        .unwrap()
}

fn as_json(raw: &str) -> serde_json::Value {
    serde_json::from_str(raw).expect("信封必为合法 JSON")
}

#[tokio::test]
async fn register_and_ui_config_pass_login_gate_while_send_rejected() {
    let tmp = TempDir::new("gate");
    let app = app_with_auth(&tmp);

    // 该拒被拒：未登录时 send 一律拒（登录门仍在）。
    let send = as_json(&dispatch_json(&app, r#"{"cmd":"send","session_id":"s1","text":"hi"}"#).await);
    assert_eq!(send["ok"], false);
    assert_eq!(send["error"]["code"], "auth_error");
    assert!(send["error"]["message"]
        .as_str()
        .unwrap()
        .contains("未登录"));

    // UiConfig 免登录：未认证可查（登录页要用它决定注册入口显隐）。
    let ui = as_json(&dispatch_json(&app, r#"{"cmd":"ui_config"}"#).await);
    assert_eq!(ui["ok"], true);
    assert_eq!(ui["data"]["register_enabled"], true);

    // Register 免登录：穿过登录门到达网络层（127.0.0.1:1 连接拒绝 → 「无法连接认证服务」），
    // 而非登录门的「未登录」——证明白名单放行。
    let reg = as_json(
        &dispatch_json(
            &app,
            r#"{"cmd":"register","username":"newuser","password":"Abcd1234!"}"#,
        )
        .await,
    );
    assert_eq!(reg["ok"], false);
    let msg = reg["error"]["message"].as_str().unwrap();
    assert!(
        msg.contains("无法连接认证服务") || msg.contains("注册未开放"),
        "注册应到达网络层（离线负例），实际: {msg}"
    );
    assert!(!msg.contains("未登录"), "注册不得被登录门拦截: {msg}");
}

#[tokio::test]
async fn ui_config_reflects_register_enabled_toggle() {
    let tmp = TempDir::new("toggle");
    let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(MockModel::saying("hi")))
        .with_register_enabled(false)
        .build()
        .unwrap();
    let ui = as_json(&dispatch_json(&app, r#"{"cmd":"ui_config"}"#).await);
    assert_eq!(ui["data"]["register_enabled"], false);
}

#[tokio::test]
async fn register_rejects_blank_input_before_network() {
    let tmp = TempDir::new("blank");
    let app = app_with_auth(&tmp);
    let r = as_json(
        &dispatch_json(&app, r#"{"cmd":"register","username":"  ","password":"Abcd1234!"}"#).await,
    );
    assert_eq!(r["ok"], false);
    assert!(r["error"]["message"].as_str().unwrap().contains("请输入用户名和密码"));
}

#[tokio::test]
async fn register_rejects_weak_password_locally() {
    // 本地镜像门户复杂度策略快速失败：不触网、不建号（该拒被拒负例）。
    let tmp = TempDir::new("weakpwd");
    let app = app_with_auth(&tmp);
    let r = as_json(
        &dispatch_json(&app, r#"{"cmd":"register","username":"newuser","password":"123"}"#).await,
    );
    assert_eq!(r["ok"], false);
    let msg = r["error"]["message"].as_str().unwrap();
    assert!(msg.contains("密码长度"), "应命中本地策略：{msg}");
}
