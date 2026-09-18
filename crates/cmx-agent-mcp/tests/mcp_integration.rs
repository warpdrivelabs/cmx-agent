//! MCP 客户端集成测试：spawn 一个用 python3 写的 **mock MCP server**（换行 JSON-RPC），
//! 走完 initialize → tools/list → tools/call，验证代理工具能真实转发。python3 缺失则跳过。

use std::sync::Arc;

use cmx_agent_core::{Tool, ToolCtx};
use cmx_agent_mcp::{McpServerConfig, connect_server};
use serde_json::json;

const MOCK_SERVER: &str = r#"
# Windows 下 Python 子进程 stdio 默认跟随系统 ANSI 代码页（cp936/GBK），会把客户端写入的
# UTF-8 JSON-RPC 误解码（"你好"→"浣犲ソ"）。MCP stdio 约定是 UTF-8——强制两端管道编码。
import sys, json
sys.stdin.reconfigure(encoding="utf-8")
sys.stdout.reconfigure(encoding="utf-8")
def send(o): sys.stdout.write(json.dumps(o)+"\n"); sys.stdout.flush()
for line in sys.stdin:
    line=line.strip()
    if not line: continue
    msg=json.loads(line)
    mid=msg.get("id"); method=msg.get("method")
    if method=="initialize":
        send({"jsonrpc":"2.0","id":mid,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"mock","version":"1"}}})
    elif method=="notifications/initialized":
        pass
    elif method=="tools/list":
        send({"jsonrpc":"2.0","id":mid,"result":{"tools":[
            {"name":"echo","description":"回声","inputSchema":{"type":"object","properties":{"text":{"type":"string"}}}},
            {"name":"boom","description":"总是报错","inputSchema":{"type":"object"}}]}})
    elif method=="tools/call":
        p=msg.get("params",{}); name=p.get("name"); args=p.get("arguments",{})
        if name=="echo":
            send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"echo: "+str(args.get("text",""))}],"isError":False}})
        elif name=="boom":
            send({"jsonrpc":"2.0","id":mid,"result":{"content":[{"type":"text","text":"炸了"}],"isError":True}})
        else:
            send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"unknown tool"}})
    else:
        send({"jsonrpc":"2.0","id":mid,"error":{"code":-32601,"message":"unknown method"}})
"#;

fn python3_available() -> bool {
    std::process::Command::new("python3")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn mcp_end_to_end_via_mock_server() {
    if !python3_available() {
        eprintln!("python3 不可用，跳过 MCP 集成测试");
        return;
    }
    let cfg = McpServerConfig {
        label: "mock".into(),
        command: "python3".into(),
        args: vec!["-c".into(), MOCK_SERVER.into()],
        // 双保险：reconfigure 之外的兜底（覆盖 stdin 解码与 locale 相关默认）
        env: vec![("PYTHONIOENCODING".into(), "utf-8".into())],
    };
    let tools = connect_server(&cfg).await.expect("连接 mock MCP server");
    // 两个工具，命名空间前缀 mcp_mock_*
    assert_eq!(tools.len(), 2);
    let names: Vec<String> = tools.iter().map(|t| t.spec().name).collect();
    assert!(names.contains(&"mcp_mock_echo".to_string()), "{names:?}");
    assert!(names.contains(&"mcp_mock_boom".to_string()));

    let roots: Vec<std::path::PathBuf> = vec![];
    let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };

    // 调 echo → 转发 tools/call → 返回文本
    let echo: &Arc<dyn Tool> = tools.iter().find(|t| t.spec().name == "mcp_mock_echo").unwrap();
    let r = echo.invoke(json!({"text":"你好 MCP"}), &ctx).await.unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["text"], "echo: 你好 MCP");

    // 调 boom → isError → ToolResult err
    let boom: &Arc<dyn Tool> = tools.iter().find(|t| t.spec().name == "mcp_mock_boom").unwrap();
    let r = boom.invoke(json!({}), &ctx).await.unwrap();
    assert!(!r.ok, "boom 应为错误");
}

#[tokio::test]
async fn connect_failure_is_error() {
    // 不存在的命令 → 连接失败（调用方可跳过该 server）
    let cfg = McpServerConfig {
        label: "nope".into(),
        command: "definitely-not-a-real-binary-xyz".into(),
        args: vec![],
        env: vec![],
    };
    assert!(connect_server(&cfg).await.is_err());
}
