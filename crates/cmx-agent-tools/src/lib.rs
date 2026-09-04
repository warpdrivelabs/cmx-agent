//! cmx-agent 内置工具（工具平面示例）。这些是"双手"的最小集，用于验证内核与守卫管道；
//! 真正的企业能力（cmx-flow/rules/ontology/report/data-auth）与 MCP 外接在后续里程碑接入，
//! 但都实现同一个 [`cmx_agent_core::Tool`] trait，挂进同一个 [`cmx_agent_core::ToolRegistry`]。

mod add;
mod clock;
mod danger_rm;
mod echo;
mod fs_read;

pub use add::AddTool;
pub use clock::ClockTool;
pub use danger_rm::DangerRmTool;
pub use echo::EchoTool;
pub use fs_read::FsReadTool;

use cmx_agent_core::ToolRegistry;
use std::sync::Arc;

/// 便捷装配：把全部内置工具注册进一个新的注册表。
pub fn default_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool))
        .register(Arc::new(ClockTool::system()))
        .register(Arc::new(AddTool))
        .register(Arc::new(FsReadTool))
        .register(Arc::new(DangerRmTool));
    reg
}
