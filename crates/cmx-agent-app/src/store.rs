//! 会话落库（M1「会话日志落库」）。把 append-only 会话事件日志以 **JSONL** 存到磁盘：一行一事件，
//! 天然 append-only、可流式追加、可逐行回放——与内核日志同构。
//!
//! 布局：`<root>/sessions/<session_id>/log.jsonl`（事件流）+ `meta.json`（id/system/时间戳）。
//! 无数据库依赖，纯文件——桌面壳本地优先；后续可换 store-pg 实现同一 [`SessionStore`] trait。

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use cmx_agent_core::Session;
use cmx_agent_core::event::SessionEvent;

use crate::error::{AppError, AppResult};

/// 会话元数据（与事件流分离，便于列表页快速枚举而不必读全量日志）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionMeta {
    pub id: String,
    /// 会话标题（侧栏列表展示）。首条用户消息自动填充；空则前端回退用 id。
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub system: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub event_count: usize,
}

/// 会话存储抽象。M1 提供文件实现 [`FileSessionStore`]；后续可加 pg 实现。
pub trait SessionStore: Send + Sync {
    /// 追加若干新事件到某会话（不重写已有行）。
    fn append_events(&self, session_id: &str, events: &[SessionEvent]) -> AppResult<()>;
    /// 加载一个会话（重建其日志）。不存在返回 `AppError::NotFound`。
    fn load(&self, session_id: &str) -> AppResult<Session>;
    /// 列出所有会话元数据（按 updated_at 倒序）。
    fn list(&self) -> AppResult<Vec<SessionMeta>>;
    /// 写入/更新会话元数据。
    fn put_meta(&self, meta: &SessionMeta) -> AppResult<()>;
    /// 删除一个会话（连同其日志）。幂等：不存在也返回 Ok。
    fn delete(&self, session_id: &str) -> AppResult<()>;
}

/// 基于文件系统的会话存储（JSONL）。
pub struct FileSessionStore {
    root: PathBuf,
}

impl FileSessionStore {
    /// 以某根目录建库（自动创建 `<root>/sessions/`）。
    pub fn new(root: impl Into<PathBuf>) -> AppResult<Self> {
        let root = root.into();
        std::fs::create_dir_all(root.join("sessions"))?;
        Ok(Self { root })
    }

    fn session_dir(&self, id: &str) -> AppResult<PathBuf> {
        // 防路径注入：会话 id 不得含分隔符 / .. 。
        if id.is_empty()
            || id.contains('/')
            || id.contains('\\')
            || id.contains("..")
            || id.contains('\0')
        {
            return Err(AppError::BadRequest(format!("illegal session id '{id}'")));
        }
        Ok(self.root.join("sessions").join(id))
    }

    fn log_path(&self, id: &str) -> AppResult<PathBuf> {
        Ok(self.session_dir(id)?.join("log.jsonl"))
    }

    fn meta_path(&self, id: &str) -> AppResult<PathBuf> {
        Ok(self.session_dir(id)?.join("meta.json"))
    }
}

impl SessionStore for FileSessionStore {
    fn append_events(&self, session_id: &str, events: &[SessionEvent]) -> AppResult<()> {
        if events.is_empty() {
            return Ok(());
        }
        let dir = self.session_dir(session_id)?;
        std::fs::create_dir_all(&dir)?;
        let path = self.log_path(session_id)?;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;
        for ev in events {
            let line = serde_json::to_string(ev)?;
            writeln!(f, "{line}")?;
        }
        f.flush()?;
        Ok(())
    }

    fn load(&self, session_id: &str) -> AppResult<Session> {
        let path = self.log_path(session_id)?;
        if !path.exists() {
            return Err(AppError::NotFound(format!("session '{session_id}'")));
        }
        let meta = read_meta(&self.meta_path(session_id)?)?;
        let file = std::fs::File::open(&path)?;
        let reader = BufReader::new(file);
        let mut events = Vec::new();
        for (i, line) in reader.lines().enumerate() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let ev: SessionEvent = serde_json::from_str(&line).map_err(|e| {
                AppError::Corrupt(format!("session '{session_id}' line {}: {e}", i + 1))
            })?;
            events.push(ev);
        }
        let system = meta.and_then(|m| m.system);
        Ok(Session::from_events(session_id, system, events))
    }

    fn list(&self) -> AppResult<Vec<SessionMeta>> {
        let sessions = self.root.join("sessions");
        let mut out = Vec::new();
        if !sessions.exists() {
            return Ok(out);
        }
        for entry in std::fs::read_dir(&sessions)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().to_string();
            if let Some(meta) = read_meta(&self.meta_path(&id)?)? {
                out.push(meta);
            }
        }
        out.sort_by_key(|m| std::cmp::Reverse(m.updated_at));
        Ok(out)
    }

    fn put_meta(&self, meta: &SessionMeta) -> AppResult<()> {
        let dir = self.session_dir(&meta.id)?;
        std::fs::create_dir_all(&dir)?;
        let json = serde_json::to_string_pretty(meta)?;
        std::fs::write(self.meta_path(&meta.id)?, json)?;
        Ok(())
    }

    fn delete(&self, session_id: &str) -> AppResult<()> {
        let dir = self.session_dir(session_id)?;
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
        Ok(())
    }
}

fn read_meta(path: &Path) -> AppResult<Option<SessionMeta>> {
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(path)?;
    let meta: SessionMeta = serde_json::from_str(&s)
        .map_err(|e| AppError::Corrupt(format!("meta {}: {e}", path.display())))?;
    Ok(Some(meta))
}
