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
    /// 创建会话时选中的工作空间；None = 不使用工作空间的普通任务。
    #[serde(default)]
    pub workspace_id: Option<String>,
}

/// 事件窗口：只含日志的一段（大会话分页 / 尾加载用，避免一次解析、传输、渲染全量事件）。
#[derive(Debug, Clone)]
pub struct EventWindow {
    /// 窗口内事件（时间正序）。
    pub events: Vec<SessionEvent>,
    /// 该会话事件总数。
    pub total: usize,
    /// 窗口首个事件在全量日志中的下标（0=已到最早；>0=上方还有更早的，可继续「加载更早」）。
    pub start: usize,
    /// 会话标题（顺带回传，免前端再单独查 `list`）。
    pub title: Option<String>,
}

/// 会话存储抽象。M1 提供文件实现 [`FileSessionStore`]；后续可加 pg 实现。
pub trait SessionStore: Send + Sync {
    /// 追加若干新事件到某会话（不重写已有行）。
    fn append_events(&self, session_id: &str, events: &[SessionEvent]) -> AppResult<()>;
    /// 加载一个会话（重建其日志）。不存在返回 `AppError::NotFound`。
    fn load(&self, session_id: &str) -> AppResult<Session>;
    /// 只加载事件日志的一个「窗口」（大会话分页 / 尾加载）。
    /// `limit=None`→全量；`Some(n)`→取窗口末 `n` 条。
    /// `before=None`→以日志末尾为右界（最新一屏）；`Some(k)`→以下标 `k`（不含）为右界，向前翻页。
    /// 默认实现走 `load` 再切片（正确但仍全量解析）；文件实现覆盖为「只解析窗口内的行」。
    fn load_events_window(
        &self,
        session_id: &str,
        limit: Option<usize>,
        before: Option<usize>,
    ) -> AppResult<EventWindow> {
        let session = self.load(session_id)?;
        let all = session.log.events();
        let total = all.len();
        let end = before.unwrap_or(total).min(total);
        let start = match limit {
            Some(n) => end.saturating_sub(n),
            None => 0,
        };
        Ok(EventWindow {
            events: all[start..end].to_vec(),
            total,
            start,
            title: None,
        })
    }
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

    fn load_events_window(
        &self,
        session_id: &str,
        limit: Option<usize>,
        before: Option<usize>,
    ) -> AppResult<EventWindow> {
        let path = self.log_path(session_id)?;
        if !path.exists() {
            return Err(AppError::NotFound(format!("session '{session_id}'")));
        }
        // 只读「行」（不解析），先拿总数与窗口边界，再**只解析窗口内的行**——
        // 这样大会话切换不必解析整段日志（JSON parse 是主要开销），是本次提速的关键。
        let file = std::fs::File::open(&path)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader
            .lines()
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|l| !l.trim().is_empty())
            .collect();
        let total = lines.len();
        let end = before.unwrap_or(total).min(total);
        let start = match limit {
            Some(n) => end.saturating_sub(n),
            None => 0,
        };
        let mut events = Vec::with_capacity(end.saturating_sub(start));
        for (i, line) in lines[start..end].iter().enumerate() {
            let ev: SessionEvent = serde_json::from_str(line).map_err(|e| {
                AppError::Corrupt(format!("session '{session_id}' line {}: {e}", start + i + 1))
            })?;
            events.push(ev);
        }
        let title = read_meta(&self.meta_path(session_id)?)?.and_then(|m| m.title);
        Ok(EventWindow {
            events,
            total,
            start,
            title,
        })
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
