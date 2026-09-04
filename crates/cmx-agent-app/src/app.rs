//! 应用 façade [`AgentApp`]：桌面壳后端的单一入口。持有一个共享 `Agent` + 一个 `SessionStore`，
//! 对外提供"新建会话 / 发一条消息 / 列会话 / 读会话 / 删会话"等**用例级**操作，并在每个回合后
//! **增量落库**（只 append 新事件，不重写历史）。
//!
//! 这一层与前门无关（同核多壳）：CLI、Tauri invoke、Headless HTTP 都调它。见 [`crate::protocol`]。

use std::sync::Arc;

use cmx_agent_connectors::{ConnectorCard, ConnectorRegistry};
use cmx_agent_core::event::StopReason;
use cmx_agent_core::{Agent, Session};

use crate::error::{AppError, AppResult};
use crate::store::{SessionMeta, SessionStore};

/// 一个回合的对外结果（含新产生的事件条数，便于前门增量渲染）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct SendOutcome {
    pub session_id: String,
    pub turn: u64,
    pub reason: StopReason,
    pub steps: usize,
    pub final_text: Option<String>,
    /// 本回合新增事件（供前门直接展示/审计，无需重读全量日志）。
    pub new_events: Vec<cmx_agent_core::event::SessionEvent>,
}

/// 应用 façade。`Agent` 无内部可变状态（回合号从会话派生），故可 `Arc` 共享给多会话/多前门。
pub struct AgentApp {
    agent: Arc<Agent>,
    store: Arc<dyn SessionStore>,
    default_system: Option<String>,
    /// 连接器注册表（面板查询用）。None = 未启用连接器。
    connectors: Option<Arc<ConnectorRegistry>>,
}

impl AgentApp {
    pub fn new(agent: Arc<Agent>, store: Arc<dyn SessionStore>) -> Self {
        Self {
            agent,
            store,
            default_system: None,
            connectors: None,
        }
    }

    pub fn with_default_system(mut self, system: impl Into<String>) -> Self {
        self.default_system = Some(system.into());
        self
    }

    /// 注入连接器注册表（由 DesktopAppBuilder 调用）。
    pub fn with_connectors(mut self, connectors: Arc<ConnectorRegistry>) -> Self {
        self.connectors = Some(connectors);
        self
    }

    /// 列出连接器卡片（描述 + live 健康）。未启用连接器时返回空表。
    pub async fn list_connectors(&self) -> Vec<ConnectorCard> {
        match &self.connectors {
            Some(cr) => cr.probe_all().await,
            None => vec![],
        }
    }

    /// 新建会话并落元数据。返回其 id。
    pub fn create_session(&self, id: impl Into<String>) -> AppResult<String> {
        let id = id.into();
        if id.is_empty() {
            return Err(AppError::BadRequest("empty session id".into()));
        }
        let now = chrono::Utc::now();
        let meta = SessionMeta {
            id: id.clone(),
            title: None,
            system: self.default_system.clone(),
            created_at: now,
            updated_at: now,
            event_count: 0,
        };
        self.store.put_meta(&meta)?;
        Ok(id)
    }

    /// 向某会话发一条用户消息，跑一个回合，**增量落库**新事件，返回结果。
    /// 若会话已有持久化日志，先加载恢复（回合号、历史上下文都续上）。
    pub async fn send(&self, session_id: &str, user_input: &str) -> AppResult<SendOutcome> {
        // 加载已有会话；不存在则以默认 system 新建一个内存会话（并补落元数据）。
        let mut session = match self.store.load(session_id) {
            Ok(s) => s,
            Err(AppError::NotFound(_)) => {
                self.create_session(session_id)?;
                let mut s = Session::new(session_id);
                if let Some(sys) = &self.default_system {
                    s = s.with_system(sys.clone());
                }
                s
            }
            Err(e) => return Err(e),
        };

        let before = session.log.len();
        let outcome = self.agent.run_turn(&mut session, user_input).await?;

        // 只取本回合新增的事件，append 落库（不重写历史行）。
        let new_events: Vec<_> = session.log.events()[before..].to_vec();
        self.store.append_events(session_id, &new_events)?;

        // 刷新元数据（更新时间 + 事件总数；标题首条用户消息自动填充，保留原 created_at/title）。
        let now = chrono::Utc::now();
        let prev = self.store.list()?.into_iter().find(|m| m.id == session_id);
        let title = prev
            .as_ref()
            .and_then(|m| m.title.clone())
            .or_else(|| Some(make_title(user_input)));
        let meta = SessionMeta {
            id: session_id.to_string(),
            title,
            system: session.system().map(|s| s.to_string()),
            created_at: prev.map(|m| m.created_at).unwrap_or(now),
            updated_at: now,
            event_count: session.log.len(),
        };
        self.store.put_meta(&meta)?;

        Ok(SendOutcome {
            session_id: session_id.to_string(),
            turn: outcome.turn,
            reason: outcome.reason,
            steps: outcome.steps,
            final_text: outcome.final_text,
            new_events,
        })
    }

    /// 读取一个会话的全部事件（回放/展示）。
    pub fn get_events(
        &self,
        session_id: &str,
    ) -> AppResult<Vec<cmx_agent_core::event::SessionEvent>> {
        let session = self.store.load(session_id)?;
        Ok(session.log.events().to_vec())
    }

    /// 列出所有会话元数据。
    pub fn list_sessions(&self) -> AppResult<Vec<SessionMeta>> {
        self.store.list()
    }

    /// 删除一个会话。
    pub fn delete_session(&self, session_id: &str) -> AppResult<()> {
        self.store.delete(session_id)
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }
}

/// 从首条用户消息生成会话标题：取首行、截断到 ~24 字符。
fn make_title(user_input: &str) -> String {
    let first_line = user_input.trim().lines().next().unwrap_or("").trim();
    let title: String = first_line.chars().take(24).collect();
    if title.is_empty() {
        "新会话".to_string()
    } else if first_line.chars().count() > 24 {
        format!("{title}…")
    } else {
        title
    }
}
