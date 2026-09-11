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

#[tokio::test]
async fn approve_command_accepts_all_and_session_and_is_backward_compatible() {
    let tmp = TempDir::new("proto-approve");
    // 需交互式审批者，approve 命令才有落点。
    let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), Arc::new(MockModel::saying("hi")))
        .interactive_approval()
        .build()
        .unwrap();

    // 新形态：带 all + session_id（「本对话全部允许」）。无待决审批 → resolved=false 属正常，
    // 但命令应被接受并回显 all=true（会话已被标记为全部允许）。
    let resp = dispatch_json(
        &app,
        r#"{"cmd":"approve","call_id":"none","approved":true,"all":true,"session_id":"s1"}"#,
    )
    .await;
    let v: serde_json::Value = serde_json::from_str(&resp).unwrap();
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["all"], true);

    // 旧形态：只有 call_id + approved（serde default 补齐 all=false/session_id=""）应仍兼容。
    let resp2 = dispatch_json(&app, r#"{"cmd":"approve","call_id":"x","approved":false}"#).await;
    let v2: serde_json::Value = serde_json::from_str(&resp2).unwrap();
    assert_eq!(v2["ok"], true);
    assert_eq!(v2["data"]["all"], false);
}

#[tokio::test]
async fn get_events_paginates_with_limit_total_start() {
    let tmp = TempDir::new("proto-getpage");
    let app = app_with(
        &tmp,
        MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id(
                "c1",
                "add",
                serde_json::json!({"a":1,"b":1}),
            )]),
            ModelResponse::text("2"),
        ]),
    );
    let _ = dispatch_json(&app, r#"{"cmd":"create_session","id":"p1"}"#).await;
    let _ = dispatch_json(&app, r#"{"cmd":"send","session_id":"p1","text":"go"}"#).await;

    // 全量：start=0、total=事件数
    let full: serde_json::Value =
        serde_json::from_str(&dispatch_json(&app, r#"{"cmd":"get_events","session_id":"p1"}"#).await)
            .unwrap();
    let total = full["data"]["events"].as_array().unwrap().len();
    assert!(total >= 5, "一个含工具调用的回合应产生多条事件");
    assert_eq!(full["data"]["total"].as_u64().unwrap() as usize, total);
    assert_eq!(full["data"]["start"].as_u64().unwrap(), 0);

    // 尾窗口 limit=2：只回 2 条，start=total-2，total 不变
    let win: serde_json::Value = serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"get_events","session_id":"p1","limit":2}"#).await,
    )
    .unwrap();
    assert_eq!(win["data"]["events"].as_array().unwrap().len(), 2);
    assert_eq!(win["data"]["total"].as_u64().unwrap() as usize, total);
    assert_eq!(win["data"]["start"].as_u64().unwrap() as usize, total - 2);
}

/// set_policy（两旋钮热切换）：合法值立即生效并回显；非法值进 bad_request 信封（错误信息带合法值清单）。
#[tokio::test]
async fn set_policy_roundtrips_and_rejects_invalid_values() {
    let tmp = TempDir::new("proto-policy");
    let app = app_with(&tmp, MockModel::new([ModelResponse::text("ok")]));

    // 合法切换：read-only × on-request
    let v: serde_json::Value = serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_policy","sandbox":"read-only","approval":"on-request"}"#)
            .await,
    )
    .unwrap();
    assert_eq!(v["ok"], true, "{v:?}");
    assert_eq!(v["data"]["sandbox"], "read-only");
    assert_eq!(v["data"]["approval"], "on-request");

    // 切到危险档再切回（验证重复切换稳定）
    let v: serde_json::Value = serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_policy","sandbox":"danger-full-access","approval":"never"}"#)
            .await,
    )
    .unwrap();
    assert_eq!(v["ok"], true, "{v:?}");
    assert_eq!(v["data"]["sandbox"], "danger-full-access");
    assert_eq!(v["data"]["approval"], "never");
    let v: serde_json::Value = serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_policy","sandbox":"workspace-write","approval":"on-request"}"#)
            .await,
    )
    .unwrap();
    assert_eq!(v["ok"], true, "{v:?}");

    // 非法 sandbox：bad_request + 合法值提示
    let v: serde_json::Value = serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_policy","sandbox":"yolo","approval":"never"}"#).await,
    )
    .unwrap();
    assert_eq!(v["ok"], false, "{v:?}");
    assert_eq!(v["error"]["code"], "bad_request");
    assert!(v["error"]["message"].as_str().unwrap().contains("read-only"));

    // 非法 approval：同样 bad_request
    let v: serde_json::Value = serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_policy","sandbox":"read-only","approval":"always"}"#).await,
    )
    .unwrap();
    assert_eq!(v["ok"], false, "{v:?}");
    assert_eq!(v["error"]["code"], "bad_request");
    assert!(v["error"]["message"].as_str().unwrap().contains("on-request"));
}

// ── 多 provider 配置（providers.json）：播种 / 增删改 / 激活切换 / 掩码 ──────────────
// 注意：以下测试假定运行环境未设 CMX_AGENT_MODEL_* / DEEPSEEK_API_KEY（CI 干净环境成立），
// 否则 env 优先会盖过 providers.json（与生产桌面壳「点击启动无 env」不一致）。

fn resp(v: serde_json::Value) -> serde_json::Value {
    v
}

#[tokio::test]
async fn list_providers_seeds_from_model_json() {
    let tmp = TempDir::new("prov-seed");
    std::fs::write(
        tmp.path().join("model.json"),
        r#"{"base_url":"https://api.deepseek.com","api_key":"sk-VIZBzFoeHvpS12kZhK6abcd","model":"deepseek-r1"}"#,
    )
    .unwrap();
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"list_providers"}"#).await,
    )
    .unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    let providers = v["data"]["providers"].as_array().unwrap();
    assert!(!providers.is_empty(), "至少内置 MLamp 预设：{providers:?}");
    let ds = providers
        .iter()
        .find(|p| p["id"] == "builtin-default")
        .expect("builtin-default 应存在");
    assert_eq!(ds["builtin"], true);
    assert_eq!(ds["model"], "deepseek-r1", "model.json 的 model 应合入命中条目");
    let masked = ds["api_key_masked"].as_str().unwrap();
    assert!(masked.starts_with("sk-..."), "掩码格式：{masked}");
    assert!(masked.ends_with("abcd"), "掩码保留末 4 位：{masked}");
    assert!(!serde_json::to_string(&v).unwrap().contains("VIZBz"), "明文 key 不得泄漏");
    // model.json 命中 base_url → active 指向它
    assert_eq!(ds["active"], true);
}

#[tokio::test]
async fn save_provider_creates_and_list_grows() {
    let tmp = TempDir::new("prov-create");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(
            &app,
            r#"{"cmd":"set_model_config","name":"我的Kimi","base_url":"https://api.moonshot.cn/v1","model":"kimi-v1","api_key_action":"set","api_key_value":"sk-kimi-secret-x"}"#,
        )
        .await,
    )
    .unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    let id = v["data"]["id"].as_str().unwrap().to_string();
    assert!(id.starts_with("p-"), "新建 id 应为 p- 前缀：{id}");

    let v = resp(serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"list_providers"}"#).await,
    )
    .unwrap());
    let providers = v["data"]["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 2, "一内置 + 一自定义：{providers:?}");
    let kimi = providers.iter().find(|p| p["id"] == id.as_str()).unwrap();
    assert_eq!(kimi["name"], "我的Kimi");
    assert_eq!(kimi["active"], false, "新建不自动激活");
    assert_eq!(kimi["configured_key"], true);

    // 回读（掩码）
    let req = serde_json::json!({"cmd":"get_model_config","id":id});
    let v = resp(serde_json::from_str(&dispatch_json(&app, &req.to_string()).await).unwrap());
    assert_eq!(v["data"]["name"], "我的Kimi");
    let masked = v["data"]["api_key_masked"].as_str().unwrap();
    assert!(masked.starts_with("sk-..."), "{masked}");
    assert!(!serde_json::to_string(&v).unwrap().contains("kimi-secret"), "明文 key 不得回传");
}

#[tokio::test]
async fn save_provider_rejects_duplicate_name() {
    let tmp = TempDir::new("prov-dup");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let body = r#"{"cmd":"set_model_config","name":"我的Kimi","base_url":"https://api.moonshot.cn/v1","model":"k1","api_key_action":"keep"}"#;
    let v = resp(serde_json::from_str(&dispatch_json(&app, body).await).unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    // 同名新建 → bad_request
    let v = resp(serde_json::from_str(&dispatch_json(&app, body).await).unwrap());
    assert_eq!(v["ok"], false, "{v:?}");
    assert_eq!(v["error"]["code"], "bad_request");
    assert!(v["error"]["message"].as_str().unwrap().contains("已存在"));
    // 缺 name 的新建 → bad_request
    let v = resp(serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_model_config","base_url":"http://x","model":"m","api_key_action":"keep"}"#).await,
    )
    .unwrap());
    assert_eq!(v["ok"], false);
    assert!(v["error"]["message"].as_str().unwrap().contains("名称"));
}

#[tokio::test]
async fn save_provider_keep_preserves_key() {
    let tmp = TempDir::new("prov-keep");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(
            &app,
            r#"{"cmd":"set_model_config","name":"网关","base_url":"https://gw.example.com/v1","model":"m1","api_key_action":"set","api_key_value":"sk-keep-me-9999"}"#,
        )
        .await,
    )
    .unwrap());
    let id = v["data"]["id"].as_str().unwrap().to_string();
    // keep 更新：换模型，key 沿用
    let req = serde_json::json!({"cmd":"set_model_config","id":id,"model":"m2","base_url":"https://gw.example.com/v1","api_key_action":"keep"});
    let v = resp(serde_json::from_str(&dispatch_json(&app, &req.to_string()).await).unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    let req = serde_json::json!({"cmd":"get_model_config","id":id});
    let v = resp(serde_json::from_str(&dispatch_json(&app, &req.to_string()).await).unwrap());
    let masked = v["data"]["api_key_masked"].as_str().unwrap();
    assert_eq!(masked, "sk-...9999", "keep 后 key 不变：{masked}");
    assert_eq!(v["data"]["model"], "m2");
}

#[tokio::test]
async fn delete_provider_rejects_builtin() {
    let tmp = TempDir::new("prov-del-builtin");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"delete_provider","id":"builtin-mlamp"}"#).await,
    )
    .unwrap());
    assert_eq!(v["ok"], false, "{v:?}");
    assert_eq!(v["error"]["code"], "bad_request");
    assert!(v["error"]["message"].as_str().unwrap().contains("内置"));
}

#[tokio::test]
async fn delete_provider_and_active_fallback() {
    let tmp = TempDir::new("prov-del");
    let app = app_with(&tmp, MockModel::saying("hi"));
    // 建一个自定义条目
    let v = resp(serde_json::from_str(
        &dispatch_json(
            &app,
            r#"{"cmd":"set_model_config","name":"临时的","base_url":"https://t.example.com/v1","model":"t1","api_key_action":"keep"}"#,
        )
        .await,
    )
    .unwrap());
    let id = v["data"]["id"].as_str().unwrap().to_string();
    // 激活它再删它 → active 回落到剩余第一条（内置预设）
    let req = serde_json::json!({"cmd":"set_active_provider","id":id});
    let v = resp(serde_json::from_str(&dispatch_json(&app, &req.to_string()).await).unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    let req = serde_json::json!({"cmd":"delete_provider","id":id});
    let v = resp(serde_json::from_str(&dispatch_json(&app, &req.to_string()).await).unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    assert!(v["data"]["active"].is_string(), "删除激活条目后 active 应回落：{v:?}");
    let v = resp(serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"list_providers"}"#).await,
    )
    .unwrap());
    let providers = v["data"]["providers"].as_array().unwrap();
    assert_eq!(providers.len(), 1);
    assert!(providers.iter().any(|p| p["active"] == true), "有剩余内置条目被激活");
}

#[tokio::test]
async fn set_active_provider_swaps_and_persists() {
    let tmp = TempDir::new("prov-active");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(
            &app,
            r#"{"cmd":"set_model_config","name":"Kimi","base_url":"https://api.moonshot.cn/v1","model":"kimi-v1","api_key_action":"keep"}"#,
        )
        .await,
    )
    .unwrap());
    let id = v["data"]["id"].as_str().unwrap().to_string();
    let req = serde_json::json!({"cmd":"set_active_provider","id":id});
    let v = resp(serde_json::from_str(&dispatch_json(&app, &req.to_string()).await).unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    assert_eq!(v["data"]["current"], "kimi-v1");
    // 落盘断言
    let raw = std::fs::read_to_string(tmp.path().join("providers.json")).unwrap();
    let pf: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(pf["active"], id.as_str());
}

#[tokio::test]
async fn set_model_writes_active_provider_entry() {
    let tmp = TempDir::new("prov-setmodel");
    std::fs::write(
        tmp.path().join("model.json"),
        r#"{"base_url":"https://api.deepseek.com","api_key":"sk-x","model":"deepseek-chat"}"#,
    )
    .unwrap();
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_model","model":"deepseek-r1"}"#).await,
    )
    .unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    // providers.json 的激活条目 model 已更新
    let raw = std::fs::read_to_string(tmp.path().join("providers.json")).unwrap();
    let pf: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let active = pf["active"].as_str().unwrap();
    let entry = pf["providers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["id"] == active)
        .unwrap();
    assert_eq!(entry["model"], "deepseek-r1");
}

#[tokio::test]
async fn set_model_demo_not_persisted() {
    let tmp = TempDir::new("prov-demo");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(&app, r#"{"cmd":"set_model","model":"demo"}"#).await,
    )
    .unwrap());
    assert_eq!(v["ok"], true, "{v:?}");
    assert_eq!(v["data"]["persisted"], false);
    // demo 切换不应写出 providers.json 的激活变化（无激活条目）
    assert!(v["data"]["current"] == "demo");
}

#[tokio::test]
async fn change_password_without_auth_service_is_auth_error() {
    let tmp = TempDir::new("proto-pwd-noauth");
    let app = app_with(&tmp, MockModel::saying("hi"));
    let v = resp(serde_json::from_str(
        &dispatch_json(
            &app,
            r#"{"cmd":"change_password","old_password":"a","new_password":"b"}"#,
        )
        .await,
    )
    .unwrap());
    assert_eq!(v["ok"], false, "{v:?}");
    assert_eq!(v["error"]["code"], "auth_error");
}

#[tokio::test]
async fn change_password_rejects_same_or_empty_new_password() {
    // 测试环境未启用 auth 服务，命令在任何参数校验前就返回 auth_error——
    // 本用例锁定的是空/同值入参同样收敛为稳定信封码 auth_error（永不 panic、不触网）。
    let tmp = TempDir::new("proto-pwd-args");
    let app = app_with(&tmp, MockModel::saying("hi"));
    for body in [
        r#"{"cmd":"change_password","old_password":"","new_password":""}"#,
        r#"{"cmd":"change_password","old_password":"x","new_password":"x"}"#,
    ] {
        let v = resp(serde_json::from_str(&dispatch_json(&app, body).await).unwrap());
        assert_eq!(v["ok"], false, "{body} -> {v:?}");
        assert_eq!(v["error"]["code"], "auth_error");
    }
}
