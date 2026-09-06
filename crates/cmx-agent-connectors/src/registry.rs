//! 连接器注册表：连接器元信息（给面板）+ 把连接器工具挂进内核 ToolRegistry + 并发健康探测。

use std::sync::Arc;

use cmx_agent_core::ToolRegistry;

use crate::client::CmxServiceClient;
use crate::connectors::{
    EngineChain, EnterpriseContext, FlowCompleteTask, FlowConnector, FlowStartInstance,
    OntoConnector, OntoExecuteAction, OntoPutObject, ReportCompute, ReportConnector,
};
use crate::health::{ConnectorStatus, probe};

/// 三服务的 base_url 配置（缺省指向本机标准端口）。
#[derive(Debug, Clone)]
pub struct ConnectorConfig {
    pub flow_base: String,
    pub onto_base: String,
    pub report_base: String,
    pub tenant: String,
    pub user: String,
    pub api_key: Option<String>,
}

impl Default for ConnectorConfig {
    fn default() -> Self {
        Self {
            flow_base: "http://127.0.0.1:8091".into(),
            onto_base: "http://127.0.0.1:8097".into(),
            report_base: "http://127.0.0.1:8092".into(),
            tenant: "default".into(),
            user: "admin".into(),
            api_key: None,
        }
    }
}

/// 一个连接器的静态描述（面板卡片用；不含 live 状态）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectorDescriptor {
    pub id: String,
    /// 中文显示名。
    pub name: String,
    pub service: String,
    pub base_url: String,
    /// 它暴露的工具名（面板 chip）。
    pub tools: Vec<String>,
    /// 一句话说明。
    pub description: String,
}

/// 一个连接器卡片 = 描述 + live 状态（给前端面板）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConnectorCard {
    #[serde(flatten)]
    pub descriptor: ConnectorDescriptor,
    pub status: ConnectorStatus,
}

/// 连接器注册表。
pub struct ConnectorRegistry {
    descriptors: Vec<ConnectorDescriptor>,
    clients: Vec<CmxServiceClient>,
    config: ConnectorConfig,
}

impl ConnectorRegistry {
    /// 按配置装配三连接器。
    pub fn new(config: ConnectorConfig) -> Self {
        let mk = |base: &str| {
            CmxServiceClient::new(base)
                .with_identity(config.tenant.clone(), config.user.clone())
                .with_api_key(config.api_key.clone())
        };
        let descriptors = vec![
            ConnectorDescriptor {
                id: "flow".into(),
                name: "流程引擎".into(),
                service: "cmx-flow".into(),
                base_url: config.flow_base.clone(),
                tools: vec!["flow_list_definitions".into()],
                description: "发起/查询审批流程，读流程定义".into(),
            },
            ConnectorDescriptor {
                id: "onto".into(),
                name: "本体平台".into(),
                service: "cmx-ontology".into(),
                base_url: config.onto_base.clone(),
                tools: vec!["onto_list_object_types".into()],
                description: "读业务对象类型与实例（企业数字孪生）".into(),
            },
            ConnectorDescriptor {
                id: "report".into(),
                name: "报表平台".into(),
                service: "cmx-report".into(),
                base_url: config.report_base.clone(),
                tools: vec!["report_list_reports".into()],
                description: "读报表定义与财报模板".into(),
            },
        ];
        let clients = vec![
            mk(&config.flow_base),
            mk(&config.onto_base),
            mk(&config.report_base),
        ];
        Self {
            descriptors,
            clients,
            config,
        }
    }

    pub fn config(&self) -> &ConnectorConfig {
        &self.config
    }

    /// 注入共享令牌槽到三连接器 client——登录后所有连接器读写自动带 `Authorization: Bearer`
    /// （auth=on 的服务如 cmx-flow 必需；auth=off 的 cmx-ontology 无害忽略）。须在 `register_into` 前调用。
    pub fn with_token(mut self, store: crate::client::TokenStore) -> Self {
        self.clients = self
            .clients
            .into_iter()
            .map(|c| c.with_token(store.clone()))
            .collect();
        self
    }

    pub fn descriptors(&self) -> &[ConnectorDescriptor] {
        &self.descriptors
    }

    /// 把三连接器工具挂进内核 ToolRegistry（与内置工具并存）。
    pub fn register_into(&self, registry: &mut ToolRegistry) {
        registry
            .register(Arc::new(FlowConnector {
                client: self.clients[0].clone(),
            }))
            .register(Arc::new(OntoConnector {
                client: self.clients[1].clone(),
            }))
            .register(Arc::new(ReportConnector {
                client: self.clients[2].clone(),
            }));
        // U11 引擎写侧：flow 起实例 / 办任务（写操作，requires_approval=Always）。
        registry
            .register(Arc::new(FlowStartInstance {
                client: self.clients[0].clone(),
            }))
            .register(Arc::new(FlowCompleteTask {
                client: self.clients[0].clone(),
            }));
        // U11 续：onto 建对象 / 执行动作（clients[1]）+ report 计算（clients[2]）。
        registry
            .register(Arc::new(OntoPutObject {
                client: self.clients[1].clone(),
            }))
            .register(Arc::new(OntoExecuteAction {
                client: self.clients[1].clone(),
            }))
            .register(Arc::new(ReportCompute {
                client: self.clients[2].clone(),
            }));
        // U12 本体上下文：一站式域模型（onto/flow/report 三 client）。
        registry.register(Arc::new(EnterpriseContext {
            onto: self.clients[1].clone(),
            flow: self.clients[0].clone(),
            report: self.clients[2].clone(),
        }));
        // U14 业务联动流水线：一次审批跑单据→凭证→流程→报表。
        registry.register(Arc::new(EngineChain {
            onto: self.clients[1].clone(),
            flow: self.clients[0].clone(),
            report: self.clients[2].clone(),
        }));
    }

    /// 并发探测三连接器健康，返回卡片列表（描述 + live 状态）。
    /// 用 `tokio::join!` 真并发——三个离线探测各自超时不叠加（否则 3×timeout）。
    pub async fn probe_all(&self) -> Vec<ConnectorCard> {
        let (s0, s1, s2) = tokio::join!(
            probe(&self.clients[0]),
            probe(&self.clients[1]),
            probe(&self.clients[2]),
        );
        self.descriptors
            .iter()
            .cloned()
            .zip([s0, s1, s2])
            .map(|(descriptor, status)| ConnectorCard { descriptor, status })
            .collect()
    }
}
