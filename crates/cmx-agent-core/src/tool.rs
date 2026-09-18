//! 工具平面（方案图 5）：智能体的「双手」。统一工具契约 = name/description/inputSchema + **守卫标注**
//! （是否需权限？是否需审批？是否幂等？是否高危？）——让 LLM 可发现、让护栏可拦截、让审计可回放。
//!
//! M0 内置工具在 `cmx-agent-tools`；后续 cmx-flow/rules/ontology/report/data-auth 与 MCP 外接均实现同一
//! [`Tool`] trait 挂进 [`ToolRegistry`]。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// 一次工具调用意图（由模型产出；`id` 关联其后的守卫裁决与结果）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub input: Value,
}

impl ToolCall {
    /// 新建一次调用，自动分配 uuid 调用号。
    pub fn new(name: impl Into<String>, input: Value) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            input,
        }
    }

    /// 指定调用号（测试可复现）。
    pub fn with_id(id: impl Into<String>, name: impl Into<String>, input: Value) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            input,
        }
    }
}

/// 审批要求（守卫标注之一）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Approval {
    /// 从不需要人工审批。
    #[default]
    Never,
    /// 视条件（M0 一律按需审批；后续接 FEEL 条件与 cmx-flow）。
    Conditional,
    /// 总是需要人工审批。
    Always,
}

/// 守卫标注：护栏据此决定对该工具施加哪些闸门，无需"猜"。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GuardHints {
    /// 需要的权限点（PDP/PEP 据此判定，接 cmx-data-auth）。`None` = 无需鉴权。
    #[serde(default)]
    pub requires_auth: Option<String>,
    /// 审批要求。
    #[serde(default)]
    pub requires_approval: Approval,
    /// 是否幂等（影响重试与审批策略）。
    #[serde(default)]
    pub idempotent: bool,
    /// 是否高危（由 [`crate::guard::HighRiskGuard`] 要求审批，不得自动批准）。
    #[serde(default)]
    pub high_risk: bool,
    /// 需要联网（工具能力描述，不作为执行围栏）。
    #[serde(default)]
    pub network: bool,
    /// 有写副作用（工具能力描述，计划模式仍按工具白名单控制）。
    #[serde(default)]
    pub writes: bool,
    /// 写目标路径所在的入参字段名（如 fs_write 的 "path"）。非空时由
    /// [`crate::guard::WorkspaceWriteGuard`] 做工作目录边界判定：目标在工作目录内放行，
    /// 目录外升级为需审批。空 = 不做边界判定（该工具的审批仅由 `requires_approval` 决定）。
    #[serde(default)]
    pub write_path_args: Vec<String>,
    /// 参数触发审批：`Some((字段, 取值集合))` 时，入参该字段命中集合即升级为需审批
    /// （如 git 的写子命令 add/commit）。读子命令不在集合内 → 不额外要求审批。
    #[serde(default)]
    pub approval_arg_values: Option<(String, Vec<String>)>,
}

/// 工具规格（对齐 MCP：name/description/inputSchema + 守卫标注 x-guard）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    /// JSON Schema（对象）。M0 不做严格校验，仅透传给模型；后续接确定性校验（护栏②）。
    #[serde(default)]
    pub input_schema: Value,
    #[serde(default, rename = "x-guard")]
    pub guard: GuardHints,
    /// 交互提问工具（如 ask_user）：会挂起等待用户输入。内核据此在 pre 相托管执行
    /// （register + 落 QuestionAsked + 执行段挂起等答），不走普通 invoke 路径。
    /// 该类工具**不触碰 SessionLog**——事件只从内核写入会话日志（方案 20260914 §4.3 不变量）。
    #[serde(default)]
    pub user_interactive: bool,
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: serde_json::json!({"type": "object"}),
            guard: GuardHints::default(),
            user_interactive: false,
        }
    }

    pub fn schema(mut self, schema: Value) -> Self {
        self.input_schema = schema;
        self
    }

    pub fn guard(mut self, guard: GuardHints) -> Self {
        self.guard = guard;
        self
    }

    /// 标记为交互提问工具（内核托管挂起路径）。
    pub fn user_interactive(mut self) -> Self {
        self.user_interactive = true;
        self
    }
}

/// 工具结果（回灌给模型；`ok=false` 让模型自愈而非中止回合）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolResult {
    pub ok: bool,
    pub output: Value,
}

impl ToolResult {
    pub fn ok(output: Value) -> Self {
        Self { ok: true, output }
    }

    pub fn err(msg: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: serde_json::json!({ "error": msg.into() }),
        }
    }
}

/// 工具执行失败（非致命；内核转为 `ToolResult::err` 回灌）。
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// 工具执行上下文：工作区路径基准与会话归属。
pub struct ToolCtx<'a> {
    /// 相对路径的解析基准，不是安全围栏，也不限制绝对路径访问。
    pub workspace_roots: &'a [PathBuf],
    /// 本回合所属会话 id（方案 20260914 阶段一：子智能体每父会话并发计数、per-session 能力用）。
    pub session_id: &'a str,
}

/// 工具 trait：一件"双手"。
#[async_trait]
pub trait Tool: Send + Sync {
    /// 工具规格（含守卫标注）。
    fn spec(&self) -> ToolSpec;

    /// 执行。入参为模型给的 JSON；返回结果或非致命错误。
    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError>;
}

/// 工具注册表（= dsh `ctx.tools`）。按名索引，供路由与"给模型的工具清单"。
///
/// **内部共享 + 可运行时增删**：`tools` 用 `Arc<RwLock<..>>`，故 `#[derive(Clone)]` 得到的是
/// **同一张表的句柄**（非独立副本）。`Agent` 持有一份、`AgentApp` 经 `agent.tools().clone()` 再持一份，
/// 二者指向同一表——插件安装/卸载经 [`ToolRegistry::register_dyn`]/[`ToolRegistry::unregister`]（`&self`）
/// 即时改表，下一回合 `specs()`/`get()` 立刻可见（热注册，无需重启）。
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: Arc<RwLock<HashMap<String, Arc<dyn Tool>>>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册工具（重名覆盖）。装配期链式用（`&mut self`）；内部亦锁写共享表。
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> &mut Self {
        let name = tool.spec().name;
        self.tools.write().expect("tools lock").insert(name, tool);
        self
    }

    /// 运行时**热注册**（`&self`，经 `Arc<Agent>` 也能调）。
    /// 已存在同名工具时**拒绝**并返回 `Err`（方案 §7.2 / 红队 N3：守卫白名单按工具名匹配，
    /// 恶意插件取名 `fs_read`/`task` 会整体顶掉内置同名工具借道白名单——注册面必须封堵；
    /// 装配期 `register`（`&mut self`）不受此限，重复注册仍为覆盖语义）。
    pub fn register_dyn(&self, tool: Arc<dyn Tool>) -> Result<(), String> {
        let name = tool.spec().name;
        let mut table = self.tools.write().expect("tools lock");
        if table.contains_key(&name) {
            return Err(format!(
                "工具「{name}」已注册（内置或先装插件），拒绝同名遮蔽"
            ));
        }
        table.insert(name, tool);
        Ok(())
    }

    /// 运行时**热卸载**（`&self`）。返回是否移除了该名工具。
    pub fn unregister(&self, name: &str) -> bool {
        self.tools.write().expect("tools lock").remove(name).is_some()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().expect("tools lock").get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.read().expect("tools lock").contains_key(name)
    }

    /// 给模型的工具清单（供其发现与选择）。
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut v: Vec<ToolSpec> = self
            .tools
            .read()
            .expect("tools lock")
            .values()
            .map(|t| t.spec())
            .collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn len(&self) -> usize {
        self.tools.read().expect("tools lock").len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.read().expect("tools lock").is_empty()
    }
}
