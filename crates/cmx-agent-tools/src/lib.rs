//! cmx-agent 内置工具（工具平面）。这些是"双手"的集合，实现同一个 [`cmx_agent_core::Tool`] trait，
//! 挂进同一个 [`cmx_agent_core::ToolRegistry`]。
//!
//! - 基础示例：echo / clock / add / danger_rm（验证内核与守卫管道）。
//! - **编码面（E1）**：fs_read / fs_write / fs_edit / apply_patch / shell / grep / glob
//!   （shell 2026-09-08 由 bash 改名，Windows 探测链见 `proc.rs`）。
//! - **编码面进阶（E2）**：git / run_tests / repo_map / update_plan——仓库理解 + 版本控制 + 测试 + 计划。
//! - 企业能力（cmx-flow/rules/ontology/report）经连接器与 MCP 在后续里程碑接入。

mod add;
mod apply_patch;
mod chart;
mod clock;
mod danger_rm;
mod data_describe;
mod echo;
mod fs_edit;
mod fs_read;
mod fs_write;
mod git;
mod glob_tool;
#[cfg(windows)]
mod job;
mod grep;
mod plan;
mod proc;
mod repo_map;
mod run_tests;
mod sandbox;
mod shell;
mod task;
#[cfg(test)]
mod testutil;

pub use add::AddTool;
pub use apply_patch::ApplyPatchTool;
pub use chart::ChartTool;
pub use clock::ClockTool;
pub use danger_rm::DangerRmTool;
pub use data_describe::DataDescribeTool;
pub use echo::EchoTool;
pub use fs_edit::FsEditTool;
pub use fs_read::FsReadTool;
pub use fs_write::FsWriteTool;
pub use git::GitTool;
pub use glob_tool::GlobTool;
pub use grep::GrepTool;
pub use plan::UpdatePlanTool;
pub use repo_map::RepoMapTool;
pub use run_tests::RunTestsTool;
pub use shell::ShellTool;
// 子智能体（U1）：不进 default_registry（需构建后注入 agent 句柄），由 DesktopAppBuilder 装配。
pub use task::{SubagentHandle, TaskTool};

use cmx_agent_core::ToolRegistry;
use std::sync::Arc;

/// 便捷装配：把全部内置工具注册进一个新的注册表。
pub fn default_registry() -> ToolRegistry {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(EchoTool))
        .register(Arc::new(ClockTool::system()))
        .register(Arc::new(AddTool))
        .register(Arc::new(DangerRmTool))
        // 编码面（E1）
        .register(Arc::new(FsReadTool))
        .register(Arc::new(FsWriteTool))
        .register(Arc::new(FsEditTool))
        .register(Arc::new(ApplyPatchTool))
        .register(Arc::new(ShellTool))
        .register(Arc::new(GrepTool))
        .register(Arc::new(GlobTool))
        // 编码面进阶（E2）
        .register(Arc::new(GitTool))
        .register(Arc::new(RunTestsTool))
        .register(Arc::new(RepoMapTool))
        .register(Arc::new(UpdatePlanTool))
        // 办公面（P3/U7）：数据分析 + 图表可视化
        .register(Arc::new(DataDescribeTool))
        .register(Arc::new(ChartTool));
    reg
}

