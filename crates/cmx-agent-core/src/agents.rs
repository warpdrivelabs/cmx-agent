//! 子智能体类型注册表数据类型（方案 20260914 §6.1，阶段一）。纯数据 + 解析缝：
//! `cmx-agent-tools`（task 工具过滤与动态枚举）与 `cmx-agent-app`（agents.json 读写、
//! 模型解析）共用本模块——tools 不得依赖 app，模型解析经 [`ModelResolver`] 回调缝反转。
//!
//! 热生效：app 层持 `Arc<RwLock<Vec<AgentSpec>>>`（合并后的生效清单）并给 [`crate::tool::ToolRegistry`]
//! 里的 task 工具共享一份；内核每步 `tools.specs()` 现调 `t.spec()`，故 `TaskTool::spec()`
//! 动态拼 `subagent_type` 枚举——agents.json 变更改共享清单即天然生效，零额外同步路径。

use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::model::ModelSeam;

/// 内置类型名：通用子智能体（全部工具，现 task 行为类型化）。
pub const GENERAL_PURPOSE: &str = "general-purpose";
/// 内置类型名：只读探索（8 件只读工具白名单）。
pub const EXPLORE: &str = "explore";

/// 工具集选择。语义对父回合可见集 `visible` 求值（§6.1 定稿公式）：
/// `All → visible`（含后装的插件/MCP 动态工具）；`Allow → visible ∩ names`（冻结清单，
/// 后装插件不在名单即不可用）；`Deny → visible \ names`。未知名一律忽略。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ToolSelection {
    /// 全部可见工具。
    #[default]
    All,
    /// 白名单（冻结清单）。
    Allow { names: Vec<String> },
    /// 黑名单（其余全放行）。
    Deny { names: Vec<String> },
}

impl ToolSelection {
    /// 对可见集求值（§6.1 公式）。入参为父回合可见工具名集合。
    pub fn effective<I>(&self, visible: I) -> Vec<String>
    where
        I: IntoIterator<Item = String>,
    {
        let visible: Vec<String> = visible.into_iter().collect();
        match self {
            ToolSelection::All => visible,
            ToolSelection::Allow { names } => visible
                .into_iter()
                .filter(|n| names.iter().any(|x| x == n))
                .collect(),
            ToolSelection::Deny { names } => visible
                .into_iter()
                .filter(|n| !names.iter().any(|x| x == n))
                .collect(),
        }
    }
}

/// 一个子智能体类型（内置与自定义共用命名空间，name 唯一）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSpec {
    /// 唯一 snake_case 标识；task 工具 `subagent_type` 参数取值。
    pub name: String,
    /// UI 显示名。
    #[serde(default)]
    pub title: String,
    /// 给模型的选型描述（进 task 工具 description 枚举说明）。
    #[serde(default)]
    pub description: String,
    /// 工具集选择（额外收紧，不是放宽——守卫/审批/沙箱照常全量生效）。
    #[serde(default)]
    pub tools: ToolSelection,
    /// 专属模型 provider id（`p-<nanos>` / `builtin-mlamp`）；None = 继承默认（父当前模型槽）。
    #[serde(default)]
    pub model: Option<String>,
    /// 系统提示词；空 = 用默认子代理提示词（cmx-agent-tools 的 `SUBAGENT_SYSTEM`）。
    #[serde(default)]
    pub system_prompt: String,
    /// 停用后 task 不可选（`subagent_type` 枚举中消失）。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 内置类型不可删、agents.json 只存其 model/enabled 覆盖项。
    #[serde(default)]
    pub builtin: bool,
}

fn default_true() -> bool {
    true
}

/// 子代理专属模型解析缝（core 定义，app 实现）。
pub trait ModelResolver: Send + Sync {
    /// `None` = 继承默认（父当前模型槽，热切自动跟随）；`Some(id)` = 按 provider id 解析。
    /// 失败显式报错，不静默降级。
    fn resolve(&self, provider: Option<&str>) -> Result<Arc<dyn ModelSeam>, String>;
}

/// 内置两条（编译进代码，对齐 ZCode 截图「内置子智能体」）。
pub fn builtin_specs() -> Vec<AgentSpec> {
    vec![
        AgentSpec {
            name: GENERAL_PURPOSE.into(),
            title: "通用子智能体".into(),
            description: "全能子任务执行者，可用与主代理相同的全部工具，适合独立完成一个子任务。".into(),
            tools: ToolSelection::All,
            model: None,
            system_prompt: String::new(),
            enabled: true,
            builtin: true,
        },
        AgentSpec {
            name: EXPLORE.into(),
            title: "只读探索".into(),
            description: "只读调研型子智能体：读代码、搜内容、查网页、列计划，不做任何修改。调研/摸底/信息收集任务优先派给它。".into(),
            tools: ToolSelection::Allow {
                names: [
                    "fs_read",
                    "grep",
                    "glob",
                    "repo_map",
                    "lsp",
                    "web_fetch",
                    "web_search",
                    "update_plan",
                ]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            },
            model: None,
            system_prompt: "你是只读探索型子智能体：只调用只读工具做调研并汇总事实结论，\
             绝不修改任何文件或系统状态。回答给出结论与证据（文件/出处），简洁中文；\
             不要输出「任务已完成」等状态语（完成状态由系统另行呈现，正文只放结论与证据本体）。"
                .into(),
            enabled: true,
            builtin: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_formulas() {
        let visible = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(ToolSelection::All.effective(visible.clone()), visible);
        assert_eq!(
            ToolSelection::Allow { names: vec!["a".into(), "zz".into()] }.effective(visible.clone()),
            vec!["a".to_string()],
            "未知名忽略（冻结清单语义）"
        );
        assert_eq!(
            ToolSelection::Deny { names: vec!["b".into(), "zz".into()] }.effective(visible),
            vec!["a".to_string(), "c".to_string()],
            "Deny = 差集；未知名忽略"
        );
    }

    #[test]
    fn builtins_present_and_named() {
        let specs = builtin_specs();
        assert_eq!(specs.len(), 2);
        assert_eq!(specs[0].name, GENERAL_PURPOSE);
        assert_eq!(specs[1].name, EXPLORE);
        assert!(specs.iter().all(|s| s.builtin && s.enabled));
        // explore 白名单 8 件
        match &specs[1].tools {
            ToolSelection::Allow { names } => assert_eq!(names.len(), 8),
            other => panic!("explore 应为 Allow，实际 {other:?}"),
        }
    }
}
