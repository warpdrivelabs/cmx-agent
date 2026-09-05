//! LSP stdio 客户端：spawn 语言服务器，用 **Content-Length 帧的 JSON-RPC 2.0**（LSP 传输约定，区别于
//! MCP 的换行分隔）握手 + 查询。支持 hover / definition / references / documentSymbol / diagnostics。
//!
//! 诊断是 server **推送**（`textDocument/publishDiagnostics` 通知），故读响应时顺带收集按 uri 缓存。

use std::collections::{HashMap, HashSet};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};

#[derive(Debug, thiserror::Error)]
pub enum LspError {
    #[error("启动语言服务器失败: {0}")]
    Spawn(String),
    #[error("IO: {0}")]
    Io(String),
    #[error("JSON-RPC 错误 code={code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("协议错误: {0}")]
    Protocol(String),
    #[error("server 已退出（EOF）")]
    Eof,
}

/// 一个语言服务器的 stdio 连接。
pub struct LspClient {
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    next_id: i64,
    opened: HashSet<String>,
    diagnostics: HashMap<String, Vec<Value>>,
}

impl LspClient {
    /// spawn 语言服务器并完成 `initialize` 握手。`root_uri` = 工作区根（file://…）。
    pub async fn connect(
        command: &str,
        args: &[String],
        root_uri: &str,
    ) -> Result<Self, LspError> {
        let mut child = tokio::process::Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| LspError::Spawn(e.to_string()))?;
        let stdin = child.stdin.take().ok_or(LspError::Protocol("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or(LspError::Protocol("no stdout".into()))?;
        let mut c = Self {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 0,
            opened: HashSet::new(),
            diagnostics: HashMap::new(),
        };
        c.initialize(root_uri).await?;
        Ok(c)
    }

    async fn initialize(&mut self, root_uri: &str) -> Result<(), LspError> {
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "hover": {"contentFormat": ["markdown","plaintext"]},
                    "definition": {}, "references": {}, "documentSymbol": {},
                    "publishDiagnostics": {}
                }
            },
            "clientInfo": {"name":"cmx-agent","version":"0.1.0"}
        });
        let _ = self.request("initialize", params).await?;
        self.notify("initialized", json!({})).await?;
        Ok(())
    }

    /// 确保文档已 didOpen（首次打开发送内容）。
    pub async fn ensure_open(&mut self, uri: &str, language_id: &str, text: &str) -> Result<(), LspError> {
        if self.opened.contains(uri) {
            return Ok(());
        }
        self.notify(
            "textDocument/didOpen",
            json!({"textDocument":{"uri":uri,"languageId":language_id,"version":1,"text":text}}),
        )
        .await?;
        self.opened.insert(uri.to_string());
        Ok(())
    }

    pub async fn hover(&mut self, uri: &str, line: u32, character: u32) -> Result<Value, LspError> {
        self.request(
            "textDocument/hover",
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character}}),
        )
        .await
    }
    pub async fn definition(&mut self, uri: &str, line: u32, character: u32) -> Result<Value, LspError> {
        self.request(
            "textDocument/definition",
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character}}),
        )
        .await
    }
    pub async fn references(&mut self, uri: &str, line: u32, character: u32) -> Result<Value, LspError> {
        self.request(
            "textDocument/references",
            json!({"textDocument":{"uri":uri},"position":{"line":line,"character":character},
                   "context":{"includeDeclaration":true}}),
        )
        .await
    }
    pub async fn document_symbol(&mut self, uri: &str) -> Result<Value, LspError> {
        self.request("textDocument/documentSymbol", json!({"textDocument":{"uri":uri}}))
            .await
    }

    /// 收集某 uri 的诊断：didOpen 后 server 推送需要时间；此处在 `wait` 窗口内抽读通知。
    pub async fn collect_diagnostics(&mut self, uri: &str, wait: Duration) -> Vec<Value> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(d) = self.diagnostics.get(uri) {
                if !d.is_empty() {
                    return d.clone();
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match tokio::time::timeout(remaining, self.read_message()).await {
                Ok(Ok(msg)) => {
                    self.handle_incoming(&msg);
                }
                _ => break,
            }
        }
        self.diagnostics.get(uri).cloned().unwrap_or_default()
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, LspError> {
        self.next_id += 1;
        let id = self.next_id;
        self.write_msg(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        loop {
            let msg = self.read_message().await?;
            // server→client 请求（如 workspace/configuration）：回一个空响应以免卡住
            if msg.get("method").is_some() && msg.get("id").is_some() {
                let sid = msg.get("id").cloned().unwrap();
                self.write_msg(&json!({"jsonrpc":"2.0","id":sid,"result":null})).await.ok();
                continue;
            }
            if msg.get("id").and_then(|v| v.as_i64()) == Some(id) {
                if let Some(err) = msg.get("error") {
                    return Err(LspError::Rpc {
                        code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
                        message: err.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string(),
                    });
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            self.handle_incoming(&msg);
        }
    }

    fn handle_incoming(&mut self, msg: &Value) {
        if msg.get("method").and_then(|m| m.as_str()) == Some("textDocument/publishDiagnostics") {
            if let Some(p) = msg.get("params") {
                if let Some(uri) = p.get("uri").and_then(|u| u.as_str()) {
                    let diags = p.get("diagnostics").and_then(|d| d.as_array()).cloned().unwrap_or_default();
                    self.diagnostics.insert(uri.to_string(), diags);
                }
            }
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), LspError> {
        self.write_msg(&json!({"jsonrpc":"2.0","method":method,"params":params})).await
    }

    /// 写一条 Content-Length 帧消息。
    async fn write_msg(&mut self, v: &Value) -> Result<(), LspError> {
        let body = serde_json::to_string(v).map_err(|e| LspError::Io(e.to_string()))?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);
        self.stdin.write_all(frame.as_bytes()).await.map_err(|e| LspError::Io(e.to_string()))?;
        self.stdin.flush().await.map_err(|e| LspError::Io(e.to_string()))?;
        Ok(())
    }

    /// 读一条 Content-Length 帧消息（先读头到空行取长度，再读 body N 字节）。
    async fn read_message(&mut self) -> Result<Value, LspError> {
        let mut content_len: Option<usize> = None;
        loop {
            let mut line = String::new();
            let n = self.reader.read_line(&mut line).await.map_err(|e| LspError::Io(e.to_string()))?;
            if n == 0 {
                return Err(LspError::Eof);
            }
            let t = line.trim_end();
            if t.is_empty() {
                break; // 头结束
            }
            if let Some(v) = t.strip_prefix("Content-Length:") {
                content_len = v.trim().parse().ok();
            }
        }
        let len = content_len.ok_or(LspError::Protocol("缺少 Content-Length".into()))?;
        let mut buf = vec![0u8; len];
        self.reader.read_exact(&mut buf).await.map_err(|e| LspError::Io(e.to_string()))?;
        serde_json::from_slice(&buf).map_err(|e| LspError::Protocol(e.to_string()))
    }

    pub async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
    }
}

#[cfg(test)]
mod tests {
    // 纯逻辑无可测；协议往返见 tests/lsp_integration.rs（mock server）。
}
