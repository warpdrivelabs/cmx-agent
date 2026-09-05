//! cmx-agent 连接器（M2）：把运行中的 cmx 微服务经 HTTP 暴露为智能体工具，并提供 live 健康探测。
//!
//! 方案图 5「工具平面 · ① cmx 引擎工具」的落地——**这是复刻超越 WorkBuddy 的护城河**：通用 WorkBuddy
//! 只有文件/浏览器/办公软件，我们把整套 ERP 引擎（flow/onto/report）变成智能体的双手。
//!
//! 与内核解耦：连接器工具实现 `cmx_agent_core::Tool`，挂进同一个 `ToolRegistry`，内核零改动。
//! 网络依赖（reqwest）隔离在本 crate，`cmx-agent-core`/`tools` 仍零网络、纯离线可测。

pub mod auth;
pub mod client;
pub mod connectors;
pub mod health;
pub mod registry;

pub use auth::{AuthConfig, AuthProvider, LoggedInUser};
pub use client::{ClientError, CmxServiceClient};
pub use connectors::{FlowConnector, OntoConnector, ReportConnector};
pub use health::{ConnectorStatus, probe};
pub use registry::{ConnectorCard, ConnectorConfig, ConnectorDescriptor, ConnectorRegistry};
