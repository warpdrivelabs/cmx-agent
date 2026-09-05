//! cmx-agent LSP 客户端（U2）：stdio LSP 代码智能（定义/引用/悬停/符号/诊断），有则用无则降级。
//!
//! LSP 传输用 **Content-Length 帧的 JSON-RPC 2.0**（区别于 MCP 的换行分隔）。语言服务器较重，按扩展名
//! 缓存复用连接。配置来自 `<data_dir>/lsp.json`。进程/协议依赖隔离在本 crate。

pub mod client;
pub mod tool;

pub use client::{LspClient, LspError};
pub use tool::{LspServerConfig, LspTool};
