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
use std::sync::Arc;

use crate::guard::SandboxMode;

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
    /// 是否高危（仅在 danger-full-access 沙箱放行，否则拦截）。
    #[serde(default)]
    pub high_risk: bool,
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
}

impl ToolSpec {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            input_schema: serde_json::json!({"type": "object"}),
            guard: GuardHints::default(),
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

/// 工具执行上下文（M0：沙箱模式 + 允许的文件根。后续扩展工作目录、租户、审计句柄等）。
pub struct ToolCtx<'a> {
    pub sandbox: SandboxMode,
    pub allowed_roots: &'a [PathBuf],
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
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册工具（重名覆盖）。
    pub fn register(&mut self, tool: Arc<dyn Tool>) -> &mut Self {
        let name = tool.spec().name;
        self.tools.insert(name, tool);
        self
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// 给模型的工具清单（供其发现与选择）。
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut v: Vec<ToolSpec> = self.tools.values().map(|t| t.spec()).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}
