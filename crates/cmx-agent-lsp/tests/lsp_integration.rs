//! LSP 客户端集成测试：spawn 一个 python3 写的 **mock LSP server**（Content-Length 帧 JSON-RPC），
//! 走 initialize → didOpen → hover / documentSymbol / diagnostics(推送)。python3 缺失则跳过。

use std::collections::HashMap;

use cmx_agent_core::guard::SandboxMode;
use cmx_agent_core::{Tool, ToolCtx};
use cmx_agent_lsp::{LspServerConfig, LspTool};
use serde_json::json;

// mock LSP：Content-Length 帧；initialize 回能力；didOpen 后推送一条诊断；hover/documentSymbol 固定返回。
const MOCK_LSP: &str = r#"
import sys, json
inp = sys.stdin.buffer
out = sys.stdout.buffer
def read_msg():
    headers = {}
    while True:
        line = inp.readline()
        if not line: return None
        line = line.decode('utf-8').strip()
        if line == '': break
        if ':' in line:
            k,v = line.split(':',1); headers[k.strip().lower()] = v.strip()
    n = int(headers.get('content-length','0'))
    body = inp.read(n)
    return json.loads(body.decode('utf-8'))
def send(o):
    b = json.dumps(o).encode('utf-8')
    out.write(('Content-Length: %d\r\n\r\n' % len(b)).encode('utf-8')); out.write(b); out.flush()
while True:
    m = read_msg()
    if m is None: break
    mid = m.get('id'); method = m.get('method')
    if method == 'initialize':
        send({'jsonrpc':'2.0','id':mid,'result':{'capabilities':{'hoverProvider':True,'documentSymbolProvider':True}}})
    elif method == 'initialized':
        pass
    elif method == 'textDocument/didOpen':
        uri = m['params']['textDocument']['uri']
        send({'jsonrpc':'2.0','method':'textDocument/publishDiagnostics','params':{'uri':uri,'diagnostics':[
            {'range':{'start':{'line':2,'character':0},'end':{'line':2,'character':5}},'severity':2,'message':'未使用的变量 x'}]}})
    elif method == 'textDocument/hover':
        send({'jsonrpc':'2.0','id':mid,'result':{'contents':{'kind':'markdown','value':'fn main() -> ()\n\n程序入口'}}})
    elif method == 'textDocument/documentSymbol':
        send({'jsonrpc':'2.0','id':mid,'result':[{'name':'main','kind':12,'range':{'start':{'line':0,'character':0},'end':{'line':3,'character':0}}}]})
    else:
        if mid is not None: send({'jsonrpc':'2.0','id':mid,'result':None})
"#;

fn python3_ok() -> bool {
    std::process::Command::new("python3").arg("--version")
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

fn setup_tool() -> (std::path::PathBuf, LspTool) {
    let root = std::env::temp_dir().join(format!("cmx-lsp-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("main.rs"), "fn main() {\n    let x = 1;\n    println!(\"hi\");\n}\n").unwrap();
    let mut servers = HashMap::new();
    servers.insert(".rs".to_string(), LspServerConfig {
        command: "python3".into(),
        args: vec!["-c".into(), MOCK_LSP.into()],
        language_id: Some("rust".into()),
    });
    (root, LspTool::new(servers))
}

#[tokio::test]
async fn lsp_hover_and_symbols_and_diagnostics() {
    if !python3_ok() { eprintln!("python3 不可用，跳过 LSP 集成测试"); return; }
    let (root, tool) = setup_tool();
    let roots = vec![root.clone()];
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };

    // hover
    let r = tool.invoke(json!({"operation":"hover","path":"main.rs","line":0,"character":3}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert!(r.output["hover"].as_str().unwrap().contains("程序入口"), "{r:?}");

    // document_symbol
    let r = tool.invoke(json!({"operation":"document_symbol","path":"main.rs"}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    let syms = r.output["symbols"].as_array().unwrap();
    assert!(syms.iter().any(|s| s.as_str().unwrap().contains("main")), "{syms:?}");

    // diagnostics（server 在 didOpen 后推送）
    let r = tool.invoke(json!({"operation":"diagnostics","path":"main.rs"}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    let d = r.output["diagnostics"].as_array().unwrap();
    assert!(d.iter().any(|x| x.as_str().unwrap().contains("未使用的变量")), "{d:?}");

    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn unconfigured_degrades_gracefully() {
    let tool = LspTool::new(HashMap::new());
    let roots: Vec<std::path::PathBuf> = vec![std::env::temp_dir()];
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
    let r = tool.invoke(json!({"operation":"hover","path":"x.rs"}), &ctx).await.unwrap();
    assert!(!r.ok);
    assert!(r.output["error"].as_str().unwrap().contains("未配置语言服务器"));
}

#[tokio::test]
async fn unknown_extension_reports() {
    if !python3_ok() { return; }
    let (root, tool) = setup_tool();
    std::fs::write(root.join("a.xyz"), "??").unwrap();
    let roots = vec![root.clone()];
    let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
    let _ = tool; let _t = LspTool::new({ let mut m=HashMap::new(); m.insert(".rs".to_string(), LspServerConfig{command:"python3".into(),args:vec!["-c".into(),MOCK_LSP.into()],language_id:None}); m });
    let r = _t.invoke(json!({"operation":"hover","path":"a.xyz","line":0,"character":0}), &ctx).await.unwrap();
    assert!(!r.ok, "无对应 server 应降级报错");
    std::fs::remove_dir_all(&root).ok();
}
