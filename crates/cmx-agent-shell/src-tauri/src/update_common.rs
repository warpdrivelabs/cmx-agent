//! 更新胶水层（方案 C6）：前端统一入口 `check_update` / `download_and_install` / `get_app_version`。
//!
//! 壳内薄胶水，零业务逻辑（不许下沉进 cmx-agent-app）。平台分叉约定（方案 D1 终版）：
//! **三平台全走 tauri-plugin-updater 插件**——deb 更新通路是插件内置的（minisign 验签 →
//! `pkexec dpkg -i`），不存在也不需要 Linux 自研分支；分叉只集中在下面两个命令里，禁止散落。
//!
//! 关键认知（Tauri updater 固定用法）：
//! - `Update` 对象不可序列化，必须进程内传递——`check_update` 把句柄存全局槽，
//!   `download_and_install` 从槽取出；前端「持有版本号再来装」走不通。
//! - macOS 的 `install()` 解压 .app.tar.gz 覆盖当前 .app 后**不会自动重启**，须显式 `restart()`。
//! - 进度经 Tauri 事件 `update_progress` 推前端（标题栏按钮内联百分比，不做弹窗，方案 §4.5）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::{Update, UpdaterExt};

/// check() 拿到的 Update 句柄槽：重复 check 直接覆盖（Mutex 槽即可，无需状态机）。
static PENDING_UPDATE: Mutex<Option<Update>> = Mutex::new(None);

/// 防重入：download_and_install 进行中再点「更新」直接拒绝（前端按钮态 + 这里双保险）。
static INSTALLING: AtomicBool = AtomicBool::new(false);

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
    let body = serde_json::json!({
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
    let path = pending_path();
    let Ok(txt) = std::fs::read_to_string(&path) else { return };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
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

/// 检查更新：返回与 `dispatch_json` 同形的统一信封 JSON 串（前端 call 风格解析）。
///
/// 成功：`{ok:true, data:{update_available, version?, notes?, force}}`
/// 失败（源不可达/网络断）：`{ok:false,...}`——前端 toast，绝不影响主功能（fail-closed）。
/// 有更新时把 `Update` 句柄存 [`PENDING_UPDATE`] 槽，供 [`download_and_install`] 取用。
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<String, String> {
    // D5：endpoint 运行时注入（tauri.conf.json 不再持有 endpoints），URL = 构建期烧录的
    // portal_base() + 固定路径（方案 §7.1）。endpoints 收 Vec<Url> 且返回 Result。
    // {{target}}/{{current_version}} 由插件请求前替换（2.11.0 updater.rs :459-489，query 里的
    // 花括号不被 url crate 编码）：服务端据此对「该平台无产物」回 204（无更新），
    // 避免 platforms 缺键被插件判 TargetsNotFound 报错。
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
        // （updater 2.11.0 updater.rs :388/:698），见下方入槽前的放宽。
        .timeout(Duration::from_secs(10))
        // check() 挂在 Updater 上（builder 先 build；2.11.0 updater.rs :365/:432）。
        .build()
        .map_err(|e| e.to_string())?
        .check()
        .await
        .map_err(|e| e.to_string())?;
    // 入槽前把下载超时放宽为裕量（120s：正常包几秒下完；门户/网络挂死时 2 分钟内失败可重试，
    // 而不是永远"下载中"）。builder 的 timeout 会流入 Update 并同样作用于下载请求
    // （updater 2.11.0 updater.rs :388/:698）。
    if let Some(u) = update.as_mut() {
        u.timeout = Some(Duration::from_secs(120));
    }
    Ok(serde_json::json!({
        "ok": true,
        "data": match update {
            Some(u) => {
                let info = serde_json::json!({
                    "update_available": true,
                    "version": u.version,
                    "notes": u.body,
                    // force（强制更新）：P1 从 u.raw_json 取服务端 force 字段；P0 恒 false（数据通路已通）。
                    "force": false,
                });
                if let Ok(mut slot) = PENDING_UPDATE.lock() {
                    *slot = Some(u);
                }
                info
            }
            None => serde_json::json!({ "update_available": false }),
        }
    })
    .to_string())
}

/// 下载 + 验签 + 安装 + 重启（用 [`PENDING_UPDATE`] 槽里的句柄）。
///
/// 下载进度经事件 `update_progress`（`{received, total}`，累计字节）实时推前端；
/// 验签由插件内置完成（minisign，公钥在 tauri.conf.json），签名不符直接报错，绝不安装（fail-closed）。
/// 安装成功后 emit `update_installed` 并 `restart()`——restart 即刻替换进程，本命令不再返回前端。
#[tauri::command]
pub async fn download_and_install(app: AppHandle) -> Result<(), String> {
    if INSTALLING.swap(true, Ordering::SeqCst) {
        return Err("更新已在进行中".into());
    }
    let update = PENDING_UPDATE
        .lock()
        .ok()
        .and_then(|mut slot| slot.take())
        .ok_or_else(|| "请先检查更新".to_string());
    let result = async move {
        let update: Update = update?;
        // P1 上报：安装前落 pending 标记（from/to），新版本首次启动时上报 success（report_pending_update）。
        let from_v = app.package_info().version.to_string();
        let to_v = update.version.clone();
        let _ = std::fs::write(
            pending_path(),
            serde_json::json!({ "from_version": from_v, "to_version": to_v }).to_string(),
        );
        let app2 = app.clone();
        // 进度回调：on_chunk 给的是本次 chunk 增量（total 可能为 None），壳内累加成累计值再 emit。
        // emit 失败静默忽略（窗口可能已关，不当更新失败，对齐方案 L4 精神）。
        let mut received: usize = 0;
        let r = update
            .download_and_install(
                move |chunk, total| {
                    received += chunk;
                    let _ = app2.emit(
                        "update_progress",
                        serde_json::json!({ "received": received, "total": total }),
                    );
                },
                || { /* 下载完成：无需通知，安装紧随其后 */ },
            )
            .await;
        if let Err(e) = r {
            // P1：失败也上报（方案值域 failed_verify / failed_install），升级量只计成功、失败口径单列。
            let s = e.to_string();
            let kind = if s.contains("signature")
                || s.contains("minisign")
                || s.contains("公钥")
            {
                "failed_verify"
            } else {
                "failed_install"
            };
            post_report(&from_v, &to_v, kind).await;
            let _ = std::fs::remove_file(pending_path());
            return Err(s);
        }
        // install 完成（macOS：当前 .app 已被新版覆盖）。restart 会立刻杀掉 webview，
        // emit 的最后一帧大概率前端收不到，仅尽力而为。
        let _ = app.emit("update_installed", serde_json::json!({}));
        // macOS install() 不自动重启，显式重启进新版。restart() 返回 `!`（进程被替换，正常不返回）。
        app.restart()
    }
    .await;
    INSTALLING.store(false, Ordering::SeqCst);
    result
}
