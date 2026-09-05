//! cmx-agent MCP 客户端（U3）：作为 MCP host 连接外部 MCP server，把其工具归一进 ToolRegistry。
//!
//! MCP（Model Context Protocol）是 Codex/Claude Code 等通用的工具生态标准。本 crate 实现 **stdio 传输**
//! 的 MCP 客户端（换行分隔 JSON-RPC 2.0）：spawn server 子进程 → initialize 握手 → tools/list → 把每个
//! 工具包装成 `cmx_agent_core::Tool` 代理（invoke 转发 tools/call）。网络/进程依赖隔离在本 crate。

pub mod client;
pub mod tool;

pub use client::{McpClient, McpError, McpToolInfo, extract_text, is_error};
pub use tool::{McpServerConfig, McpTool, connect_server, load_and_connect};
