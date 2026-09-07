//! 三个 cmx 服务连接器工具。各实现 `cmx_agent_core::Tool`，与内置工具同契约，挂进同一 ToolRegistry。
//! 每个连接器持一个 [`CmxServiceClient`]，调真实读端点，把返回**摘要化**（避免把整坨 JSON 灌回模型）。

use async_trait::async_trait;
use cmx_agent_core::tool::{Approval, GuardHints};
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::client::CmxServiceClient;

/// business_chain 各 op → 所需写权限（供链内逐步 PEP）。
fn op_perm(op: &str) -> Option<&'static str> {
    match op {
        "put_object" | "execute_action" => Some("onto:write"),
        "start_instance" | "complete_task" => Some("flow:write"),
        "compute_report" => Some("report:write"),
        _ => None,
    }
}

/// flow 连接器：列流程定义。端点 `POST /api/flow/v1/definitions/list`（活体契约：POST + 信封 data.definitions；
/// auth=on 实例需登录后带 Bearer——由共享令牌槽自动附加）。
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
        // 活体 cmx-flow-server 的定义列表是 POST /definitions/list（见 OpenAPI），非 GET /definitions。
        match self.client.post_write("/api/flow/v1/definitions/list", &json!({})).await {
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
        // 活体 cmx-rpt-server 的报表列表是 GET /api/report-design/reports（见路由表）；其余为历史候选兜底。
        // 注意：该服务 authed 路由需登录后带 Bearer（共享令牌槽自动附加）。
        let mut last_err = String::new();
        for path in [
            "/api/report-design/reports",
            "/api/rpt/v1/reports",
            "/api/report/v1/reports",
            "/api/rpt/v1/report/list",
        ] {
            match self.client.get_data(path).await {
                Ok(data) => {
                    // 活体 report-design/reports 信封形如 data.items[]；旧候选可能是数组或 data.reports[]。
                    let count = data.as_array().map(|a| a.len()).or_else(|| {
                        data.get("items")
                            .or_else(|| data.get("reports"))
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
                Err(e) => last_err = e.to_string(),
            }
        }
        Ok(ToolResult::err(format!(
            "report 连接器：未取到报表列表（末次错误：{last_err}；服务可能未登录/端点路径不同）"
        )))
    }
}

// ── U11 引擎写侧：让 agent 真正**驱动** cmx-flow（起实例 / 办任务）。写操作 requires_approval=Always ──

/// flow 写侧：起流程实例。端点 `POST /api/flow/v1/instances/start`。
pub struct FlowStartInstance {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for FlowStartInstance {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "flow_start_instance",
            "在 cmx-flow 起一个流程实例（definitionKey 必填，可带 businessKey 绑单据、variables 流程变量）。\
             **会真实创建流程实例，需人工审批。**",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "definitionKey": { "type": "string", "description": "流程定义 key（先用 flow_list_definitions 查）" },
                "businessKey": { "type": "string", "description": "业务键（可选，绑定单据）" },
                "variables": { "type": "object", "description": "流程变量（可选）" }
            },
            "required": ["definitionKey"]
        }))
        .guard(GuardHints {
            requires_auth: Some("flow:write".into()),
            requires_approval: Approval::Always,
            idempotent: false,
            // 企业写=仅审批门：Approval::Always 经 X4 人工批准后执行；不标 high_risk，
            // 否则 HighRiskGuard 会在桌面壳 WorkspaceWrite 沙箱下于审批前硬拦（走不到审批卡）。
            high_risk: false,
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(key) = input.get("definitionKey").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("flow_start_instance: 'definitionKey' 必填"));
        };
        let mut body = json!({ "definitionKey": key });
        if let Some(bk) = input.get("businessKey") {
            body["businessKey"] = bk.clone();
        }
        if let Some(vars) = input.get("variables") {
            body["variables"] = vars.clone();
        }
        match self.client.post_write("/api/flow/v1/instances/start", &body).await {
            Ok(data) => Ok(ToolResult::ok(json!({
                "service": "cmx-flow",
                "started": true,
                "instanceId": data.get("instanceId").or_else(|| data.get("id")),
                "status": data.get("status"),
                "data": data,
            }))),
            Err(e) => Ok(ToolResult::err(format!("flow 起实例失败: {e}"))),
        }
    }
}

/// flow 写侧：办理（完成）任务。端点 `POST /api/flow/v1/tasks/complete`（taskId+instanceId 在 body，均必填）。
pub struct FlowCompleteTask {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for FlowCompleteTask {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "flow_complete_task",
            "办理（完成）cmx-flow 的一个待办任务（taskId + instanceId 必填，可带 variables、decision、comment）。**会真实推进流程，需人工审批。**",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "taskId": { "type": "string", "description": "任务 id" },
                "instanceId": { "type": "string", "description": "流程实例 id（起实例返回的 id）" },
                "variables": { "type": "object", "description": "提交的表单/流程变量（可选）" },
                "decision": { "type": "string", "description": "审批决定（可选，如 approve/reject）" },
                "comment": { "type": "string", "description": "审批意见（可选）" }
            },
            "required": ["taskId", "instanceId"]
        }))
        .guard(GuardHints {
            requires_auth: Some("flow:write".into()),
            requires_approval: Approval::Always,
            idempotent: false,
            // 企业写=仅审批门：Approval::Always 经 X4 人工批准后执行；不标 high_risk，
            // 否则 HighRiskGuard 会在桌面壳 WorkspaceWrite 沙箱下于审批前硬拦（走不到审批卡）。
            high_risk: false,
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(tid) = input.get("taskId").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("flow_complete_task: 'taskId' 必填"));
        };
        let Some(iid) = input.get("instanceId").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("flow_complete_task: 'instanceId' 必填（起实例返回的 id）"));
        };
        // 活体契约：POST /tasks/complete，body 含 taskId + instanceId（均必填）+ 可选 variables/decision/comment。
        let mut body = json!({ "taskId": tid, "instanceId": iid });
        if let Some(vars) = input.get("variables") {
            body["variables"] = vars.clone();
        }
        if let Some(d) = input.get("decision") {
            body["decision"] = d.clone();
        }
        if let Some(c) = input.get("comment") {
            body["comment"] = c.clone();
        }
        match self.client.post_write("/api/flow/v1/tasks/complete", &body).await {
            Ok(data) => Ok(ToolResult::ok(json!({
                "service": "cmx-flow",
                "completed": true,
                "taskId": tid,
                "data": data,
            }))),
            Err(e) => Ok(ToolResult::err(format!("flow 办理任务失败: {e}"))),
        }
    }
}

// ── U11 引擎写侧续：onto 建对象 / 执行动作 + report 计算 ──

/// onto 写侧：新建/更新对象实例。端点 `POST /api/onto/v1/objects/{objectType}`，body `{properties:{...}}`。
pub struct OntoPutObject {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for OntoPutObject {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "onto_put_object",
            "在 cmx-ontology 新建/更新一个对象实例（objectType + properties）。\
             **会真实写入本体库，需人工审批。** 先用 onto_list_object_types 查对象类型与主键。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "objectType": { "type": "string", "description": "对象类型 apiName（如 WsOrd）" },
                "properties": { "type": "object", "description": "对象属性（须含主键，如 {id:'WO-1', amount:500}）" }
            },
            "required": ["objectType", "properties"]
        }))
        .guard(GuardHints {
            requires_auth: Some("onto:write".into()),
            requires_approval: Approval::Always,
            idempotent: false,
            // 企业写=仅审批门：Approval::Always 经 X4 人工批准后执行；不标 high_risk，
            // 否则 HighRiskGuard 会在桌面壳 WorkspaceWrite 沙箱下于审批前硬拦（走不到审批卡）。
            high_risk: false,
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(ot) = input.get("objectType").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("onto_put_object: 'objectType' 必填"));
        };
        let Some(props) = input.get("properties").filter(|p| p.is_object()) else {
            return Ok(ToolResult::err("onto_put_object: 'properties' 必填（对象）"));
        };
        let body = json!({ "properties": props });
        let path = format!("/api/onto/v1/objects/{ot}");
        match self.client.post_write(&path, &body).await {
            Ok(data) => Ok(ToolResult::ok(json!({
                "service": "cmx-ontology",
                "wrote": true,
                "objectType": ot,
                "data": data,
            }))),
            Err(e) => Ok(ToolResult::err(format!("onto 写对象失败: {e}"))),
        }
    }
}

/// onto 写侧：执行动作（O4 动作引擎，校验+编辑+原子写回；dryRun 只预演）。
/// 端点 `POST /api/onto/v1/action-types/{apiName}/execute`，body `{params:{...}, dryRun?}`。
pub struct OntoExecuteAction {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for OntoExecuteAction {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "onto_execute_action",
            "执行 cmx-ontology 的一个动作类型（O4 动作引擎：校验→改对象→副作用，原子写回）。\
             dryRun=true 只试算不落库（不需审批）；dryRun=false 真写（需审批）。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "actionType": { "type": "string", "description": "动作类型 apiName（如 wsClose）" },
                "params": { "type": "object", "description": "动作参数（如 {orderId:'WO-1'}）" },
                "dryRun": { "type": "boolean", "default": false, "description": "true=只试算预演；false=真写库" }
            },
            "required": ["actionType", "params"]
        }))
        .guard(GuardHints {
            requires_auth: Some("onto:write".into()),
            // 试算(dryRun)无副作用；真写需审批。守卫按 Always 稳妥兜底，dryRun 亦过卡（可接受）。
            requires_approval: Approval::Always,
            idempotent: false,
            // 企业写=仅审批门：Approval::Always 经 X4 人工批准后执行；不标 high_risk，
            // 否则 HighRiskGuard 会在桌面壳 WorkspaceWrite 沙箱下于审批前硬拦（走不到审批卡）。
            high_risk: false,
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(at) = input.get("actionType").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("onto_execute_action: 'actionType' 必填"));
        };
        let params = input.get("params").cloned().unwrap_or_else(|| json!({}));
        let dry = input.get("dryRun").and_then(|v| v.as_bool()).unwrap_or(false);
        let body = json!({ "params": params, "dryRun": dry });
        let path = format!("/api/onto/v1/action-types/{at}/execute");
        match self.client.post_write(&path, &body).await {
            Ok(data) => Ok(ToolResult::ok(json!({
                "service": "cmx-ontology",
                "executed": true,
                "dryRun": dry,
                "actionType": at,
                "data": data,
            }))),
            Err(e) => Ok(ToolResult::err(format!("onto 执行动作失败: {e}"))),
        }
    }
}

/// report 写侧：计算报表（装载公式→递归求值→落 cr_cell_data）。
/// 端点 `POST /api/report-design/reports/{code}/compute`，body `{orgCode, periodCode, schemeCode}`。
pub struct ReportCompute {
    pub client: CmxServiceClient,
}

#[async_trait]
impl Tool for ReportCompute {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "report_compute",
            "计算 cmx-report 的一张报表（按 组织/期间/方案 取数求值并落库）。**会写入计算结果，需人工审批。**",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "reportCode": { "type": "string", "description": "报表编码（如 CBS/CIS）" },
                "orgCode": { "type": "string", "description": "组织编码" },
                "periodCode": { "type": "string", "description": "期间（如 2026-06）" },
                "schemeCode": { "type": "string", "description": "方案编码（如 CAS_LEGAL）" }
            },
            "required": ["reportCode", "orgCode", "periodCode"]
        }))
        .guard(GuardHints {
            requires_auth: Some("report:write".into()),
            requires_approval: Approval::Always,
            idempotent: false,
            // 企业写=仅审批门：Approval::Always 经 X4 人工批准后执行；不标 high_risk，
            // 否则 HighRiskGuard 会在桌面壳 WorkspaceWrite 沙箱下于审批前硬拦（走不到审批卡）。
            high_risk: false,
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(code) = input.get("reportCode").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("report_compute: 'reportCode' 必填"));
        };
        let body = json!({
            "orgCode": input.get("orgCode").and_then(|v| v.as_str()).unwrap_or(""),
            "periodCode": input.get("periodCode").and_then(|v| v.as_str()).unwrap_or(""),
            "schemeCode": input.get("schemeCode").and_then(|v| v.as_str()).unwrap_or(""),
        });
        let path = format!("/api/report-design/reports/{code}/compute");
        match self.client.post_write(&path, &body).await {
            Ok(data) => Ok(ToolResult::ok(json!({
                "service": "cmx-report",
                "computed": true,
                "reportCode": code,
                "cellCount": data.get("cells").and_then(|c| c.as_array()).map(|a| a.len()),
                "errorCount": data.get("errorCount"),
                "data": data,
            }))),
            Err(e) => Ok(ToolResult::err(format!("report 计算失败: {e}"))),
        }
    }
}

// ── U12 本体上下文：一站式拉企业域模型（对象类型/关系/动作/流程/报表）供 agent 接地后再读写 ──

/// 汇总一个 scope 的读结果为紧凑摘要项列表（截断防灌爆上下文）。
fn summarize(items: &Value, take: usize, f: impl Fn(&Value) -> Value) -> (usize, Vec<Value>) {
    let arr = items
        .as_array()
        .cloned()
        .or_else(|| items.get("definitions").and_then(|d| d.as_array()).cloned())
        .or_else(|| items.get("reports").and_then(|d| d.as_array()).cloned())
        .unwrap_or_default();
    let total = arr.len();
    (total, arr.iter().take(take).map(f).collect())
}

/// U12：企业上下文——并发拉 onto 对象类型/关系/动作 + flow 定义 + report 列表，返回域模型摘要。
/// 让 agent「先看有哪些对象/动作/流程」再去写（接地企业模型，减少瞎试）。持三个 client（onto/flow/report）。
pub struct EnterpriseContext {
    pub onto: CmxServiceClient,
    pub flow: CmxServiceClient,
    pub report: CmxServiceClient,
}

#[async_trait]
impl Tool for EnterpriseContext {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "enterprise_context",
            "一站式拉取当前企业域模型：本体对象类型/关系/动作 + 流程定义 + 报表清单。\
             做任何企业读写前先调它接地（知道有哪些对象、能跑哪些动作/流程），避免瞎猜 key。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "scopes": { "type": "array", "items": { "type": "string" },
                    "description": "可选，限定范围子集：onto/flow/report；缺省全拉" },
                "limit": { "type": "integer", "default": 30, "description": "每类最多返回条数" }
            }
        }))
        .guard(GuardHints {
            requires_auth: Some("onto:read".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let take = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(30) as usize;
        let scopes: Vec<String> = input
            .get("scopes")
            .and_then(|s| s.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();
        let want = |s: &str| scopes.is_empty() || scopes.iter().any(|x| x == s);

        // 并发拉取（各自失败优雅降级为 null）
        let (obj_types, links, actions, flows, reports) = tokio::join!(
            async { if want("onto") { self.onto.get_data("/api/onto/v1/object-types").await.ok() } else { None } },
            async { if want("onto") { self.onto.get_data("/api/onto/v1/link-types").await.ok() } else { None } },
            async { if want("onto") { self.onto.get_data("/api/onto/v1/action-types").await.ok() } else { None } },
            async { if want("flow") { self.flow.get_data("/api/flow/v1/definitions").await.ok() } else { None } },
            async {
                if want("report") {
                    for p in ["/api/rpt/v1/reports", "/api/report/v1/reports", "/api/report-design/reports"] {
                        if let Ok(d) = self.report.get_data(p).await {
                            return Some(d);
                        }
                    }
                }
                None
            },
        );

        let mut ctx = json!({});
        if let Some(v) = obj_types {
            let (n, items) = summarize(&v, take, |o| json!({
                "apiName": o.get("apiName"), "displayName": o.get("displayName"),
                "primaryKey": o.get("primaryKey"), "propertyCount": o.get("propertyCount"),
            }));
            ctx["objectTypes"] = json!({ "total": n, "items": items });
        }
        if let Some(v) = links {
            let (n, items) = summarize(&v, take, |l| json!({
                "apiName": l.get("apiName"), "displayName": l.get("displayName"),
                "from": l.get("fromObjectType").or_else(|| l.get("from")), "to": l.get("toObjectType").or_else(|| l.get("to")),
            }));
            ctx["relations"] = json!({ "total": n, "items": items });
        }
        if let Some(v) = actions {
            let (n, items) = summarize(&v, take, |a| json!({
                "apiName": a.get("apiName"), "displayName": a.get("displayName"), "status": a.get("status"),
            }));
            ctx["actions"] = json!({ "total": n, "items": items });
        }
        if let Some(v) = flows {
            let (n, items) = summarize(&v, take, |d| json!({
                "key": d.get("key"), "name": d.get("name"), "startable": d.get("startable"),
            }));
            ctx["flows"] = json!({ "total": n, "items": items });
        }
        if let Some(v) = reports {
            let (n, items) = summarize(&v, take, |r| json!({
                "code": r.get("code").or_else(|| r.get("reportCode")), "name": r.get("name"),
            }));
            ctx["reports"] = json!({ "total": n, "items": items });
        }

        let empty = ctx.as_object().map(|o| o.is_empty()).unwrap_or(true);
        if empty {
            return Ok(ToolResult::err(
                "enterprise_context: 未拉到任何域模型（cmx 服务可能未启动）",
            ));
        }
        Ok(ToolResult::ok(json!({ "service": "cmx-enterprise", "context": ctx })))
    }
}

// ── U14 单据→凭证→流程→报表联动：一条可审计的引擎流水线，一次审批跑完，步间用 $N.field 引用穿线 ──

/// 把 input 里的 `$N` / `$N.a.b` 引用替换为第 N 步(1基)输出的对应字段（穿线：前一步的 id 喂给后一步）。
fn resolve_refs(v: &Value, prior: &[Value]) -> Value {
    match v {
        Value::String(s) if s.starts_with('$') => {
            let body = &s[1..];
            let (idx_str, path) = match body.split_once('.') {
                Some((i, p)) => (i, Some(p)),
                None => (body, None),
            };
            if let Ok(idx) = idx_str.parse::<usize>() {
                if idx >= 1 && idx <= prior.len() {
                    let base = &prior[idx - 1];
                    return match path {
                        Some(p) => base
                            .pointer(&format!("/{}", p.replace('.', "/")))
                            .cloned()
                            .unwrap_or(Value::Null),
                        None => base.clone(),
                    };
                }
            }
            v.clone()
        }
        Value::Object(m) => Value::Object(m.iter().map(|(k, val)| (k.clone(), resolve_refs(val, prior))).collect()),
        Value::Array(a) => Value::Array(a.iter().map(|x| resolve_refs(x, prior)).collect()),
        _ => v.clone(),
    }
}

/// U14 引擎联动流水线。持三 client；一次调用按序跑多步引擎写，fail-fast，返回全程轨迹。
/// **一次审批覆盖整条链**（人看到完整计划后批准即授权）——per-op PEP 未在链内逐步校验，由人在环兜底。
pub struct EngineChain {
    pub onto: CmxServiceClient,
    pub flow: CmxServiceClient,
    pub report: CmxServiceClient,
    /// U13 链内逐步 PEP（可选）：设了则每步 op 先按 [`op_perm`] 向 PDP 判权，deny 则停链。
    pub pep: Option<std::sync::Arc<crate::dataauth::DataAuthPep>>,
    /// 共享授权主体（与 AuthGuard 同一份；登录后为真实用户）。
    pub identity: Option<std::sync::Arc<std::sync::RwLock<cmx_agent_core::Subject>>>,
}

impl EngineChain {
    /// 链内逐步 PEP：判定本步 op 的写权限。未启用 PEP → 放行（靠工具级一次审批兜底）。
    async fn authorize_op(&self, op: &str) -> Result<(), String> {
        let (Some(pep), Some(id)) = (&self.pep, &self.identity) else {
            return Ok(()); // 未接数据权限：不逐步判，沿用一次人审
        };
        let Some(perm) = op_perm(op) else { return Ok(()) };
        let subj = id.read().map_err(|_| "授权主体锁毒化".to_string())?.clone();
        if pep.decide(&subj, perm).await {
            Ok(())
        } else {
            Err(format!("op '{op}' 权限不足（{perm}），链在此步中止"))
        }
    }

    async fn run_op(&self, op: &str, input: &Value) -> Result<Value, String> {
        // U13：先逐步判权（deny 即停链），再执行本步。
        self.authorize_op(op).await?;
        let s = |k: &str| input.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        match op {
            "put_object" => {
                let ot = s("objectType");
                let props = input.get("properties").cloned().unwrap_or_else(|| json!({}));
                self.onto
                    .post_write(&format!("/api/onto/v1/objects/{ot}"), &json!({ "properties": props }))
                    .await
                    .map_err(|e| e.to_string())
            }
            "execute_action" => {
                let at = s("actionType");
                let body = json!({ "params": input.get("params").cloned().unwrap_or_else(|| json!({})),
                                   "dryRun": input.get("dryRun").and_then(|v| v.as_bool()).unwrap_or(false) });
                self.onto
                    .post_write(&format!("/api/onto/v1/action-types/{at}/execute"), &body)
                    .await
                    .map_err(|e| e.to_string())
            }
            "start_instance" => {
                let mut body = json!({ "definitionKey": s("definitionKey") });
                if let Some(bk) = input.get("businessKey") { body["businessKey"] = bk.clone(); }
                if let Some(vars) = input.get("variables") { body["variables"] = vars.clone(); }
                self.flow.post_write("/api/flow/v1/instances/start", &body).await.map_err(|e| e.to_string())
            }
            "complete_task" => {
                let tid = s("taskId");
                // 活体契约：POST /tasks/complete，taskId + instanceId 在 body（均必填）。
                let mut body = json!({ "taskId": tid, "instanceId": s("instanceId") });
                if let Some(vars) = input.get("variables") { body["variables"] = vars.clone(); }
                if let Some(d) = input.get("decision") { body["decision"] = d.clone(); }
                if let Some(c) = input.get("comment") { body["comment"] = c.clone(); }
                self.flow
                    .post_write("/api/flow/v1/tasks/complete", &body)
                    .await
                    .map_err(|e| e.to_string())
            }
            "compute_report" => {
                let code = s("reportCode");
                let body = json!({ "orgCode": s("orgCode"), "periodCode": s("periodCode"), "schemeCode": s("schemeCode") });
                self.report
                    .post_write(&format!("/api/report-design/reports/{code}/compute"), &body)
                    .await
                    .map_err(|e| e.to_string())
            }
            other => Err(format!("未知 op：{other}（支持 put_object/execute_action/start_instance/complete_task/compute_report）")),
        }
    }
}

#[async_trait]
impl Tool for EngineChain {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "business_chain",
            "跑一条端到端业务联动流水线（如 单据→凭证→流程→报表），一次审批跑完全部步骤。\
             steps=[{op,input}]；op∈put_object/execute_action/start_instance/complete_task/compute_report；\
             input 里可用 $N.field 引用第 N 步输出（如把建对象拿到的 id 喂给起流程的 businessKey）。**会真实写多个引擎，需审批。**",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "steps": {
                    "type": "array",
                    "description": "有序步骤：[{op:'put_object', input:{objectType,properties}}, {op:'start_instance', input:{definitionKey, businessKey:'$1.data.id'}}, ...]",
                    "items": { "type": "object", "properties": { "op": {"type":"string"}, "input": {"type":"object"} }, "required": ["op"] }
                }
            },
            "required": ["steps"]
        }))
        .guard(GuardHints {
            requires_auth: Some("onto:write".into()),
            requires_approval: Approval::Always,
            idempotent: false,
            // 企业写=仅审批门：Approval::Always 经 X4 人工批准后执行；不标 high_risk，
            // 否则 HighRiskGuard 会在桌面壳 WorkspaceWrite 沙箱下于审批前硬拦（走不到审批卡）。
            high_risk: false,
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(steps) = input.get("steps").and_then(|v| v.as_array()) else {
            return Ok(ToolResult::err("business_chain: 'steps' 必填（数组）"));
        };
        let mut prior: Vec<Value> = Vec::new();
        let mut trace: Vec<Value> = Vec::new();
        let mut completed = true;
        for (i, st) in steps.iter().enumerate() {
            let op = st.get("op").and_then(|v| v.as_str()).unwrap_or("");
            let raw = st.get("input").cloned().unwrap_or_else(|| json!({}));
            let resolved = resolve_refs(&raw, &prior);
            match self.run_op(op, &resolved).await {
                Ok(data) => {
                    trace.push(json!({ "step": i + 1, "op": op, "ok": true, "output": data.clone() }));
                    prior.push(data);
                }
                Err(e) => {
                    trace.push(json!({ "step": i + 1, "op": op, "ok": false, "error": e }));
                    completed = false;
                    break; // fail-fast：前一步失败不再往下写
                }
            }
        }
        Ok(ToolResult::ok(json!({
            "service": "cmx-chain",
            "completed": completed,
            "stepCount": trace.len(),
            "steps": trace,
        })))
    }
}
