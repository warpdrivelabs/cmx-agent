//! cmx-agent 应用层（M1）：桌面壳后端。
//!
//! 「同核多壳」：本 crate 是桌面壳（Tauri）、CLI、后续 Headless HTTP 共用的**后端核**——
//! - [`AgentApp`]：用例级 façade（新建/发消息/列/读/删会话），每回合增量落库。
//! - [`SessionStore`] / [`FileSessionStore`]：会话事件 JSONL 落库（可回放）。
//! - [`protocol`]：JSON 前门命令协议 = Tauri `invoke` 边界 = Headless 请求体。
//! - [`DesktopAppBuilder`]：一键装配（沙箱根=工作区 + 内置工具 + 五层守卫）。
//!
//! Tauri WebView 外壳（`shell/`）在联网装好 tauri 后启用；它只是把 `invoke` 转成 [`protocol::dispatch`]，
//! 不含任何业务逻辑——故本 crate 全离线可测，桌面壳只是"最后一层薄壳"。

pub mod app;
pub mod builder;
pub mod demo_model;
pub mod error;
pub mod protocol;
pub mod store;

pub use app::{AgentApp, SendOutcome};
pub use builder::DesktopAppBuilder;
pub use demo_model::DemoModel;
pub use error::{AppError, AppResult};
pub use protocol::{AppRequest, AppResponse, dispatch, dispatch_json};
pub use store::{FileSessionStore, SessionMeta, SessionStore};

// 连接器类型 re-export，便于前门壳（web/tauri）无需直接依赖 cmx-agent-connectors 即可配置。
pub use cmx_agent_connectors::{ConnectorCard, ConnectorConfig, ConnectorRegistry};
