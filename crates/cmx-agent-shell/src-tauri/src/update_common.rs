//! 更新胶水层（方案 C6）：前端统一入口 `check_update` / `download_and_install` / `get_app_version`。
//!
//! 壳内薄胶水，零业务逻辑（不许下沉进 cmx-agent-app）。平台分叉约定（方案 D1 终版）：
//! **三平台全走 tauri-plugin-updater 插件**——deb 更新通路是插件内置的（minisign 验签 →
//! `pkexec dpkg -i`），不存在也不需要 Linux 自研分支。
//!
//! 2026-09-12 P1+P2 重构（下载重试与断点续传方案 §2.1）：
//! - `download_and_install` **自持化**：自己 check 拿元数据，不再依赖进程内 Update 句柄槽
//!   （槽随重试语义废弃——句柄一次性，外环每轮要新句柄）。`check_update` 保留纯只读展示。
//! - **P1 外环重试**：check 三态判定（Err=本轮失败退避 / Ok(None)=撤回即停）+ 4 轮退避
//!   2s/5s/15s；最终失败分类上报一次（failed_download / failed_verify / failed_install）。
//! - **P2 断点下载器**：`.part` + `.part.meta` 元数据 sidecar，统一总是发 `Range: bytes=N-`
//!   （206 追加 / 200 降级全量 / 416=len==size 短路直接安装），256KB 粒度推进度。
//! - **回环流安装**：下载不走插件，落本地缓存后起 127.0.0.1 随机端口迷你 HTTP 服务，把
//!   latest.json 深拷贝（仅改平台项 url 指向本地）喂给插件——minisign 验签与三平台安装
//!   逻辑零改动。服务生命周期 = 单次安装，失败路径显式 shutdown 防 fd 泄漏。
//! - 缓存治理挂启动（与 report_pending_update 同点）：升级成功清空 updates/，顺带清 30 天残留。
//!
//! 关键认知（Tauri updater 固定用法）：
//! - macOS 的 `install()` 解压 .app.tar.gz 覆盖当前 .app 后**不会自动重启**，须显式 `restart()`。
//! - `Update.raw_json` 是 latest.json **完整响应体**（updater 2.11.0 updater.rs:538
//!   `raw_json = Some(update_response.clone())`），平台项在 `["platforms"][键]` 下——回环
//!   latest 直接深拷贝它、仅改平台项 url 即可，键名原样保留（插件按
//!   `{os}-{arch}-{installer}`→`{os}-{arch}` 回退键序自会命中）。
//! - 进度经 Tauri 事件 `update_progress` `{received, total, attempt}` 推前端（received 已含
//!   续传偏移）；撤回经 `update_withdrawn` 事件（前端清按钮与 agreed 标记）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// 防重入：download_and_install 进行中再点「更新」/自动再试直接拒绝
/// （前端按钮态 + 这里双保险，验收 T13）。
static INSTALLING: AtomicBool = AtomicBool::new(false);

/// 检查请求超时（builder timeout 会流入 Update 同样作用于下载，见 check_once 放宽）。
const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// 单轮下载超时（reqwest Client 总时长：从连接到 body 读完；与旧插件裕量一致）。
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// 回环流 check + 本机下载超时（127.0.0.1 秒回，给 32MB 级裕量即可）。
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(60);
/// 外环轮数与退避（2s/5s/15s）：最坏 4×(check 10s + 下载 120s) + 22s ≈ 9.3 分钟。
const MAX_ATTEMPTS: u32 = 4;
const BACKOFF_MS: [u64; 3] = [2_000, 5_000, 15_000];
/// 进度推送粒度：每累计 256KB emit 一帧（避免高频事件打满 IPC）。
const PROGRESS_STEP: u64 = 256 * 1024;

/// 当前应用版本：前端 `APP_VERSION` 注入源（关于菜单 / 检查更新提示共用）。
/// 两壳（Web/Tauri）都可用——Web 壳前端保持静态常量，仅 Tauri 壳 invoke 覆盖。
#[tauri::command]
pub fn get_app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

// ── 更新结果上报（方案 §7.3 P1）：门户维护页「升级量 / 累计升级」的数据源 ──
// download_and_install 安装前落 pending 标记；新版本首次启动时 POST /agent-updates/report
// （免鉴权边缘通道，匿名可报）。上报未受理则标记保留、下次启动重试——升级量以门户受理为准，不重复计数。

fn pending_path() -> std::path::PathBuf {
    cmx_agent_app::shared_data_dir().join("update-pending.json")
}

async fn post_report(from: &str, to: &str, result: &str) -> bool {
    let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
    let body = json!({
        "from_version": from,
        "to_version": to,
        "target": format!("{os}-{arch}"),
        "arch": arch,
        "result": result,
    });
    let url = format!("{}/agent-updates/report", crate::portal_base());
    let accepted = match reqwest::Client::new()
        .post(url)
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
    {
        Ok(r) => r.status().is_success(),
        Err(e) => {
            tracing::warn!("更新结果上报请求失败（{result} {from}->{to}）：{e}");
            false
        }
    };
    if !accepted {
        tracing::warn!("更新结果上报未受理（{result} {from}->{to}）");
    }
    accepted
}

/// 启动时消费 pending 标记（main.rs setup 里 spawn）：当前版本 == to_version → 上报
/// success 并清标记；版本不符（用户手动装了别的版本）静默清；上报未受理保留待下次重试。
pub async fn report_pending_update(app: AppHandle) {
    // 缓存治理同点先跑（§2.3）：升级成功清空 updates/ 整目录；否则只清 30 天残留。
    // 必须在消费 pending 标记**之前**——判定"升级成功"要读标记里的 to_version。
    cleanup_updates_cache(&app);
    let path = pending_path();
    let Ok(txt) = std::fs::read_to_string(&path) else { return };
    let Ok(v) = serde_json::from_str::<Value>(&txt) else {
        let _ = std::fs::remove_file(&path);
        return;
    };
    let from = v["from_version"].as_str().unwrap_or("").to_string();
    let to = v["to_version"].as_str().unwrap_or("").to_string();
    let cur = app.package_info().version.to_string();
    if to.is_empty() || to != cur {
        // 版本不符（用户手动装了别的版本）：静默清，不计数。
        let _ = std::fs::remove_file(&path);
    } else if post_report(&from, &to, "success").await {
        // 升级成功且门户受理：清标记；未受理则保留，下次启动重试（升级量以受理为准，不重复计数）。
        let _ = std::fs::remove_file(&path);
    }
}

// ── 断点缓存（§2.1-②）：目录 / 文件名 / sidecar ──

/// 断点缓存目录：`<shared_data_dir>/updates/`（升级成功启动时整目录清空，§2.3）。
fn updates_dir() -> PathBuf {
    cmx_agent_app::shared_data_dir().join("updates")
}

/// 文件名成分白名单过滤（R5）：只留 `[A-Za-z0-9._-]`，其余替换 `_`
/// （version/platform 来自服务端 JSON——防 `../` 路径穿越，验收 T15）。
fn sanitize_component(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 断点下载器的目标元数据（来自插件 check 结果 + latest.json 平台项）。
struct ArtifactMeta {
    version: String,
    url: String,
    /// 平台项完整字节数（服务端登记时自动补；旧门户无此字段 → None，跳过长度校验）。
    size: Option<u64>,
}

impl ArtifactMeta {
    /// 缓存 stem：`{version}-{os}-{arch}`。用本机平台常量而非服务端平台键——
    /// 同机器只有一个平台语境，免得从 platforms 反查匹配键（R5 sanitize 已含）。
    fn stem(&self) -> String {
        let plat = format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH);
        format!("{}-{}", sanitize_component(&self.version), sanitize_component(&plat))
    }
}

/// 删除某版本的断点缓存三件套（.part / .part.meta / .bin）——验签失败 fail-closed（§2.1-③）。
fn cleanup_version_cache(meta: &ArtifactMeta) {
    let dir = updates_dir();
    let stem = meta.stem();
    for f in [format!("{stem}.part"), format!("{stem}.part.meta"), format!("{stem}.bin")] {
        let _ = std::fs::remove_file(dir.join(f));
    }
}

/// 启动缓存治理（§2.3，与 report_pending_update 同点）：
/// 当前版本 == pending.to_version（升级成功路径）→ 清空 updates/ 整目录；
/// 否则只清 30 天前残留（防磁盘堆积）。
fn cleanup_updates_cache(app: &AppHandle) {
    let dir = updates_dir();
    let upgraded = std::fs::read_to_string(pending_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["to_version"].as_str().map(str::to_string))
        .is_some_and(|to| to == app.package_info().version.to_string());
    if upgraded {
        let _ = std::fs::remove_dir_all(&dir);
        return;
    }
    let cutoff = std::time::SystemTime::now() - Duration::from_secs(30 * 24 * 3600);
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let stale = e
                .metadata()
                .and_then(|m| m.modified())
                .map(|m| m < cutoff)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

// ── check（①）：插件薄封装，两命令共用 ──

/// 跑一次更新检查（10s 超时）。`Ok(None)` = 无更新（204）；`Err` = 网络/服务不可达
/// （**≠ 撤回**，R2：外环对 Err 退避重试，只有 Ok(None) 才是撤回即停）。
/// 返回的 Update 携带 raw_json（完整响应体）与解析好的 url/version；timeout 已放宽为下载裕量。
async fn check_once(app: &AppHandle) -> Result<Option<Update>, String> {
    // D5：endpoint 运行时注入，URL = 构建期烧录的 portal_base() + 固定路径。
    // {{target}}/{{current_version}} 由插件请求前替换（2.11.0 updater.rs :459-489）：
    // 服务端据此对「该平台无产物」回 204，避免 platforms 缺键被插件判 TargetsNotFound。
    let endpoint = url::Url::parse(&format!(
        "{}/agent-updates/latest.json?target={{{{target}}}}&current_version={{{{current_version}}}}",
        crate::portal_base()
    ))
    .map_err(|e| e.to_string())?;
    let mut update = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| e.to_string())?
        // 10s 只对检查请求安全：builder 的 timeout 会流入 Update 并同样作用于下载请求
        // （updater 2.11.0 updater.rs :388/:698），见下方放宽。
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    if let Some(u) = update.as_mut() {
        u.timeout = Some(DOWNLOAD_TIMEOUT);
    }
    Ok(update)
}

/// 从 raw_json 的 platforms 里找 url 与插件选中项相同的平台项，取其 `size`
/// （服务端登记时自动补，方案 §2.4-R11）。旧门户无 size → None（客户端跳过长度校验）。
fn find_platform_size(raw: &Value, target: &url::Url) -> Option<u64> {
    let map = raw.get("platforms")?.as_object()?;
    for entry in map.values() {
        let u = entry.get("url").and_then(Value::as_str).unwrap_or("");
        if u.is_empty() {
            continue;
        }
        match url::Url::parse(u) {
            Ok(parsed) if parsed == *target => {
                return entry.get("size").and_then(Value::as_u64);
            }
            _ => continue,
        }
    }
    None
}

// ── P2 断点下载器（②，内环单次尝试；重试统一在外环，R3）──

/// 断点下载到缓存：`.part` 追加续传，完成后 rename 定稿 `.bin`，返回 .bin 路径。
///
/// - R6：`.part.meta` sidecar（url/size/version）与 .part 同生共死；续传前核对，
///   不匹配即弃全部缓存（防同版本重登记换产物续传拼出混合文件）。
/// - R12：**统一总是发 `Range: bytes=<len>-`**（len=0 即 bytes=0-）——206/200 一轮即知
///   服务端能力，探测状态机消失；416 = .part 已完整（len==size）→ 定稿直装（T8）。
/// - 进度：每 256KB emit `update_progress {received, total, attempt}`（received 含续传偏移）。
async fn download_to_cache(
    app: &AppHandle,
    client: &reqwest::Client,
    meta: &ArtifactMeta,
    attempt: u32,
) -> Result<PathBuf, String> {
    let dir = updates_dir();
    tokio::fs::create_dir_all(&dir).await.map_err(|e| format!("创建更新缓存目录失败：{e}"))?;
    let stem = meta.stem();
    let part = dir.join(format!("{stem}.part"));
    let part_meta = dir.join(format!("{stem}.part.meta"));
    let bin = dir.join(format!("{stem}.bin"));

    // R6：sidecar 核对；不符 = 产物已换 → 弃全部缓存。
    let mut cache_ok = false;
    if let Ok(txt) = tokio::fs::read_to_string(&part_meta).await {
        if let Ok(obj) = serde_json::from_str::<Value>(&txt) {
            cache_ok = obj["url"].as_str() == Some(meta.url.as_str())
                && obj["size"].as_u64() == meta.size
                && obj["version"].as_str() == Some(meta.version.as_str());
        }
    }
    if !cache_ok {
        let _ = tokio::fs::remove_file(&part).await;
        let _ = tokio::fs::remove_file(&bin).await;
        let _ = tokio::fs::remove_file(&part_meta).await;
    } else if tokio::fs::metadata(&bin).await.is_ok() {
        // 上轮已下完（.bin 定稿）但未装完（如回环 bind 失败/安装报错）：直接复用，跳过下载。
        return Ok(bin);
    }
    let base_len = tokio::fs::metadata(&part).await.map(|m| m.len()).unwrap_or(0);

    // R7 磁盘预检：可用空间 < size×1.1 → 直接终止（重试救不了磁盘满，不上报，T11）。
    if let Some(sz) = meta.size {
        if let Some(free) = disk_free_bytes(&dir) {
            if free < sz.saturating_add(sz / 10) {
                return Err("磁盘空间不足，无法下载更新（清理磁盘后重试）".into());
            }
        }
    }

    // R12：统一发 Range（base_len=0 即 bytes=0-）。
    let mut resp = client
        .get(&meta.url)
        .header(reqwest::header::RANGE, format!("bytes={base_len}-"))
        .send()
        .await
        .map_err(|e| format!("连接失败：{e}"))?;
    let status = resp.status();

    // total：206 从 Content-Range 取完整总长；200 降级时 Content-Length 即全量；416 特判。
    let total: Option<u64> = match status {
        reqwest::StatusCode::PARTIAL_CONTENT => resp
            .headers()
            .get(reqwest::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.rsplit('/').next())
            .and_then(|t| t.trim().parse().ok()),
        reqwest::StatusCode::OK => resp.content_length(),
        reqwest::StatusCode::RANGE_NOT_SATISFIABLE => {
            // 416：.part 已完整（len==size）→ 定稿直装；len>size 或无 size → 弃缓存下轮重下。
            if base_len > 0 && meta.size == Some(base_len) {
                finalize_part(&part, &bin).await?;
                return Ok(bin);
            }
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!("缓存校验失败（416：part={base_len}B size={:?}）", meta.size));
        }
        other => return Err(format!("下载响应异常：HTTP {other}")),
    };

    // R6：sidecar 与 .part 同生共死（从现在起写盘）。
    let sidecar = json!({ "url": meta.url, "size": meta.size, "version": meta.version });
    tokio::fs::write(&part_meta, sidecar.to_string())
        .await
        .map_err(|e| format!("写缓存元数据失败：{e}"))?;

    // 206 追加续传；200 降级（服务端不支持 Range）清 .part 当全量重写（= P1 行为）。
    let resuming = status == reqwest::StatusCode::PARTIAL_CONTENT && base_len > 0;
    let mut file = if resuming {
        tokio::fs::OpenOptions::new().append(true).open(&part).await
    } else {
        tokio::fs::File::create(&part).await
    }
    .map_err(|e| format!("打开缓存文件失败：{e}"))?;

    let mut received: u64 = if resuming { base_len } else { 0 };
    let mut since_emit: u64 = 0;
    while let Some(chunk) = resp.chunk().await.map_err(|e| format!("下载中断：{e}"))? {
        file.write_all(&chunk).await.map_err(|e| format!("写入缓存失败：{e}"))?;
        received += chunk.len() as u64;
        since_emit += chunk.len() as u64;
        if since_emit >= PROGRESS_STEP {
            since_emit = 0;
            let _ = app.emit(
                "update_progress",
                json!({ "received": received, "total": total, "attempt": attempt }),
            );
        }
    }
    file.sync_all().await.map_err(|e| format!("刷盘失败：{e}"))?;
    drop(file);

    // 完整性：len != size（有 size 时）→ 删 .part 本轮失败；无 size（旧门户）跳过校验，验签兜底。
    if let Some(sz) = total {
        if received != sz {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(format!("下载不完整（{received}/{sz} 字节）"));
        }
    }
    finalize_part(&part, &bin).await?;
    let _ = app.emit(
        "update_progress",
        json!({ "received": received, "total": total, "attempt": attempt }),
    );
    Ok(bin)
}

/// `.part` 定稿：rename → `.bin`（Windows 杀软短暂锁文件时重试 3×100ms，R10；
/// 仍败 = 本轮失败，.part 完整保留，下轮 416 短路直接安装）。
async fn finalize_part(part: &Path, bin: &Path) -> Result<(), String> {
    let mut last = None;
    for i in 0..3u32 {
        match tokio::fs::rename(part, bin).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                if i == 2 {
                    last = Some(e);
                } else {
                    tracing::warn!("缓存定稿 rename 重试（{}/3）：{e}", i + 1);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
    Err(format!("缓存定稿失败：{}", last.map(|e| e.to_string()).unwrap_or_default()))
}

/// 目录所在盘可用空间（R7 磁盘预检）。Windows 走 kernel32 零依赖 FFI；
/// 其他平台暂返回 None（跳过预检，磁盘满由写入失败兜底，failed_download 口径）。
#[cfg(target_os = "windows")]
fn disk_free_bytes(dir: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            lp_dir: *const u16,
            lp_free: *mut u64,
            lp_total: *mut u64,
            lp_total_free: *mut u64,
        ) -> i32;
    }
    let wide: Vec<u16> = dir.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
    let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(wide.as_ptr(), &mut avail, &mut total, &mut free) != 0 }
        .then_some(avail)
}

#[cfg(not(target_os = "windows"))]
fn disk_free_bytes(_dir: &Path) -> Option<u64> {
    None
}

// ── P2 回环流安装（④）：本地文件喂插件验签 + 安装 ──

/// 回环流迷你 HTTP 服务：accept 循环处理 GET（`/latest.json` + `/artifact`），
/// shutdown 信号随时退出。单连接串行处理（插件请求天然串行）；Connection: close 禁复用。
async fn serve_loopback(
    listener: tokio::net::TcpListener,
    bin: PathBuf,
    latest_body: Vec<u8>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        let stream = tokio::select! {
            _ = shutdown_rx.changed() => return,
            r = listener.accept() => match r {
                Ok((s, _)) => s,
                Err(_) => return,
            },
        };
        if let Err(e) = handle_loopback_conn(stream, &bin, &latest_body).await {
            tracing::warn!("回环流请求处理失败：{e}");
        }
    }
}

async fn handle_loopback_conn(
    mut stream: tokio::net::TcpStream,
    bin: &Path,
    latest_body: &[u8],
) -> Result<(), String> {
    // 读请求头（最多 8KB，遇 \r\n\r\n 即止）。
    let mut buf = vec![0u8; 8192];
    let mut filled = 0usize;
    loop {
        let n = stream.read(&mut buf[filled..]).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        filled += n;
        if filled == buf.len() || buf[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
    }
    let req = String::from_utf8_lossy(&buf[..filled]);
    let mut parts = req.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let is_head = method == "HEAD";

    let (status, ctype, body): (&str, &str, Vec<u8>) = if path.starts_with("/latest.json") {
        ("200 OK", "application/json", latest_body.to_vec())
    } else if path.starts_with("/artifact") {
        match tokio::fs::read(bin).await {
            Ok(data) => ("200 OK", "application/octet-stream", data),
            Err(e) => ("500 Internal Server Error", "text/plain", e.to_string().into_bytes()),
        }
    } else {
        ("404 Not Found", "text/plain", b"not found".to_vec())
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await.map_err(|e| e.to_string())?;
    if !is_head {
        stream.write_all(&body).await.map_err(|e| e.to_string())?;
    }
    stream.shutdown().await.map_err(|e| e.to_string())?;
    Ok(())
}

/// 回环流安装（§2.1-④）：起 127.0.0.1 随机端口迷你服务，latest.json 深拷贝（仅改平台项
/// url 指向本地）喂插件 check → download（本机秒下）→ minisign 验签 → install。
///
/// install 成功 → emit `update_installed` → `restart()`（即刻替换进程，本函数不返回）；
/// 任何失败 → 显式停服后 Err（fd 泄漏防护）。失败路径的缓存处置（验签失败删缓存）由外环做。
async fn install_via_loopback(
    app: &AppHandle,
    bin: PathBuf,
    raw_json: &Value,
    from_v: &str,
    to_v: &str,
) -> Result<(), String> {
    // ④a bind 随机端口（T10：重试 3 次，仍败 = 本轮失败走退避）。
    let mut listener = None;
    for _ in 0..3 {
        if let Ok(l) = tokio::net::TcpListener::bind("127.0.0.1:0").await {
            listener = Some(l);
            break;
        }
    }
    let Some(listener) = listener else {
        return Err("本机回环端口绑定失败（已重试 3 次）".into());
    };
    let port = listener.local_addr().map_err(|e| e.to_string())?.port();

    // ④c 回环 latest.json = raw_json 完整响应体深拷贝（updater.rs:538 语义），
    // 仅改平台项 url；version/notes/signature/platforms 键原样保留。
    let mut latest = raw_json.clone();
    if let Some(map) = latest.get_mut("platforms").and_then(Value::as_object_mut) {
        for (_key, entry) in map.iter_mut() {
            if entry.get("url").and_then(Value::as_str).is_some() {
                entry["url"] = json!(format!("http://127.0.0.1:{port}/artifact"));
            }
        }
    }
    let latest_body = latest.to_string().into_bytes();

    // ④b 服务 task：bind 已完成（内核 backlog 已收连接），spawn 只是 accept 循环，无就绪竞态。
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    tauri::async_runtime::spawn(serve_loopback(listener, bin, latest_body, shutdown_rx));

    // ④e install 前落 pending 上报标记（R15：install 可能中断进程，标记先于风险动作落盘；
    // 下载失败不写 pending——failed_download 即时报无需补报）。
    let _ = std::fs::write(
        pending_path(),
        json!({ "from_version": from_v, "to_version": to_v }).to_string(),
    );

    let result = async {
        let endpoint = url::Url::parse(&format!("http://127.0.0.1:{port}/latest.json"))
            .map_err(|e| e.to_string())?;
        let update = app
            .updater_builder()
            .endpoints(vec![endpoint])
            .map_err(|e| e.to_string())?
            .timeout(LOOPBACK_TIMEOUT)
            .build()
            .map_err(|e| e.to_string())?
            .check()
            .await
            .map_err(|e| format!("回环 check 失败：{e}"))?;
        let update =
            update.ok_or("回环 check 意外返回无更新（自造 latest 不可能 204）".to_string())?;
        // 本机下载秒级：进度只尽力 emit（received 不带 attempt——前端缺省视为手动轮）。
        let app2 = app.clone();
        let mut received: usize = 0;
        update
            .download_and_install(
                move |chunk, total| {
                    received += chunk;
                    let _ = app2.emit(
                        "update_progress",
                        json!({ "received": received, "total": total }),
                    );
                },
                || { /* 下载完成：无需通知，安装紧随其后 */ },
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok::<(), String>(())
    }
    .await;
    // 失败路径显式停服（fd 泄漏防护，方案 ④）；成功路径 install 已完成，restart 前顺手停。
    let _ = shutdown_tx.send(true);
    result?;

    let _ = app.emit("update_installed", json!({}));
    // macOS install() 不自动重启，显式重启进新版。restart 即刻替换进程，正常不返回（`!`）。
    app.restart()
}

// ── 前端命令入口 ──

/// 检查更新：返回与 `dispatch_json` 同形的统一信封 JSON 串（前端 call 风格解析）。
///
/// 成功：`{ok:true, data:{update_available, version?, notes?, force}}`
/// 失败（源不可达/网络断）：`{ok:false,...}`——前端 toast，绝不影响主功能（fail-closed）。
/// **纯只读展示**（登录首查 / 主窗兜底 / 手动菜单查），与 `download_and_install` 不共享
/// 状态（R1 去槽化：下载命令自己 check 拿新句柄；两命令间重复请求是幂等只读，可接受）。
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<String, String> {
    let update = check_once(&app).await?;
    let data = match update {
        Some(u) => {
            // force 从 raw_json 取服务端字段（随版本走；缺省 false 兼容旧门户）。
            let force = u.raw_json.get("force").and_then(Value::as_bool).unwrap_or(false);
            json!({
                "update_available": true,
                "version": u.version,
                "notes": u.body,
                "force": force,
            })
        }
        None => json!({ "update_available": false }),
    };
    Ok(json!({ "ok": true, "data": data }).to_string())
}

/// 下载 + 验签 + 安装 + 重启（自持化，方案 §2.1：外环重试 + 断点下载 + 回环安装）。
///
/// 返回语义：
/// - `Ok(())` = 版本撤回（204）——安装成功路径不走这里（restart 即刻替换进程，前端收不到返回）；
/// - `Err(msg)` = 4 轮全部失败（前端弹重试窗；自动再试只 toast）。
#[tauri::command]
pub async fn download_and_install(app: AppHandle) -> Result<(), String> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("更新已在进行中".into());
    }
    let result = run_update_loop(&app).await;
    INSTALLING.store(false, Ordering::SeqCst);
    result
}

/// 外环（P1）：4 轮退避重试；每轮 = check 三态 → 断点下载 → 回环安装。
async fn run_update_loop(app: &AppHandle) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(DOWNLOAD_TIMEOUT)
        .build()
        .map_err(|e| format!("HTTP 客户端构建失败：{e}"))?;
    let from_v = app.package_info().version.to_string();
    let mut to_v = String::new();
    let mut last_kind = "failed_download";
    let mut last_err = String::new();

    for attempt in 1..=MAX_ATTEMPTS {
        if attempt > 1 {
            // 退避（2s/5s/15s）后进入下一轮：.part 保留，续传省流量。
            tokio::time::sleep(Duration::from_millis(BACKOFF_MS[(attempt - 2) as usize])).await;
        }
        // ① check 三态（R2）：Err = 本轮失败（断网/超时 ≠ 撤回）；Ok(None) = 撤回即停。
        let (update, raw) = match check_once(app).await {
            Ok(Some(u)) => {
                let raw = u.raw_json.clone();
                (u, raw)
            }
            Ok(None) => {
                let _ = app.emit("update_withdrawn", json!({}));
                return Ok(());
            }
            Err(e) => {
                last_kind = "failed_download";
                last_err = format!("检查更新失败：{e}");
                tracing::warn!("更新第 {attempt}/{MAX_ATTEMPTS} 轮失败：{last_err}");
                continue;
            }
        };
        to_v = update.version.clone();
        let size = find_platform_size(&raw, &update.download_url);
        let meta =
            ArtifactMeta { version: to_v.clone(), url: update.download_url.to_string(), size };

        // ② 断点下载（内环单次尝试，R3）。
        let bin = match download_to_cache(app, &client, &meta, attempt).await {
            Ok(b) => b,
            Err(e) => {
                // R7：磁盘满重试也救不了——终止且不上报（T11）。
                if e.contains("磁盘空间不足") {
                    return Err(e);
                }
                last_kind = "failed_download";
                last_err = format!("下载失败：{e}");
                tracing::warn!("更新第 {attempt}/{MAX_ATTEMPTS} 轮失败：{last_err}");
                continue;
            }
        };

        // ④ 回环流：本地文件喂插件验签 + 安装（成功即 restart，不返回）。
        match install_via_loopback(app, bin, &raw, &from_v, &to_v).await {
            Ok(()) => return Ok(()), // 仅 restart 前编译可达
            Err(e) => {
                let kind = if e.contains("signature")
                    || e.contains("minisign")
                    || e.contains("公钥")
                {
                    // 验签失败（.part 被篡改/混合文件）→ 删缓存下轮全量重下，fail-closed（T6）。
                    cleanup_version_cache(&meta);
                    "failed_verify"
                } else {
                    "failed_install"
                };
                last_kind = kind;
                last_err = format!("安装失败：{e}");
                tracing::warn!("更新第 {attempt}/{MAX_ATTEMPTS} 轮失败：{last_err}");
                continue;
            }
        }
    }
    // 4 轮全败：只上报一次（R4）；check 从未成功过（无已知目标版本）则无口径可报。
    if !to_v.is_empty() {
        post_report(&from_v, &to_v, last_kind).await;
    }
    Err(last_err)
}
