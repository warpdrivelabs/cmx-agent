//! 连接器测试。单测走离线（工具 spec / 信封 / 降级）；`#[ignore]` 的集成测试需真 live 服务手动跑。

use cmx_agent_connectors::{ConnectorConfig, ConnectorRegistry};
use cmx_agent_core::{SandboxMode, Tool, ToolCtx};
use std::path::PathBuf;

fn dummy_ctx_roots() -> Vec<PathBuf> {
    vec![]
}

#[test]
fn registry_exposes_three_connectors_with_tools() {
    let reg = ConnectorRegistry::new(ConnectorConfig::default());
    let descs = reg.descriptors();
    assert_eq!(descs.len(), 3);
    let ids: Vec<&str> = descs.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, vec!["flow", "onto", "report"]);
    // 每个连接器至少暴露一个工具
    for d in descs {
        assert!(!d.tools.is_empty(), "connector {} exposes no tools", d.id);
    }
}

#[test]
fn register_into_adds_connector_tools_to_registry() {
    let mut registry = cmx_agent_core::ToolRegistry::new();
    let reg = ConnectorRegistry::new(ConnectorConfig::default());
    reg.register_into(&mut registry);
    for t in [
        "flow_list_definitions",
        "onto_list_object_types",
        "report_list_reports",
    ] {
        assert!(registry.contains(t), "tool {t} not registered");
    }
}

#[test]
fn connector_tools_carry_read_guard_hints() {
    use cmx_agent_connectors::{CmxServiceClient, FlowConnector, OntoConnector};
    let flow = FlowConnector {
        client: CmxServiceClient::new("http://127.0.0.1:8091"),
    };
    let spec = flow.spec();
    assert_eq!(spec.guard.requires_auth.as_deref(), Some("flow:read"));
    assert!(spec.guard.idempotent);

    let onto = OntoConnector {
        client: CmxServiceClient::new("http://127.0.0.1:8097"),
    };
    assert_eq!(
        onto.spec().guard.requires_auth.as_deref(),
        Some("onto:read")
    );
}

/// 降级：指向一个**不存在**的端口，工具调用应返回 ok=false 的错误结果（不 panic）。
#[tokio::test]
async fn connector_tool_gracefully_degrades_when_service_down() {
    use cmx_agent_connectors::{CmxServiceClient, FlowConnector};
    // 指向一个几乎不可能有服务的端口
    let flow = FlowConnector {
        client: CmxServiceClient::new("http://127.0.0.1:9"),
    };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx {
        sandbox: SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
    };
    let r = flow.invoke(serde_json::json!({}), &ctx).await.unwrap();
    assert!(
        !r.ok,
        "unreachable service must yield error result, not panic"
    );
    assert!(r.output["error"].as_str().unwrap().contains("失败"));
}

/// 健康探测：不可达服务 → offline，不报错。
#[tokio::test]
async fn probe_offline_service_returns_offline_status() {
    use cmx_agent_connectors::{CmxServiceClient, probe};
    let client = CmxServiceClient::new("http://127.0.0.1:9");
    let status = probe(&client).await;
    assert!(!status.online);
    assert!(status.service_name.is_none());
    assert!(status.detail.is_some());
}

// ───────────────────── LIVE 集成测试（需真服务，默认 #[ignore]）─────────────────────
// 手动跑：先确保 cmx-flow 在 :8091 运行，再 `cargo test --offline -p cmx-agent-connectors -- --ignored`

#[tokio::test]
#[ignore = "需要 live cmx-flow 服务在 :8091"]
async fn live_flow_lists_real_definitions() {
    use cmx_agent_connectors::{
        AuthConfig, AuthProvider, CmxServiceClient, FlowConnector, TokenStore,
    };
    use std::sync::{Arc, RwLock};
    // 活体 cmx-flow 为 auth=on：先经门户登录取 token，注入共享令牌槽 → 连接器带 Bearer。
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth
        .login("admin", "Admin@12345")
        .await
        .expect("门户登录失败（需 live portal :8080 + 种子账号）");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let flow = FlowConnector {
        client: CmxServiceClient::new("http://127.0.0.1:8091")
            .with_identity("default", "admin")
            .with_token(store),
    };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx {
        sandbox: SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
    };
    let r = flow.invoke(serde_json::json!({}), &ctx).await.unwrap();
    assert!(r.ok, "live flow call failed: {:?}", r.output);
    assert_eq!(r.output["service"], "cmx-flow");
    assert!(
        r.output["count"].as_u64().unwrap() >= 1,
        "expected ≥1 process definition"
    );
}

#[tokio::test]
#[ignore = "需要 live cmx-flow 服务在 :8091"]
async fn live_probe_flow_online() {
    use cmx_agent_connectors::{CmxServiceClient, probe};
    let status = probe(&CmxServiceClient::new("http://127.0.0.1:8091")).await;
    assert!(status.online, "flow should be online: {:?}", status.detail);
    assert!(status.service_name.unwrap().contains("flow"));
}

#[tokio::test]
#[ignore = "需要 live cmx-flow :8091 + 门户 :8080 + 种子定义 s5_voucher"]
async fn live_flow_start_instance() {
    use cmx_agent_connectors::{
        AuthConfig, AuthProvider, CmxServiceClient, FlowStartInstance, TokenStore,
    };
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let start = FlowStartInstance {
        client: CmxServiceClient::new("http://127.0.0.1:8091")
            .with_identity("default", "admin")
            .with_token(store),
    };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx {
        sandbox: SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
    };
    let r = start
        .invoke(
            serde_json::json!({ "definitionKey": "s5_voucher", "variables": { "amount": 42 } }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(r.ok, "live start failed: {:?}", r.output);
    assert_eq!(r.output["service"], "cmx-flow");
    assert_eq!(r.output["started"], true);
    // 真实起实例返回实例 id（data.id / instanceId）+ ACTIVE 态。
    assert!(
        r.output["instanceId"].is_string() || r.output["data"]["id"].is_string(),
        "expected instance id: {:?}",
        r.output
    );
}

#[tokio::test]
#[ignore = "需要 live cmx-flow :8091 + 门户 :8080 + 种子定义 s5_voucher（assignee=admin）"]
async fn live_flow_start_then_complete() {
    use cmx_agent_connectors::{
        AuthConfig, AuthProvider, CmxServiceClient, FlowCompleteTask, FlowStartInstance, TokenStore,
    };
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let client = CmxServiceClient::new("http://127.0.0.1:8091")
        .with_identity("default", "admin")
        .with_token(store);
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx {
        sandbox: SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
    };
    // ① 起实例 → 取 instanceId + 首个待办 taskId。
    let start = FlowStartInstance { client: client.clone() };
    let sr = start
        .invoke(
            serde_json::json!({ "definitionKey": "s5_voucher", "variables": { "amount": 7 } }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(sr.ok, "start failed: {:?}", sr.output);
    let iid = sr.output["data"]["id"].as_str().expect("instance id").to_string();
    let tid = sr.output["data"]["tasks"][0]["id"]
        .as_str()
        .expect("task id")
        .to_string();
    // ② 办结该任务（assignee=admin，按 username 授权放行）→ 实例应 COMPLETED。
    let complete = FlowCompleteTask { client };
    let cr = complete
        .invoke(
            serde_json::json!({ "taskId": tid, "instanceId": iid, "decision": "approve" }),
            &ctx,
        )
        .await
        .unwrap();
    assert!(cr.ok, "complete failed: {:?}", cr.output);
    assert_eq!(cr.output["completed"], true);
    assert_eq!(cr.output["data"]["state"], "COMPLETED", "实例应办结: {:?}", cr.output);
}

#[tokio::test]
#[ignore = "需要 live cmx-report :8092 + 门户 :8080 + 种子报表"]
async fn live_report_lists_real_reports() {
    use cmx_agent_connectors::{
        AuthConfig, AuthProvider, CmxServiceClient, ReportConnector, TokenStore,
    };
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let report = ReportConnector {
        client: CmxServiceClient::new("http://127.0.0.1:8092")
            .with_identity("default", "admin")
            .with_token(store),
    };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx {
        sandbox: SandboxMode::WorkspaceWrite,
        allowed_roots: &roots,
    };
    let r = report.invoke(serde_json::json!({}), &ctx).await.unwrap();
    assert!(r.ok, "live report failed: {:?}", r.output);
    assert_eq!(r.output["service"], "cmx-report");
    assert_eq!(r.output["endpoint"], "/api/report-design/reports");
    assert!(
        r.output["count"].as_u64().unwrap() >= 1,
        "expected ≥1 report: {:?}",
        r.output
    );
}

// ── U11 引擎写侧：mock cmx-flow 服务，验证 flow_start_instance 真发 POST + 解信封 ──
#[tokio::test]
async fn flow_start_instance_posts_and_unwraps_envelope() {
    use cmx_agent_connectors::{CmxServiceClient, FlowStartInstance};
    use cmx_agent_core::guard::SandboxMode;
    use cmx_agent_core::{Tool, ToolCtx};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    if Command::new("python3").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) {
        eprintln!("python3 不可用，跳过");
        return;
    }
    // mock：POST /api/flow/v1/instances/start → 回信封 {code:0,data:{instanceId,status}}，回显收到的 definitionKey
    const MOCK: &str = r#"
import sys, json, http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('Content-Length','0')); body=json.loads(self.rfile.read(n) or b'{}')
        env={"code":0,"msg":"ok","data":{"instanceId":"inst-9","status":"RUNNING","echoKey":body.get("definitionKey")}}
        b=json.dumps(env).encode(); self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
httpd=socketserver.TCPServer(("127.0.0.1",0),H); print(httpd.server_address[1]); sys.stdout.flush(); httpd.serve_forever()
"#;
    let mut child = Command::new("python3").arg("-c").arg(MOCK).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let tool = FlowStartInstance { client: CmxServiceClient::new(format!("http://127.0.0.1:{port}")) };
    let roots = vec![std::path::PathBuf::from("/tmp")];
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = tool.invoke(serde_json::json!({"definitionKey":"leave_approval","variables":{"days":3}}), &ctx).await.unwrap();

    let _ = child.kill();
    let _ = child.wait();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["started"], true);
    assert_eq!(r.output["instanceId"], "inst-9");           // 信封 data 已解出
    assert_eq!(r.output["data"]["echoKey"], "leave_approval"); // 请求体真发过去
}

// U11 续：mock onto，验证 onto_execute_action 发 POST 到 /action-types/{name}/execute + 回显 params/dryRun
#[tokio::test]
async fn onto_execute_action_posts_params_and_dryrun() {
    use cmx_agent_connectors::{CmxServiceClient, OntoExecuteAction};
    use cmx_agent_core::guard::SandboxMode;
    use cmx_agent_core::{Tool, ToolCtx};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    if Command::new("python3").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) {
        return;
    }
    const MOCK: &str = r#"
import sys, json, http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('Content-Length','0')); body=json.loads(self.rfile.read(n) or b'{}')
        env={"code":0,"msg":"ok","data":{"path":self.path,"echoParams":body.get("params"),"dryRun":body.get("dryRun")}}
        b=json.dumps(env).encode(); self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
httpd=socketserver.TCPServer(("127.0.0.1",0),H); print(httpd.server_address[1]); sys.stdout.flush(); httpd.serve_forever()
"#;
    let mut child = Command::new("python3").arg("-c").arg(MOCK).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let tool = OntoExecuteAction { client: CmxServiceClient::new(format!("http://127.0.0.1:{port}")) };
    let roots = vec![std::path::PathBuf::from("/tmp")];
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = tool.invoke(serde_json::json!({"actionType":"wsClose","params":{"orderId":"WO-1"},"dryRun":true}), &ctx).await.unwrap();

    let _ = child.kill();
    let _ = child.wait();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["executed"], true);
    assert_eq!(r.output["dryRun"], true);
    assert_eq!(r.output["data"]["path"], "/api/onto/v1/action-types/wsClose/execute");
    assert_eq!(r.output["data"]["echoParams"]["orderId"], "WO-1");
}

// U13：mock cmx-data-auth PDP，验证 PEP decide→缓存→cached_allow（permit 放行 / deny 拒绝）
#[tokio::test]
async fn dataauth_pep_permits_and_denies_via_mock_pdp() {
    use cmx_agent_connectors::DataAuthPep;
    use cmx_agent_core::guard::Subject;
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    if Command::new("python3").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) {
        return;
    }
    // mock PDP：resource.action=="write" → deny，其余 permit（回显判定）
    const MOCK: &str = r#"
import sys, json, http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('Content-Length','0')); body=json.loads(self.rfile.read(n) or b'{}')
        act=body.get("resource",{}).get("action")
        eff="deny" if act=="write" else "permit"
        env={"code":0,"msg":"ok","data":{"effect":eff}}
        b=json.dumps(env).encode(); self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
httpd=socketserver.TCPServer(("127.0.0.1",0),H); print(httpd.server_address[1]); sys.stdout.flush(); httpd.serve_forever()
"#;
    let mut child = Command::new("python3").arg("-c").arg(MOCK).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let pep = DataAuthPep::new(format!("http://127.0.0.1:{port}"), "default", "alice", true);
    let subj = Subject::new("alice");
    // 判定：read → permit，write → deny
    assert!(pep.decide(&subj, "flow:read").await, "read 应放行");
    assert!(!pep.decide(&subj, "flow:write").await, "write 应拒绝");
    // 判定后走缓存（同步查）
    assert!(pep.cached_allow(&subj, "flow:read"));
    assert!(!pep.cached_allow(&subj, "flow:write"));

    let _ = child.kill();
    let _ = child.wait();
}

// U12：mock 三服务端点，验证 enterprise_context 并发聚合对象类型/动作/流程/报表
#[tokio::test]
async fn enterprise_context_aggregates_domain_model() {
    use cmx_agent_connectors::{CmxServiceClient, EnterpriseContext};
    use cmx_agent_core::guard::SandboxMode;
    use cmx_agent_core::{Tool, ToolCtx};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    if Command::new("python3").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) {
        return;
    }
    // 一个 mock 服务按 path 分派：object-types / action-types / link-types / flow definitions / reports
    const MOCK: &str = r#"
import sys, json, http.server, socketserver
R={
 "/api/onto/v1/object-types":[{"apiName":"WsOrd","displayName":"订单","primaryKey":"id","propertyCount":5}],
 "/api/onto/v1/link-types":[{"apiName":"ordOfCust","fromObjectType":"WsOrd","toObjectType":"WsCust"}],
 "/api/onto/v1/action-types":[{"apiName":"wsClose","displayName":"关闭订单","status":"active"}],
 "/api/flow/v1/definitions":{"definitions":[{"key":"leave","name":"请假","startable":True}]},
 "/api/rpt/v1/reports":[{"code":"CBS","name":"合并资产负债表"}],
}
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_GET(self):
        data=R.get(self.path)
        if data is None: self.send_response(404); self.end_headers(); return
        env={"code":0,"msg":"ok","data":data}; b=json.dumps(env).encode()
        self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
httpd=socketserver.TCPServer(("127.0.0.1",0),H); print(httpd.server_address[1]); sys.stdout.flush(); httpd.serve_forever()
"#;
    let mut child = Command::new("python3").arg("-c").arg(MOCK).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let base = format!("http://127.0.0.1:{port}");

    let tool = EnterpriseContext {
        onto: CmxServiceClient::new(base.clone()),
        flow: CmxServiceClient::new(base.clone()),
        report: CmxServiceClient::new(base),
    };
    let roots = vec![std::path::PathBuf::from("/tmp")];
    let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots };
    let r = tool.invoke(serde_json::json!({}), &ctx).await.unwrap();

    let _ = child.kill();
    let _ = child.wait();
    assert!(r.ok, "{r:?}");
    let c = &r.output["context"];
    assert_eq!(c["objectTypes"]["items"][0]["apiName"], "WsOrd");
    assert_eq!(c["relations"]["items"][0]["from"], "WsOrd");
    assert_eq!(c["actions"]["items"][0]["apiName"], "wsClose");
    assert_eq!(c["flows"]["items"][0]["key"], "leave");
    assert_eq!(c["reports"]["items"][0]["code"], "CBS");
}

// U14：mock 引擎，验证 business_chain 按序跑 + 步间 $1.data.id 引用穿线（建对象 id → 起流程 businessKey）
#[tokio::test]
async fn business_chain_threads_refs_across_steps() {
    use cmx_agent_connectors::{CmxServiceClient, EngineChain};
    use cmx_agent_core::guard::SandboxMode;
    use cmx_agent_core::{Tool, ToolCtx};
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};

    if Command::new("python3").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) {
        return;
    }
    // mock：/objects/* → data.id="OBJ-7"；/instances/start → 回显收到的 businessKey
    const MOCK: &str = r#"
import sys, json, http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_POST(self):
        n=int(self.headers.get('Content-Length','0')); body=json.loads(self.rfile.read(n) or b'{}')
        if self.path.startswith("/api/onto/v1/objects/"):
            data={"id":"OBJ-7","props":body.get("properties")}
        elif self.path=="/api/flow/v1/instances/start":
            data={"instanceId":"INST-3","gotBusinessKey":body.get("businessKey")}
        else:
            data={"path":self.path}
        env={"code":0,"msg":"ok","data":data}; b=json.dumps(env).encode()
        self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
httpd=socketserver.TCPServer(("127.0.0.1",0),H); print(httpd.server_address[1]); sys.stdout.flush(); httpd.serve_forever()
"#;
    let mut child = Command::new("python3").arg("-c").arg(MOCK).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let base = format!("http://127.0.0.1:{port}");

    let tool = EngineChain {
        onto: CmxServiceClient::new(base.clone()),
        flow: CmxServiceClient::new(base.clone()),
        report: CmxServiceClient::new(base),
        pep: None,
        identity: None,
    };
    let roots = vec![std::path::PathBuf::from("/tmp")];
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    // 步1 建对象；步2 起流程，businessKey 引用步1输出的 id
    let r = tool.invoke(serde_json::json!({"steps":[
        {"op":"put_object","input":{"objectType":"WsOrd","properties":{"amount":500}}},
        {"op":"start_instance","input":{"definitionKey":"approve","businessKey":"$1.id"}}
    ]}), &ctx).await.unwrap();

    let _ = child.kill();
    let _ = child.wait();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["completed"], true);
    assert_eq!(r.output["stepCount"], 2);
    assert_eq!(r.output["steps"][0]["output"]["id"], "OBJ-7");
    // 关键：步2 收到的 businessKey = 步1 的 id（引用穿线成功）
    assert_eq!(r.output["steps"][1]["output"]["gotBusinessKey"], "OBJ-7");
}

// ───────────────────── LIVE 写侧集成（需真服务 + 门户，默认 #[ignore]）─────────────────────

#[tokio::test]
#[ignore = "需要 live cmx-ontology :8097 + 门户 :8080"]
async fn live_onto_put_object() {
    use cmx_agent_connectors::{AuthConfig, AuthProvider, CmxServiceClient, OntoPutObject, TokenStore};
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let client = CmxServiceClient::new("http://127.0.0.1:8097")
        .with_identity("default", "admin")
        .with_token(store);
    // 幂等 seed 一个对象类型 Widget（saved:true 即使已存在）
    let _ = client
        .post_write(
            "/api/onto/v1/object-types",
            &serde_json::json!({
                "apiName":"Widget","displayName":"小部件","primaryKey":"id",
                "properties":[{"apiName":"id","displayName":"ID","dataType":"string"},
                              {"apiName":"name","displayName":"名称","dataType":"string"}]
            }),
        )
        .await;
    let put = OntoPutObject { client };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = put
        .invoke(
            serde_json::json!({"objectType":"Widget","properties":{"id":"w-live-1","name":"活体测试"}}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(r.ok, "live onto put failed: {:?}", r.output);
    assert_eq!(r.output["service"], "cmx-ontology");
    assert_eq!(r.output["objectType"], "Widget");
}

#[tokio::test]
#[ignore = "需要 live cmx-report :8092 + 门户 :8080 + 种子报表 STAT_01_D"]
async fn live_report_compute() {
    use cmx_agent_connectors::{AuthConfig, AuthProvider, CmxServiceClient, ReportCompute, TokenStore};
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let comp = ReportCompute {
        client: CmxServiceClient::new("http://127.0.0.1:8092")
            .with_identity("default", "admin")
            .with_token(store),
    };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = comp
        .invoke(
            serde_json::json!({"reportCode":"STAT_01_D","orgCode":"0000","periodCode":"2026-07"}),
            &ctx,
        )
        .await
        .unwrap();
    assert!(r.ok, "live report compute failed: {:?}", r.output);
    assert_eq!(r.output["service"], "cmx-report");
    // 真实算表返回 data.computed（算了几格）
    assert!(r.output["data"]["computed"].is_number(), "应返回计算格数: {:?}", r.output);
}

#[tokio::test]
#[ignore = "需要 live cmx-data-auth :8098"]
async fn live_dataauth_decide_permits_admin() {
    use cmx_agent_connectors::DataAuthPep;
    use cmx_agent_core::Subject;
    // 超管角色 → PDP 全放行（note「超管角色 → 全放行」）
    let mut subj = Subject::new("admin");
    subj.roles = vec!["admin".into()];
    let pep = DataAuthPep::new("http://127.0.0.1:8098", "default", "admin", true);
    // decide 打真 PDP /api/dataauth/v1/decide → effect=permit → true
    assert!(pep.decide(&subj, "flow:write").await, "admin 应被 PDP 放行 flow:write");
    // 写缓存后同步查也放行
    assert!(pep.cached_allow(&subj, "flow:write"));
}

#[tokio::test]
#[ignore = "需要 live cmx-ontology :8097 + 门户 :8080"]
async fn live_onto_execute_action() {
    use cmx_agent_connectors::{AuthConfig, AuthProvider, CmxServiceClient, OntoExecuteAction, TokenStore};
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let client = CmxServiceClient::new("http://127.0.0.1:8097")
        .with_identity("default", "admin")
        .with_token(store);
    // 幂等 seed：Widget 类型 + 对象 w1 + 带 modifyObject logic 的 renameWidget 动作。
    let _ = client.post_write("/api/onto/v1/object-types", &serde_json::json!({
        "apiName":"Widget","displayName":"小部件","primaryKey":"id",
        "properties":[{"apiName":"id","displayName":"ID","dataType":"string"},{"apiName":"name","displayName":"名称","dataType":"string"}]
    })).await;
    let _ = client.post_write("/api/onto/v1/objects/Widget", &serde_json::json!({"properties":{"id":"w1","name":"orig"}})).await;
    let _ = client.post_write("/api/onto/v1/action-types", &serde_json::json!({
        "apiName":"renameWidget","displayName":"改名",
        "parameters":[{"name":"id","required":true},{"name":"name","required":true}],
        "logic":[{"op":"modifyObject","objectType":"Widget","pk":"$id","set":{"name":"$name"}}]
    })).await;
    let exec = OntoExecuteAction { client };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    // dryRun：应解析出 1 条编辑、不落库。
    let r = exec.invoke(serde_json::json!({"actionType":"renameWidget","params":{"id":"w1","name":"renamed"},"dryRun":true}), &ctx).await.unwrap();
    assert!(r.ok, "live onto execute failed: {:?}", r.output);
    assert_eq!(r.output["service"], "cmx-ontology");
    assert_eq!(r.output["dryRun"], true);
    assert_eq!(r.output["data"]["applied"], 1, "应解析出 1 条编辑: {:?}", r.output);
}

#[tokio::test]
#[ignore = "需要 live cmx-onto/flow/report + 门户 + seed(Widget/s5_voucher/STAT_01_D)"]
async fn live_business_chain_end_to_end() {
    use cmx_agent_connectors::{AuthConfig, AuthProvider, CmxServiceClient, EngineChain, TokenStore};
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let mk = |port| CmxServiceClient::new(format!("http://127.0.0.1:{port}")).with_identity("default","admin").with_token(store.clone());
    let onto = mk(8097);
    // seed Widget 类型（幂等）
    let _ = onto.post_write("/api/onto/v1/object-types", &serde_json::json!({
        "apiName":"Widget","displayName":"小部件","primaryKey":"id",
        "properties":[{"apiName":"id","displayName":"ID","dataType":"string"},{"apiName":"name","displayName":"名称","dataType":"string"}]
    })).await;
    let chain = EngineChain { onto, flow: mk(8091), report: mk(8092), pep: None, identity: None };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    // 整链：① 建对象 → ② 起流程(businessKey 引用①的 id) → ③ 算报表。验 $N 穿线 + 各步 ok。
    let r = chain.invoke(serde_json::json!({"steps":[
        {"op":"put_object","input":{"objectType":"Widget","properties":{"id":"chain-obj-1","name":"链测"}}},
        {"op":"start_instance","input":{"definitionKey":"s5_voucher","businessKey":"$1.id","variables":{"amount":1}}},
        {"op":"compute_report","input":{"reportCode":"STAT_01_D","orgCode":"0000","periodCode":"2026-07"}}
    ]}), &ctx).await.unwrap();
    assert!(r.ok, "chain failed: {:?}", r.output);
    let steps = r.output["steps"].as_array().expect("steps");
    assert_eq!(steps.len(), 3, "应 3 步: {:?}", r.output);
    for s in steps {
        assert_eq!(s["ok"], true, "步骤失败: {:?}", s);
    }
}

#[tokio::test]
#[ignore = "需要 live cmx-data-auth :8098 + cmx-onto :8097 + 门户"]
async fn live_business_chain_per_op_pep_denies_viewer() {
    use cmx_agent_connectors::{AuthConfig, AuthProvider, CmxServiceClient, DataAuthPep, EngineChain, TokenStore};
    use cmx_agent_core::Subject;
    use std::sync::{Arc, RwLock};
    let auth = AuthProvider::new(AuthConfig::default());
    let user = auth.login("admin", "Admin@12345").await.expect("门户登录");
    let store: TokenStore = Arc::new(RwLock::new(Some(user.access_token)));
    let mk = |port| CmxServiceClient::new(format!("http://127.0.0.1:{port}")).with_identity("default","admin").with_token(store.clone());
    // 授权主体设为 viewer（无写权限）→ 链内第一步 put_object(onto:write) 应被 PDP 拒。
    let mut viewer = Subject::new("viewer-bob");
    viewer.roles = vec!["viewer".into()];
    let identity = Arc::new(RwLock::new(viewer));
    let pep = Arc::new(DataAuthPep::new("http://127.0.0.1:8098", "default", "viewer-bob", true));
    let chain = EngineChain { onto: mk(8097), flow: mk(8091), report: mk(8092),
        pep: Some(pep), identity: Some(identity) };
    let roots = dummy_ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = chain.invoke(serde_json::json!({"steps":[
        {"op":"put_object","input":{"objectType":"Widget","properties":{"id":"deny-1","name":"x"}}}
    ]}), &ctx).await.unwrap();
    // 链因逐步 PEP 在第 1 步停：该步 ok=false 且原因含权限不足。
    let step0 = &r.output["steps"][0];
    assert_eq!(step0["ok"], false, "viewer 的写步应被拒: {:?}", r.output);
    assert!(step0["error"].as_str().unwrap_or("").contains("权限不足"), "应是权限不足: {:?}", step0);
}
