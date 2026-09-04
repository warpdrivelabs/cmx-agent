//! 三个 cmx 服务连接器工具。各实现 `cmx_agent_core::Tool`，与内置工具同契约，挂进同一 ToolRegistry。
//! 每个连接器持一个 [`CmxServiceClient`]，调真实读端点，把返回**摘要化**（避免把整坨 JSON 灌回模型）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::client::CmxServiceClient;

/// flow 连接器：列流程定义。端点 `GET /api/flow/v1/definitions`。
pub struct FlowConnector {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for FlowConnector {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "flow_list_definitions",
            "列出 cmx-flow 流程引擎中已加载的流程定义（key/名称/节点数）。当用户问“有哪些流程/流程定义/审批流”时用。",
        )
        .guard(GuardHints {
            requires_auth: Some("flow:read".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        match self.client.get_data("/api/flow/v1/definitions").await {
            Ok(data) => {
                let defs = data.get("definitions").and_then(|d| d.as_array());
                let summary: Vec<Value> = defs
                    .map(|arr| {
                        arr.iter()
                            .map(|d| {
                                json!({
                                    "key": d.get("key"),
                                    "name": d.get("name"),
                                    "startable": d.get("startable"),
                                    "nodeCount": d.get("nodes").and_then(|n| n.as_array()).map(|a| a.len()),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(ToolResult::ok(json!({
                    "service": "cmx-flow",
                    "count": summary.len(),
                    "definitions": summary,
                })))
            }
            Err(e) => Ok(ToolResult::err(format!("flow 连接器调用失败: {e}"))),
        }
    }
}

/// onto 连接器：列对象类型。端点 `GET /api/onto/v1/object-types`。
pub struct OntoConnector {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for OntoConnector {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "onto_list_object_types",
            "列出 cmx-ontology 本体平台的对象类型（apiName/显示名/单据类型）。当用户问“有哪些对象/本体/业务实体”时用。",
        )
        .guard(GuardHints {
            requires_auth: Some("onto:read".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        match self.client.get_data("/api/onto/v1/object-types").await {
            Ok(data) => {
                let arr = data.as_array().cloned().unwrap_or_default();
                let summary: Vec<Value> = arr
                    .iter()
                    .map(|o| {
                        json!({
                            "apiName": o.get("apiName"),
                            "displayName": o.get("displayName"),
                            "status": o.get("status"),
                            "propertyCount": o.get("propertyCount"),
                            "docType": o.get("docType").and_then(|d| d.get("name")),
                        })
                    })
                    .collect();
                Ok(ToolResult::ok(json!({
                    "service": "cmx-ontology",
                    "count": summary.len(),
                    "objectTypes": summary,
                })))
            }
            Err(e) => Ok(ToolResult::err(format!("ontology 连接器调用失败: {e}"))),
        }
    }
}

/// report 连接器：列报表。端点尽力（多路径兜底），失败优雅降级。
pub struct ReportConnector {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for ReportConnector {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "report_list_reports",
            "列出 cmx-report 报表平台的报表定义。当用户问“有哪些报表/财报模板”时用。",
        )
        .guard(GuardHints {
            requires_auth: Some("report:read".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        // 报表列表端点路径未 100% 确认——多候选兜底，任一成功即返回。
        for path in [
            "/api/rpt/v1/reports",
            "/api/report/v1/reports",
            "/api/rpt/v1/report/list",
        ] {
            if let Ok(data) = self.client.get_data(path).await {
                let count = data.as_array().map(|a| a.len()).or_else(|| {
                    data.get("reports")
                        .and_then(|r| r.as_array())
                        .map(|a| a.len())
                });
                return Ok(ToolResult::ok(json!({
                    "service": "cmx-report",
                    "endpoint": path,
                    "count": count,
                    "data": data,
                })));
            }
        }
        Ok(ToolResult::err(
            "report 连接器：未找到可用的报表列表端点（服务可能未启动或端点路径不同）",
        ))
    }
}
