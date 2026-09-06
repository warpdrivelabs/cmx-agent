//! 插件加载 + 执行：command 插件跑 echo；http 插件打 mock；plugin_list/install 往返。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_core::guard::SandboxMode;
use cmx_agent_core::{Tool, ToolCtx};
use cmx_agent_plugin::{
    build_plugin_tools, install_manifest, load_plugins, scan_plugin_summaries, uninstall_plugin,
};
use serde_json::json;

fn tmp(tag: &str) -> PathBuf {
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let p = std::env::temp_dir().join(format!("cmx-plugin-{tag}-{n}"));
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn ctx_roots() -> Vec<PathBuf> { vec![PathBuf::from("/tmp")] }

#[tokio::test]
async fn command_plugin_loads_and_runs() {
    let dir = tmp("cmd");
    let pdir = dir.join("hello");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(
        pdir.join("cmx-plugin.json"),
        r#"{"name":"say_hi","kind":"command","description":"打招呼",
            "command":"echo","args":["hi {who}"]}"#,
    ).unwrap();

    let (tools, manifests) = load_plugins(&dir);
    assert_eq!(manifests.len(), 1);
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].spec().name, "say_hi");

    let roots = ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = tools[0].invoke(json!({"who":"世界"}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["stdout"], "hi 世界"); // {who} 占位替换 + 命令执行
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn http_plugin_calls_endpoint() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    if Command::new("python3").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) { return; }
    const MOCK: &str = r#"
import sys, json, http.server, socketserver
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_GET(self):
        b=json.dumps({"path":self.path,"ok":True}).encode()
        self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
httpd=socketserver.TCPServer(("127.0.0.1",0),H); print(httpd.server_address[1]); sys.stdout.flush(); httpd.serve_forever()
"#;
    let mut child = Command::new("python3").arg("-c").arg(MOCK).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let dir = tmp("http");
    let pdir = dir.join("weather");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(pdir.join("cmx-plugin.json"), format!(
        r#"{{"name":"weather","kind":"http","base_url":"http://127.0.0.1:{port}","method":"GET","path":"/w/{{city}}"}}"#
    )).unwrap();

    let (tools, _) = load_plugins(&dir);
    let roots = ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::ReadOnly, allowed_roots: &roots };
    let r = tools[0].invoke(json!({"city":"BJ"}), &ctx).await.unwrap();
    let _ = child.kill(); let _ = child.wait();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["status"], 200);
    assert_eq!(r.output["body"]["path"], "/w/BJ"); // {city} 替换进了 path
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn plugin_install_then_list_roundtrip() {
    let dir = tmp("inst");
    let tools = build_plugin_tools(&dir); // 空目录 → 仅 list + install
    let install: Arc<dyn Tool> = tools.iter().find(|t| t.spec().name == "plugin_install").unwrap().clone();
    let roots = ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = install.invoke(json!({"manifest":{"name":"echo_p","kind":"command","command":"echo","args":["x"]}}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["installed"], true);
    // 重新加载目录 → 现在有该插件
    let (t2, m2) = load_plugins(&dir);
    assert_eq!(m2.len(), 1);
    assert_eq!(t2[0].spec().name, "echo_p");
    std::fs::remove_dir_all(&dir).ok();
}

/// 探测可用的 wasm 运行时（PATH 上的 wasmer，或 ~/.wasmer/bin/wasmer）。测试环境无则跳过。
fn find_wasmer() -> Option<String> {
    use std::process::{Command, Stdio};
    let mut cands = vec!["wasmer".to_string()];
    if let Ok(home) = std::env::var("HOME") {
        cands.push(format!("{home}/.wasmer/bin/wasmer"));
    }
    cands.into_iter().find(|c| {
        Command::new(c).arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
    })
}

#[tokio::test]
async fn wasm_plugin_invokes_module() {
    let Some(wasmer) = find_wasmer() else {
        eprintln!("跳过 wasm_plugin_invokes_module：本机无 wasmer 运行时");
        return;
    };
    let dir = tmp("wasm");
    let pdir = dir.join("adder");
    std::fs::create_dir_all(&pdir).unwrap();
    // 手写一个导出 add(i32,i32)->i32 的 .wat（wasmer 可直接编译运行，无需 wasm 工具链）。
    std::fs::write(
        pdir.join("add.wat"),
        "(module (func $add (param i32 i32) (result i32) local.get 0 local.get 1 i32.add) (export \"add\" (func $add)))",
    ).unwrap();
    std::fs::write(pdir.join("cmx-plugin.json"), format!(
        r#"{{"name":"adder","kind":"wasm","description":"两数相加","module":"add.wat","invoke":"add","runtime":"{}","args":["{{a}}","{{b}}"]}}"#,
        wasmer.replace('\\', "\\\\")
    )).unwrap();

    let (tools, manifests) = load_plugins(&dir);
    assert_eq!(manifests.len(), 1);
    assert_eq!(tools.len(), 1, "wasm 为同步载体，应产出 1 个工具");
    assert_eq!(tools[0].spec().name, "adder");

    let roots = ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
    let r = tools[0].invoke(json!({"a":2,"b":3}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["kind"], "wasm");
    assert_eq!(r.output["stdout"], "5"); // wasmer run add.wat --invoke add -- 2 3 → 5
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn mcp_manifest_listed_but_no_sync_tool() {
    let dir = tmp("mcp");
    let pdir = dir.join("myserver");
    std::fs::create_dir_all(&pdir).unwrap();
    std::fs::write(
        pdir.join("cmx-plugin.json"),
        r#"{"name":"myserver","kind":"mcp","description":"某 MCP server","command":"my-mcp","args":["--stdio"]}"#,
    ).unwrap();
    // mcp 为异步连接载体：清单被登记（list 可见），但 load_plugins 不产出同步工具。
    let (tools, manifests) = load_plugins(&dir);
    assert_eq!(manifests.len(), 1);
    assert_eq!(manifests[0].kind, "mcp");
    assert_eq!(tools.len(), 0, "mcp 无同步工具（由壳异步连接）");
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn marketplace_list_and_remote_install() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    if Command::new("python3").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| !s.success()).unwrap_or(true) { return; }
    // mock：任意 GET 都回同一份 marketplace 目录（内嵌一份 command 插件清单）。
    const MOCK: &str = r#"
import sys, json, http.server, socketserver
CAT={"name":"demo-market","plugins":[
  {"name":"say_hi","version":"1.0","kind":"command","description":"打招呼",
   "manifest":{"name":"say_hi","kind":"command","command":"echo","args":["hi {who}"]}},
  {"name":"weather","version":"2.0","kind":"http","description":"查天气",
   "manifest":{"name":"weather","kind":"http","base_url":"http://x","method":"GET","path":"/w/{city}"}}]}
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def do_GET(self):
        b=json.dumps(CAT).encode()
        self.send_response(200); self.send_header("Content-Type","application/json"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
httpd=socketserver.TCPServer(("127.0.0.1",0),H); print(httpd.server_address[1]); sys.stdout.flush(); httpd.serve_forever()
"#;
    let mut child = Command::new("python3").arg("-c").arg(MOCK).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    let port: u16 = line.trim().parse().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let url = format!("http://127.0.0.1:{port}/marketplace.json");

    let dir = tmp("market");
    let all = build_plugin_tools(&dir);
    let market: Arc<dyn Tool> = all.iter().find(|t| t.spec().name == "plugin_marketplace").unwrap().clone();
    let install: Arc<dyn Tool> = all.iter().find(|t| t.spec().name == "plugin_install").unwrap().clone();
    let roots = ctx_roots();
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };

    // ① 浏览市场：列出 2 个可安装插件
    let r = market.invoke(json!({ "url": url }), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["count"], 2);
    assert_eq!(r.output["plugins"][0]["name"], "say_hi");
    assert_eq!(r.output["plugins"][0]["installable"], true);

    // ② 从市场按名安装
    let r = install.invoke(json!({ "name": "say_hi", "url": url }), &ctx).await.unwrap();
    let _ = child.kill(); let _ = child.wait();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["installed"], true);
    assert_eq!(r.output["kind"], "command");

    // ③ 重载 → 装好的插件成为可用工具
    let (t2, m2) = load_plugins(&dir);
    assert_eq!(m2.len(), 1);
    assert_eq!(t2[0].spec().name, "say_hi");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn install_scan_uninstall_roundtrip() {
    // 前门 install_plugin/list_plugins/uninstall_plugin 共用的底座：install_manifest + scan + uninstall。
    let dir = tmp("iu");
    let mf = serde_json::json!({
        "name":"demo-http","kind":"http","version":"1.0","description":"演示",
        "homepage":"https://example.com/","base_url":"http://x","method":"GET","path":"/p"
    });
    // 装前：空
    assert_eq!(scan_plugin_summaries(&dir).len(), 0);
    // 装
    let r = install_manifest(&dir, &mf).expect("install ok");
    assert_eq!(r["installed"], true);
    assert_eq!(r["name"], "demo-http");
    // 实时扫描立即可见（无需重启），且带 homepage
    let sums = scan_plugin_summaries(&dir);
    assert_eq!(sums.len(), 1);
    assert_eq!(sums[0]["name"], "demo-http");
    assert_eq!(sums[0]["homepage"], "https://example.com/");
    // 卸载
    let u = uninstall_plugin(&dir, "demo-http").expect("uninstall ok");
    assert_eq!(u["uninstalled"], true);
    assert_eq!(scan_plugin_summaries(&dir).len(), 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn install_rejects_bad_name_and_unknown_kind() {
    let dir = tmp("bad");
    // 名称净化后为空（全是非法字符）→ 拒绝
    assert!(install_manifest(&dir, &serde_json::json!({"name":"../","kind":"http"})).is_err());
    // 带路径穿越的名字净化成安全目录名（"../../etc"→"etc"），落在 plugins 目录内、不逃逸
    let r = install_manifest(&dir, &serde_json::json!({"name":"../../etc","kind":"http"}));
    if let Ok(v) = &r {
        let path = v["path"].as_str().unwrap_or("");
        assert!(path.starts_with(&dir.display().to_string()), "写入路径应在 plugins 目录内: {path}");
    }
    // 未知 kind → 拒绝
    assert!(install_manifest(&dir, &serde_json::json!({"name":"x","kind":"quux"})).is_err());
    // 卸载不存在的插件 → 幂等成功
    assert!(uninstall_plugin(&dir, "nope").is_ok());
    std::fs::remove_dir_all(&dir).ok();
}
