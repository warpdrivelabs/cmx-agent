//! web_fetch / web_search 集成测试：spawn 一个 python3 mock HTTP server（返回 canned HTML），
//! 验证 web_fetch 抽正文/标题、web_search 解析结果。python3 缺失则跳过。
//! 用 `CMX_AGENT_NET_ALLOW_PRIVATE` 放开 localhost（默认 SSRF 会拦 127.0.0.1）。

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

use cmx_agent_core::{Tool, ToolCtx};
use cmx_agent_net::{WebFetchTool, WebSearchTool};
use serde_json::json;

const MOCK: &str = r#"
import sys, http.server, socketserver
PAGE = "<html><head><title>测试标题</title><style>b{}</style></head><body><script>bad()</script><h1>大标题</h1><p>正文段落一 &amp; 更多</p><div>正文段落二</div></body></html>"
SEARCH = '<div><a class="result-link" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fone&rut=x">第一条结果</a><td class="result-snippet">第一条摘要文本</td><a class="result-link" href="https://two.example.org/">第二条结果</a><td class="result-snippet">第二条摘要</td></div>'
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self,*a): pass
    def _send(self, body):
        b=body.encode("utf-8"); self.send_response(200)
        self.send_header("Content-Type","text/html; charset=utf-8"); self.send_header("Content-Length",str(len(b))); self.end_headers(); self.wfile.write(b)
    def do_GET(self):
        if self.path.startswith("/page"): self._send(PAGE)
        elif self.path.startswith("/s"): self._send(SEARCH)
        else: self.send_response(404); self.end_headers()
httpd=socketserver.TCPServer(("127.0.0.1",0),H)
print(httpd.server_address[1]); sys.stdout.flush()
httpd.serve_forever()
"#;

fn python3() -> bool {
    Command::new("python3")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_mock() -> (Server, u16) {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(MOCK)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn mock http server");
    let mut r = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    r.read_line(&mut line).expect("read port");
    let port: u16 = line.trim().parse().expect("parse port");
    (Server(child), port)
}

#[tokio::test]
async fn web_fetch_and_search_via_mock() {
    if !python3() {
        eprintln!("python3 不可用，跳过 net 集成测试");
        return;
    }
    let (_srv, port) = spawn_mock();
    std::thread::sleep(std::time::Duration::from_millis(250)); // 等 serve_forever 就绪

    let roots = vec![std::path::PathBuf::from("/tmp")];
    let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };

    // ── web_fetch：抽正文 + 标题，剔除 script/head（allow_private 放开 localhost）──
    let r = WebFetchTool { allow_private: true }
        .invoke(json!({"url": format!("http://127.0.0.1:{port}/page")}), &ctx)
        .await
        .unwrap();
    assert!(r.ok, "{r:?}");
    assert_eq!(r.output["title"], "测试标题");
    let text = r.output["text"].as_str().unwrap();
    assert!(text.contains("正文段落一 & 更多"), "正文缺失：{text}");
    assert!(text.contains("正文段落二"));
    assert!(!text.contains("bad("), "脚本应被剔除：{text}");

    // ── web_search：指向 mock 的 /s?q= 端点，解析结果 ──
    let s = WebSearchTool {
        endpoint: format!("http://127.0.0.1:{port}/s?q={{q}}"),
    }
    .invoke(json!({"query":"测试"}), &ctx)
    .await
    .unwrap();
    assert!(s.ok, "{s:?}");
    let results = s.output["results"].as_array().unwrap();
    assert_eq!(results.len(), 2, "{s:?}");
    assert_eq!(results[0]["title"], "第一条结果");
    assert_eq!(results[0]["url"], "https://example.com/one"); // uddg 重定向已解出
    assert_eq!(results[0]["snippet"], "第一条摘要文本");
    assert_eq!(results[1]["url"], "https://two.example.org/");
}
