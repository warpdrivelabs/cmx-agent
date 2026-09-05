//! MCP stdio 客户端：spawn 一个 MCP server 子进程，用**换行分隔的 JSON-RPC 2.0**（MCP stdio 传输约定，
//! 区别于 LSP 的 Content-Length 帧）握手 + 列工具 + 调工具。
//!
//! 请求/响应按 `id` 关联：写一条请求后逐行读，跳过通知/无关消息，直到读到匹配 id 的响应。
//! 客户端持有子进程与管道，置于 `tokio::sync::Mutex` 内串行访问（一个 server 一条 stdio 管道）。

use std::process::Stdio;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};

/// MCP 客户端错误。
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("启动 MCP server 失败: {0}")]
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

/// 一个 MCP 工具的描述（来自 `tools/list`）。
#[derive(Debug, Clone)]
pub struct McpToolInfo {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// 一个 MCP server 的 stdio 连接。
pub struct McpClient {
    label: String,
    child: Child,
    stdin: ChildStdin,
    reader: BufReader<tokio::process::ChildStdout>,
    next_id: i64,
}

impl McpClient {
    /// spawn `command args...`，完成 `initialize` 握手。`label` 用于工具命名空间前缀。
    pub async fn connect(
        label: impl Into<String>,
        command: &str,
        args: &[String],
        env: &[(String, String)],
    ) -> Result<Self, McpError> {
        let mut cmd = tokio::process::Command::new(command);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| McpError::Spawn(e.to_string()))?;
        let stdin = child.stdin.take().ok_or(McpError::Protocol("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or(McpError::Protocol("no stdout".into()))?;
        let reader = BufReader::new(stdout);
        let mut c = Self {
            label: label.into(),
            child,
            stdin,
            reader,
            next_id: 0,
        };
        c.initialize().await?;
        Ok(c)
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    async fn initialize(&mut self) -> Result<(), McpError> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "cmx-agent", "version": "0.1.0" }
        });
        let _ = self.request("initialize", params).await?;
        // 通知 server 初始化完成（无需响应）。
        self.notify("notifications/initialized", json!({})).await?;
        Ok(())
    }

    /// 列出 server 暴露的工具。
    pub async fn list_tools(&mut self) -> Result<Vec<McpToolInfo>, McpError> {
        let res = self.request("tools/list", json!({})).await?;
        let tools = res
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        Ok(tools
            .into_iter()
            .filter_map(|t| {
                let name = t.get("name").and_then(|v| v.as_str())?.to_string();
                Some(McpToolInfo {
                    name,
                    description: t
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    input_schema: t
                        .get("inputSchema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"})),
                })
            })
            .collect())
    }

    /// 调用一个工具，返回 `tools/call` 的 result（含 content / isError）。
    pub async fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, McpError> {
        self.request("tools/call", json!({ "name": name, "arguments": arguments }))
            .await
    }

    /// 发一条 JSON-RPC 请求并等待匹配 id 的响应。
    async fn request(&mut self, method: &str, params: Value) -> Result<Value, McpError> {
        self.next_id += 1;
        let id = self.next_id;
        let req = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        self.write_msg(&req).await?;
        // 读到匹配 id 的响应（跳过通知与其它 id）。
        loop {
            let msg = self.read_msg().await?;
            // 通知（无 id）→ 跳过
            let Some(mid) = msg.get("id") else { continue };
            if mid.as_i64() != Some(id) {
                continue;
            }
            if let Some(err) = msg.get("error") {
                return Err(McpError::Rpc {
                    code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(-1),
                    message: err
                        .get("message")
                        .and_then(|m| m.as_str())
                        .unwrap_or("")
                        .to_string(),
                });
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    async fn notify(&mut self, method: &str, params: Value) -> Result<(), McpError> {
        let n = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.write_msg(&n).await
    }

    async fn write_msg(&mut self, v: &Value) -> Result<(), McpError> {
        let mut line = serde_json::to_string(v).map_err(|e| McpError::Io(e.to_string()))?;
        line.push('\n');
        self.stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|e| McpError::Io(e.to_string()))?;
        self.stdin.flush().await.map_err(|e| McpError::Io(e.to_string()))?;
        Ok(())
    }

    async fn read_msg(&mut self) -> Result<Value, McpError> {
        loop {
            let mut buf = String::new();
            let n = self
                .reader
                .read_line(&mut buf)
                .await
                .map_err(|e| McpError::Io(e.to_string()))?;
            if n == 0 {
                return Err(McpError::Eof);
            }
            let t = buf.trim();
            if t.is_empty() {
                continue;
            }
            return serde_json::from_str(t).map_err(|e| McpError::Protocol(e.to_string()));
        }
    }

    /// 关闭：杀子进程。
    pub async fn shutdown(&mut self) {
        let _ = self.child.start_kill();
    }
}

/// 把 `tools/call` 的 result 里的 content 抽成纯文本（拼接 text 部分）。
pub fn extract_text(result: &Value) -> String {
    result
        .get("content")
        .and_then(|c| c.as_array())
        .map(|parts| {
            parts
                .iter()
                .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join("\n")
        })
        .unwrap_or_default()
}

/// `tools/call` result 是否标记为错误。
pub fn is_error(result: &Value) -> bool {
    result.get("isError").and_then(|b| b.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_joins_parts() {
        let r = json!({"content":[{"type":"text","text":"你好"},{"type":"text","text":"世界"}]});
        assert_eq!(extract_text(&r), "你好\n世界");
        assert!(!is_error(&r));
    }

    #[test]
    fn is_error_flag() {
        assert!(is_error(&json!({"content":[],"isError":true})));
        assert!(!is_error(&json!({"content":[]})));
    }
}
