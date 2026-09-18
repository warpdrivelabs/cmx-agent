//! 桌面智能体工作空间注册表。
//!
//! 设计与 codex / opencode 的“workspace + task”体验对齐：
//! - 托管空间固定落在统一数据根 `<data>/workspaces/<id>`，应用负责创建与发现；
//! - 本地空间只保存用户显式选择的绝对目录，不改写、不搬迁；
//! - `None` 表示“不使用工作空间”，会话退化为普通问答/任务，不提供文件根。

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use walkdir::WalkDir;

use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorkspaceKind {
    Managed,
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(flatten)]
    pub kind: WorkspaceKind,
    pub created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    pub last_selected_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkspaceFile {
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub file_type: String,
}

#[derive(Debug, Default)]
struct WorkspaceState {
    items: Vec<WorkspaceEntry>,
    current: Option<String>,
}

#[derive(Debug)]
pub struct WorkspaceRegistry {
    path: PathBuf,
    state: RwLock<WorkspaceState>,
}

impl WorkspaceRegistry {
    pub fn load_or_init(data_dir: &Path, fallback_root: Option<&Path>) -> AppResult<Self> {
        let path = data_dir.join("workspaces.json");
        migrate_legacy_default(data_dir)?;
        let mut items = if path.exists() {
            let raw = std::fs::read_to_string(&path)?;
            match serde_json::from_str::<Vec<WorkspaceEntry>>(&raw) {
                Ok(items) => items,
                // 损坏不再 Err 砖死启动（旧实现壳层 expect → 桌面应用永久打不开）：
                // 备份现场后从零重建，默认空间由下方 ensure_default_workspace 恢复。
                Err(e) => {
                    let bak = path.with_extension("json.corrupt");
                    let _ = std::fs::rename(&path, &bak);
                    eprintln!(
                        "[workspace] workspaces.json 损坏（已备份到 {}），从零重建：{e}",
                        bak.display()
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        ensure_default_workspace(data_dir, &mut items)?;

        if items.is_empty() {
            let root = fallback_root
                .map(Path::to_path_buf)
                .unwrap_or_else(|| data_dir.join("workspaces").join("default"));
            std::fs::create_dir_all(&root)?;
            items.push(WorkspaceEntry {
                id: "default".to_string(),
                name: "默认工作空间".to_string(),
                path: root,
                kind: WorkspaceKind::Managed,
                created_at: chrono::Utc::now(),
                last_selected_at: Some(chrono::Utc::now()),
            });
        }

        let current = items
            .iter()
            .max_by_key(|w| w.last_selected_at.unwrap_or(w.created_at))
            .map(|w| w.id.clone());
        let registry = Self {
            path,
            state: RwLock::new(WorkspaceState { items, current }),
        };
        registry.persist()?;
        Ok(registry)
    }

    pub fn list(&self) -> AppResult<serde_json::Value> {
        let state = self.state.read().expect("workspace state");
        let current = state
            .current
            .as_ref()
            .and_then(|id| state.items.iter().find(|w| w.id == *id));
        Ok(serde_json::json!({
            "workspaces": state.items,
            "current": current,
        }))
    }

    pub fn select(&self, id: Option<&str>) -> AppResult<serde_json::Value> {
        // 「不使用工作空间」= 任务模式 = 内置 default 托管空间：任务也恒有文件根。
        let target = id.or(Some("default"));
        {
            let mut state = self.state.write().expect("workspace state");
            let id = target.expect("default fallback");
            if !state.items.iter().any(|w| w.id == id) {
                return Err(AppError::NotFound(format!("workspace '{id}'")));
            }
            if let Some(w) = state.items.iter_mut().find(|w| w.id == id) {
                w.last_selected_at = Some(chrono::Utc::now());
            }
            state.current = Some(id.to_string());
        }
        self.persist()?;
        self.list()
    }

    /// 把空间移出列表（不删磁盘目录）；移除的是当前空间时回落到内置 default（任务模式）。
    pub fn remove(&self, id: &str) -> AppResult<serde_json::Value> {
        if id == "default" {
            return Err(AppError::BadRequest("内置默认空间不能移除".into()));
        }
        {
            let mut state = self.state.write().expect("workspace state");
            let before = state.items.len();
            state.items.retain(|w| w.id != id);
            if state.items.len() == before {
                return Err(AppError::NotFound(format!("workspace '{id}'")));
            }
            if state.current.as_deref() == Some(id) {
                if let Some(d) = state.items.iter_mut().find(|w| w.id == "default") {
                    d.last_selected_at = Some(chrono::Utc::now());
                }
                state.current = Some("default".to_string());
            }
        }
        self.persist()?;
        self.list()
    }

    /// 用系统文件浏览器打开空间的本地文件夹（侧栏空间菜单「打开文件夹」）。
    pub fn open_folder(&self, id: &str) -> AppResult<serde_json::Value> {
        let path = {
            let state = self.state.read().expect("workspace state");
            state
                .items
                .iter()
                .find(|w| w.id == id)
                .map(|w| w.path.clone())
                .ok_or_else(|| AppError::NotFound(format!("workspace '{id}'")))?
        };
        open_in_file_manager(&path)?;
        Ok(serde_json::json!({ "opened": id }))
    }

    pub fn create_managed(&self, name: &str) -> AppResult<serde_json::Value> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("工作空间名称不能为空".into()));
        }
        let id = workspace_id("ws");
        let root = self.path.parent().ok_or_else(|| AppError::BadRequest("数据目录无效".into()))?
            .join("workspaces")
            .join(sanitize_id(&id));
        std::fs::create_dir_all(&root)?;
        let entry = WorkspaceEntry {
            id,
            name: name.to_string(),
            path: root,
            kind: WorkspaceKind::Managed,
            created_at: chrono::Utc::now(),
            last_selected_at: Some(chrono::Utc::now()),
        };
        {
            let mut state = self.state.write().expect("workspace state");
            state.items.push(entry);
            state.current = Some(state.items.last().expect("just pushed").id.clone());
        }
        self.persist()?;
        self.list()
    }

    pub fn add_local(&self, path: &str, name: Option<&str>) -> AppResult<serde_json::Value> {
        let raw = PathBuf::from(path.trim());
        if !raw.is_absolute() {
            return Err(AppError::BadRequest("本地工作空间必须是绝对路径".into()));
        }
        let path = raw.canonicalize().map_err(|e| {
            AppError::BadRequest(format!("本地目录不存在或无法访问：{e}"))
        })?;
        let path = displayable_path(path);
        if !path.is_dir() {
            return Err(AppError::BadRequest("路径不是文件夹".into()));
        }
        {
            let state = self.state.read().expect("workspace state");
            if state
                .items
                .iter()
                .any(|w| w.path == path || w.name == name.unwrap_or_default())
            {
                return Err(AppError::BadRequest("工作空间已存在".into()));
            }
        }
        let fallback = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("本地工作空间");
        let id = workspace_id("local");
        let entry = WorkspaceEntry {
            id,
            name: name
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| fallback.to_string()),
            path,
            kind: WorkspaceKind::Local,
            created_at: chrono::Utc::now(),
            last_selected_at: Some(chrono::Utc::now()),
        };
        {
            let mut state = self.state.write().expect("workspace state");
            state.items.push(entry);
            state.current = Some(state.items.last().expect("just pushed").id.clone());
        }
        self.persist()?;
        self.list()
    }

    pub fn current_context(&self) -> Option<(String, String, PathBuf)> {
        let state = self.state.read().expect("workspace state");
        state.current.as_ref().and_then(|id| {
            state
                .items
                .iter()
                .find(|w| w.id == *id)
                .map(|w| (w.id.clone(), w.name.clone(), w.path.clone()))
        })
    }

    pub fn current_path(&self) -> Option<PathBuf> {
        self.current_context().map(|(_, _, path)| path)
    }

    pub fn set_workspace_roots(&self, agent: &cmx_agent_core::Agent) -> AppResult<()> {
        let roots = self.current_path().into_iter().collect();
        let mut policy = agent.policy();
        policy.workspace_roots = roots;
        agent.set_policy(policy);
        Ok(())
    }

    /// 把 agent 文件根切到指定空间；空间不存在返回 `false`（调用方回落当前空间）。
    /// 回合开始前按「会话所属空间」定根用，保证 fs 工具与 @ 提示看同一个根。
    pub fn set_workspace_roots_for(&self, id: &str, agent: &cmx_agent_core::Agent) -> AppResult<bool> {
        let path = {
            let state = self.state.read().expect("workspace state");
            state.items.iter().find(|w| w.id == id).map(|w| w.path.clone())
        };
        match path {
            Some(path) => {
                let mut policy = agent.policy();
                policy.workspace_roots = vec![path];
                agent.set_policy(policy);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// 查指定空间的根路径（只读，不写共享 policy——回合根改由回合级快照传入内核 ToolCtx）。
    /// 空间不存在返回 `None`。
    pub fn roots_for(&self, id: &str) -> AppResult<Option<std::path::PathBuf>> {
        let state = self.state.read().expect("workspace state");
        Ok(state
            .items
            .iter()
            .find(|w| w.id == id)
            .map(|w| w.path.clone()))
    }

    /// 查指定空间的 `(id, name, root)` 上下文（红蓝审查 P2-4：系统提示词的空间段落按
    /// 会话所属空间生成时用，与 [`Self::current_context`] 同形）。空间不存在返回 `None`。
    pub fn context_for(&self, id: &str) -> Option<(String, String, PathBuf)> {
        let state = self.state.read().expect("workspace state");
        state
            .items
            .iter()
            .find(|w| w.id == id)
            .map(|w| (w.id.clone(), w.name.clone(), w.path.clone()))
    }

    pub fn search_files(&self, query: &str, limit: usize) -> AppResult<Vec<WorkspaceFile>> {
        let Some(root) = self.current_path() else {
            return Ok(Vec::new());
        };
        search_workspace_dir(&root, query, limit)
    }

    /// 在指定空间内检索文件。@ 提示按「会话所属空间」搜，而非全局当前空间；
    /// 空间已被移除时回落当前空间——与侧栏「空间已移除 → 任务组」的归组语义一致。
    pub fn search_files_in(&self, id: &str, query: &str, limit: usize) -> AppResult<Vec<WorkspaceFile>> {
        let root = {
            let state = self.state.read().expect("workspace state");
            state.items.iter().find(|w| w.id == id).map(|w| w.path.clone())
        };
        match root {
            Some(root) => search_workspace_dir(&root, query, limit),
            None => self.search_files(query, limit),
        }
    }

    fn persist(&self) -> AppResult<()> {
        let state = self.state.read().expect("workspace state");
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(&state.items)?;
        // 原子写（temp+rename）：直写被中断会留半份 JSON，下次启动解析失败。
        crate::store::write_atomic(&self.path, raw.as_bytes())?;
        Ok(())
    }
}

/// 默认托管空间统一放在 `<data>/workspaces/default`。
///
/// 旧版本曾把默认空间放在 `<data>/workspace`。这里只迁移注册表里的默认托管目录；
/// 本地工作空间是用户显式选择的目录，仍然不改写、不搬迁。
fn migrate_legacy_default(data_dir: &Path) -> AppResult<()> {
    let legacy_root = data_dir.join("workspace");
    let default_root = data_dir.join("workspaces").join("default");
    if !legacy_root.is_dir() {
        return Ok(());
    }
    if let Some(parent) = default_root.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if !default_root.exists() {
        std::fs::rename(&legacy_root, &default_root)?;
        return Ok(());
    }
    // 壳启动时可能已预创建空的 default 目录；只有空目录可以安全移除后承接旧目录。
    if std::fs::read_dir(&default_root)?.next().is_none() {
        std::fs::remove_dir(&default_root)?;
        std::fs::rename(&legacy_root, &default_root)?;
    }
    Ok(())
}

fn ensure_default_workspace(data_dir: &Path, items: &mut Vec<WorkspaceEntry>) -> AppResult<()> {
    if items.is_empty() {
        return Ok(());
    }

    let legacy_root = data_dir.join("workspace");
    let default_root = data_dir.join("workspaces").join("default");
    if let Some(default) = items
        .iter_mut()
        .find(|w| w.id == "default" && w.kind == WorkspaceKind::Managed)
    {
        if default.path == legacy_root {
            if legacy_root.exists() && !default_root.exists() {
                if let Some(parent) = default_root.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                // 同一数据目录内迁移；若旧目录被外部程序占用，启动会明确报错而不是静默丢目录。
                std::fs::rename(&legacy_root, &default_root)?;
            }
            default.path = default_root;
        }
        std::fs::create_dir_all(&default.path)?;
    } else {
        // 「任务模式 = default 空间」要求 default 恒存在：旧数据缺失时补建（不覆盖已有目录）。
        std::fs::create_dir_all(&default_root)?;
        items.push(WorkspaceEntry {
            id: "default".to_string(),
            name: "默认工作空间".to_string(),
            path: default_root,
            kind: WorkspaceKind::Managed,
            created_at: chrono::Utc::now(),
            last_selected_at: None,
        });
    }
    Ok(())
}

fn sanitize_id(raw: &str) -> String {
    raw.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// 工作空间 id：`<前缀>-<纳秒>-<进程内序号>`。仅用毫秒时间戳会在同一毫秒内连建两个
/// 空间时撞车（macOS 快盘实测：两次 add_local 同毫秒 → id 相同 → select/search_files_in
/// 全定位到第一个，界面上空间"切不动"）。纳秒 + 单调序号双保险，序号兜底时钟回拨。
fn workspace_id(prefix: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{prefix}-{nanos}-{n}")
}

/// Windows `canonicalize` 会返回 `\\?\` 物理路径；注册表/UI 展示与用户输入习惯保持一致。
fn displayable_path(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{unc}"));
    }
    if let Some(local) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(local);
    }
    path
}

/// 检索当前工作区文件。
///
/// 空查询只读一层目录，避免 `@` 刚输入就扫描整个仓库；有查询时做路径模糊匹配，
/// 并限制扫描数量 / 耗时，保证大仓库下输入框不卡顿。
fn search_workspace_dir(
    root: &Path,
    query: &str,
    limit: usize,
) -> AppResult<Vec<WorkspaceFile>> {
    let query = query.trim();
    if query.is_empty() {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(root)? {
            let entry = entry?;
            let path = entry.path();
            let is_dir = path.is_dir();
            files.push(WorkspaceFile {
                name: entry.file_name().to_string_lossy().to_string(),
                path: path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_default(),
                file_type: if is_dir { "dir" } else { "file" }.to_string(),
            });
            if files.len() >= limit {
                break;
            }
        }
        files.sort_by(|a, b| {
            a.file_type
                .cmp(&b.file_type)
                .then_with(|| a.path.to_lowercase().cmp(&b.path.to_lowercase()))
        });
        return Ok(files);
    }

    let query_lower = query.to_lowercase();
    let mut scored: Vec<(u32, WorkspaceFile)> = Vec::new();
    let started = std::time::Instant::now();
    let mut scanned = 0usize;
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.path() == root || !is_ignored_search_path(entry.path()))
        .filter_map(Result::ok)
    {
        if entry.depth() == 0 {
            continue;
        }
        scanned += 1;
        if scanned > MAX_SCAN_ENTRIES || started.elapsed() > MAX_SCAN_DURATION {
            break;
        }

        let path = entry.path();
        let is_dir = entry.file_type().is_dir();
        let Ok(relative_path) = path.strip_prefix(root) else {
            continue;
        };
        let relative = relative_path.to_string_lossy().replace('\\', "/");
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let relative_lower = relative.to_lowercase();
        let Some(base_score) =
            score_path_match(&query_lower, &file_name, &relative_lower)
        else {
            continue;
        };
        let length_penalty = (relative_lower.len().min(1000) / 20) as u32;
        scored.push((
            base_score.saturating_sub(length_penalty),
            WorkspaceFile {
                name: path
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| relative.clone()),
                path: relative,
                file_type: if is_dir { "dir" } else { "file" }.to_string(),
            },
        ));
    }

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.path.cmp(&b.1.path)));
    scored.truncate(limit);
    Ok(scored.into_iter().map(|(_, file)| file).collect())
}

const MAX_SCAN_ENTRIES: usize = 100_000;
const MAX_SCAN_DURATION: std::time::Duration = std::time::Duration::from_millis(250);

fn is_ignored_search_path(path: &Path) -> bool {
    path.components().any(|component| {
        matches!(
            component.as_os_str().to_string_lossy().as_ref(),
            ".git" | ".hg" | ".svn" | "node_modules" | "target" | "dist" | "build" | ".next"
                | ".venv" | "__pycache__"
        )
    })
}

fn score_path_match(query: &str, file_name: &str, relative: &str) -> Option<u32> {
    if relative == query {
        Some(1200)
    } else if file_name == query {
        Some(1000)
    } else if file_name.starts_with(query) {
        Some(900)
    } else if relative.starts_with(query) {
        Some(800)
    } else if file_name.contains(query) {
        Some(700)
    } else if relative.contains(query) {
        Some(600)
    } else if is_subsequence(query, relative) {
        Some(300)
    } else {
        None
    }
}

fn is_subsequence(query: &str, candidate: &str) -> bool {
    let mut chars = candidate.chars();
    query.chars().all(|expected| {
        chars
            .by_ref()
            .find(|actual| actual.to_lowercase().eq(expected.to_lowercase()))
            .is_some()
    })
}

// ── 原生文件夹选择器（协议命令 pick_local_directory）──
//
// Web 壳等没有 Tauri 对话框能力的壳，由本机后端进程调起系统选择器：rfd 进程内弹出
// 原生对话框（Windows IFileDialog / macOS NSOpenPanel / Linux xdg-desktop-portal），
// 三端统一、无外部脚本进程。rfd 的阻塞式 `pick_folder` 需要在非异步 worker 的线程上跑，
// 调用方（protocol / Tauri command）均经 spawn_blocking 进入；Windows/macOS 的 COM·STA、
// 主线程派发由 rfd 内部处理。返回本地绝对路径，后续统一交给 add_local_workspace 校验。
// Tauri 桌面壳有自己的同名 invoke 实现，同样落到本函数。

/// 调起操作系统原生的文件夹选择器，返回用户选择的绝对路径；用户取消返回 `None`。
pub fn native_pick_local_directory() -> Result<Option<String>, String> {
    let picked = rfd::FileDialog::new()
        .set_title("选择工作空间文件夹")
        .pick_folder();
    Ok(picked.map(|path| path.to_string_lossy().into_owned()))
}

/// 用系统文件浏览器打开目录：Windows explorer / macOS open / Linux xdg-open。
/// explorer 的退出码不反映结果（常非零），只看能否拉起进程。
fn open_in_file_manager(path: &Path) -> AppResult<()> {
    #[cfg(windows)]
    {
        std::process::Command::new("explorer.exe")
            .arg(path)
            .spawn()
            .map_err(|e| AppError::BadRequest(format!("无法打开文件浏览器：{e}")))?;
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(path)
            .spawn()
            .map_err(|e| AppError::BadRequest(format!("无法打开访达：{e}")))?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::process::Command::new("xdg-open")
            .arg(path)
            .spawn()
            .map_err(|e| AppError::BadRequest(format!("无法打开文件管理器：{e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unique_suffix() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    }

    #[test]
    fn managed_workspace_is_created_under_data_dir_and_selected() {
        let tmp = std::env::temp_dir().join(format!("cmx-ws-{}", unique_suffix()));
        let reg = WorkspaceRegistry::load_or_init(&tmp, None).unwrap();
        let created = reg.create_managed("测试").unwrap();
        let current = created["current"]["path"].as_str().unwrap();
        assert!(std::path::Path::new(current).starts_with(tmp.join("workspaces")));
        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn default_workspace_is_created_in_workspaces_default() {
        let tmp = std::env::temp_dir().join(format!("cmx-ws-default-{}", unique_suffix()));
        let reg = WorkspaceRegistry::load_or_init(&tmp, None).unwrap();
        assert_eq!(
            reg.current_path().unwrap(),
            tmp.join("workspaces").join("default")
        );
        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn legacy_default_workspace_moves_into_workspaces_default() {
        let tmp = std::env::temp_dir().join(format!("cmx-ws-migrate-{}", unique_suffix()));
        let legacy = tmp.join("workspace");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("readme.md"), "旧默认空间").unwrap();
        let entry = WorkspaceEntry {
            id: "default".to_string(),
            name: "默认工作空间".to_string(),
            path: legacy.clone(),
            kind: WorkspaceKind::Managed,
            created_at: chrono::Utc::now(),
            last_selected_at: Some(chrono::Utc::now()),
        };
        std::fs::create_dir_all(tmp.join("workspaces")).unwrap();
        // 模拟双壳启动时已预创建的新默认目录；迁移应安全承接旧目录内容。
        std::fs::create_dir_all(tmp.join("workspaces").join("default")).unwrap();
        std::fs::write(
            tmp.join("workspaces.json"),
            serde_json::to_string_pretty(&vec![entry]).unwrap(),
        )
        .unwrap();

        let reg = WorkspaceRegistry::load_or_init(&tmp, None).unwrap();
        let expected = tmp.join("workspaces").join("default");
        assert_eq!(reg.current_path().unwrap(), expected);
        assert!(expected.join("readme.md").is_file());
        assert!(!legacy.exists());
        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn search_files_in_scopes_to_requested_workspace_not_current() {
        let tmp = std::env::temp_dir().join(format!("cmx-ws-scope-{}", unique_suffix()));
        let reg = WorkspaceRegistry::load_or_init(&tmp, None).unwrap();
        let dir_a = tmp.join("a");
        let dir_b = tmp.join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_a.join("alpha.txt"), "a").unwrap();
        std::fs::write(dir_b.join("beta.txt"), "b").unwrap();
        let added_a = reg.add_local(&dir_a.to_string_lossy(), Some("A")).unwrap();
        let id_a = added_a["current"]["id"].as_str().unwrap().to_string();
        let added_b = reg.add_local(&dir_b.to_string_lossy(), Some("B")).unwrap();
        let id_b = added_b["current"]["id"].as_str().unwrap().to_string();
        // 当前空间已切到 B；按会话所属的 A 空间检索只应看到 A 的文件。
        reg.select(Some(&id_b)).unwrap();
        let files_a = reg.search_files_in(&id_a, "txt", 50).unwrap();
        assert!(files_a.iter().any(|f| f.name == "alpha.txt"));
        assert!(files_a.iter().all(|f| f.name != "beta.txt"));
        // 空间已被移除 → 回落当前空间（B）。
        let fallback = reg.search_files_in("nope", "beta", 50).unwrap();
        assert!(fallback.iter().any(|f| f.name == "beta.txt"));
        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn no_workspace_falls_back_to_default_task_mode() {
        let tmp = std::env::temp_dir().join(format!("cmx-ws-{}", unique_suffix()));
        let reg = WorkspaceRegistry::load_or_init(&tmp, None).unwrap();
        let picked = reg.select(None).unwrap();
        // 任务模式：「不使用工作空间」恒回落内置 default 托管空间，任务恒有文件根。
        assert_eq!(picked["current"]["id"].as_str(), Some("default"));
        assert_eq!(reg.current_path(), Some(tmp.join("workspaces").join("default")));
        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn local_workspace_is_searchable_without_relocation() {
        let tmp = std::env::temp_dir().join(format!("cmx-ws-local-{}", unique_suffix()));
        let local = tmp.join("project");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("readme.md"), "demo").unwrap();

        let reg = WorkspaceRegistry::load_or_init(&tmp, None).unwrap();
        reg.add_local(&local.to_string_lossy(), None).unwrap();
        // add_local 会 canonicalize 后经 displayable_path 归一化落库（macOS 下 /var → /private/var；
        // Windows 的 canonicalize 会带 \\?\ 逐字前缀，落库时剥掉），断言按落库同口径比较。
        assert_eq!(
            reg.current_path().unwrap(),
            displayable_path(local.canonicalize().unwrap())
        );
        assert!(reg
            .search_files("readme", 10)
            .unwrap()
            .iter()
            .any(|f| f.path == "readme.md"));

        std::fs::remove_dir_all(tmp).ok();
    }

    #[test]
    fn windows_canonical_prefix_is_displayed_normally() {
        assert_eq!(
            displayable_path(PathBuf::from(r"\\?\C:\project")),
            PathBuf::from(r"C:\project")
        );
        assert_eq!(
            displayable_path(PathBuf::from(r"\\?\UNC\server\share")),
            PathBuf::from(r"\\server\share")
        );
    }
}
