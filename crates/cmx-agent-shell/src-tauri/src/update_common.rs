//! 更新胶水层（方案 C6）：前端统一入口 `check_update` / `download_and_install` / `get_app_version`。
//!
//! 壳内薄胶水，零业务逻辑（不许下沉进 cmx-agent-app）。平台分叉约定（方案 §5.1）：
//! Win/mac 全走 tauri-plugin-updater（本文件）；Linux 将来加 `#[cfg(target_os = "linux")]`
//! 自研分支（update_linux.rs），分叉只集中在下面两个命令里，禁止散落。
//!
//! 关键认知（Tauri updater 固定用法）：
//! - `Update` 对象不可序列化，必须进程内传递——`check_update` 把句柄存全局槽，
//!   `download_and_install` 从槽取出；前端「持有版本号再来装」走不通。
//! - macOS 的 `install()` 解压 .app.tar.gz 覆盖当前 .app 后**不会自动重启**，须显式 `restart()`。
//! - 进度经 Tauri 事件 `update_progress` 推前端（标题栏按钮内联百分比，不做弹窗，方案 §4.5）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

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

/// 检查更新：返回与 `dispatch_json` 同形的统一信封 JSON 串（前端 call 风格解析）。
///
/// 成功：`{ok:true, data:{update_available, version?, notes?, force}}`
/// 失败（源不可达/网络断）：`{ok:false,...}`——前端 toast，绝不影响主功能（fail-closed）。
/// 有更新时把 `Update` 句柄存 [`PENDING_UPDATE`] 槽，供 [`download_and_install`] 取用。
#[tauri::command]
pub async fn check_update(app: AppHandle) -> Result<String, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater.check().await.map_err(|e| e.to_string())?;
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
        r.map_err(|e| e.to_string())?;
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
