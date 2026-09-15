//! cmx-agent 原生 Tauri 桌面壳入口。
//!
//! 「同核多壳」：本文件把 Tauri 的 invoke 边界接到 cmx-agent-app 的 `dispatch_json`——无业务逻辑。
//! 业务全在已测的 cmx-agent-app（façade / JSONL 落库 / 前门协议 / 五层守卫 / 认证），与 Web 壳共用同一核。
//! 前端 `ui/index.html` 里 `call()` 检测 `window.__TAURI__` 走 invoke，否则回退 HTTP。
//!
//! 登录门（SPA 单窗口）：启动后只开一个 `main` 窗口，前端 js/login.js 通过 hash 路由
//! （#/login ↔ #/）切换登录/主视图，不再使用双窗口。`login` 命令仅做认证 dispatch。

// Windows 发布版按"窗口程序"链接，双击不挂控制台终端；调试版保留终端看 eprintln 日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::sync::OnceLock;

use cmx_agent_app::{AgentApp, AuthConfig, DesktopAppBuilder, dispatch_json};
use tauri::{Emitter, Manager, State};

mod update_common;

/// 门户地址·构建期烧录（唯一来源）：build.rs 从仓库根 `.env` 的 CMX_AGENT_PORTAL_BASE 读取注入
/// （CI 可用编译环境变量 CMX_AGENT_PORTAL_DEFAULT 覆盖）。[`portal_base`] 直接取值——
/// 分发包零配置即连打包时配置的门户，运行期不可改；未注入退回 [`AuthConfig::default`]（团队门户）。
fn portal_packed_default() -> Option<String> {
    option_env!("CMX_AGENT_PORTAL_DEFAULT")
        .map(|s| s.to_string())
        .filter(|s| !s.trim().is_empty())
}

/// IM 桥状态（启动日志/诊断）：启动结果一次性写入（改配置需重启，状态随进程不变）。
static IM_STATUS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn set_im_status(s: String) {
    let _ = IM_STATUS.set(s);
}

/// 门户基址统一来源（登录门 / IM 绑定 client / IM 桥 resolver 三处共用）：
/// **构建期烧录默认**（仓库根 `.env` 的 CMX_AGENT_PORTAL_BASE，见 [`portal_packed_default`]）>
/// 默认本机。运行期不提供任何修改手段（用户要求：分发包地址与打包配置一致，装后不可改）。
/// 注意存的是 **API 根**（`http://host:8080`，API 挂 `/api/*`）。
fn portal_base() -> String {
    portal_packed_default().unwrap_or_else(|| AuthConfig::default().base_url)
}

/// 运行中的 IM 桥句柄：stop 信号（热重载用）+ provider（其 stop() 断开飞书长连接）。
/// **多通道**：每个启用的 provider 一项（飞书 + QQ 可同时在线）。
struct ImBridgeHandle {
    stop_tx: tokio::sync::watch::Sender<bool>,
    provider: Arc<dyn cmx_agent_im::ImProvider>,
}

/// 当前桥的句柄槽（热重载：保存配置 → 停旧 → 起新）。多通道 = 多个句柄。
static IM_BRIDGES: std::sync::Mutex<Vec<ImBridgeHandle>> = std::sync::Mutex::new(Vec::new());

/// 停掉当前运行的 IM 桥（若有）。发 stop 信号 + 断 provider 长连接即返回——
/// 旧 task 异步退出，不阻塞新桥启动。
fn stop_im_bridge() {
    if let Ok(mut slots) = IM_BRIDGES.lock() {
        for h in slots.drain(..) {
            let _ = h.stop_tx.send(true);
            h.provider.stop();
        }
        if !slots.is_empty() {
            eprintln!("[im] 旧桥已停（热重载）");
        }
    }
}

/// IM 遥控装配（U16）：读 `CMX_AGENT_IM_*` env（开发联调）或 `<data_dir>/im.json`（GUI 设置面板），
/// 配了就为**每个启用的通道**（飞书/QQ/微信 可多选）启动一个
/// `ImBridge` 后台 task，与桌面 UI 共用同一 `AgentApp`。IM 消息跑同一回合循环 → 事件经 SessionEventBus →
/// `session_event` Tauri 事件实时推前端，回复经 ImBridge 回发 IM。
/// 未配 / 装配失败 → 仅打日志 + 记状态，不阻断桌面 UI（IM 是可选遥控通道）。
/// **热重载**：启动前先停全部旧桥——设置面板保存后直接重调本函数即可换配置，无需重启应用。
fn start_im_if_configured(rt: &'static tokio::runtime::Runtime, app: Arc<AgentApp>) {
    stop_im_bridge();
    let data_dir = cmx_agent_app::shared_data_dir();
    let resolved = match cmx_agent_im::resolve(Some(&data_dir)) {
        Ok(r) => r,
        Err(e) => {
            // 常见原因：env/im.json 均未配置。视为「未启用 IM」，静默不打扰纯本地用户。
            eprintln!("[im] 未启用 IM 遥控（{e}）");
            set_im_status(format!("未启动：{e}"));
            return;
        }
    };
    let allow = resolved.allow.clone();
    // 个人模式：im.json `personal=true`（默认）= 所有 IM 消息直接以桌面壳当前登录用户身份
    // 跑回合，无需验证码绑定；false = 绑定模式（按发送者 open_id 鉴权）。env 来源恒绑定模式。
    let personal = resolved.personal;
    // 无人值守全权：im.json `full_access=true`（默认）= IM 回合沙箱完全放行、从不弹审批卡
    // （回合级覆盖档，不影响桌面）；false = 跟随桌面全局两旋钮。
    let full_access = resolved.full_access;
    // 绑定模式才需要绑定解析器（个人模式会被桥短路，但仍装配以防模式切换复用代码路径）。
    let portal_base = portal_base();
    let labels: Vec<&str> = resolved.channels.iter().map(|c| c.kind.label()).collect();
    let mode_label = if personal { "个人模式" } else { "绑定模式" };
    let perm_label = if full_access { "全权" } else { "随桌面档" };
    set_im_status(format!(
        "运行中：provider={}（{mode_label}·{perm_label}，配置来源={}，portal={portal_base}）",
        labels.join("+"),
        resolved.source
    ));
    eprintln!(
        "[im] 启动 IM 遥控（provider={}，{mode_label}，权限={perm_label}，配置来源={}，portal={portal_base}）",
        labels.join("+"),
        resolved.source
    );
    for ch in resolved.channels {
        let kind_label = ch.kind.label();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let provider = Arc::clone(&ch.provider);
        let app = Arc::clone(&app);
        let bindings = cmx_agent_im::PortalBindingResolver::new(portal_base.clone());
        let allow = allow.clone();
        rt.spawn(async move {
            // 飞书 Stream：起常驻后台 task；微信长轮询：trait 默认 no-op。
            if let Err(e) = provider.start().await {
                eprintln!("[im] provider[{kind_label}].start 失败：{e}");
                return;
            }
            let bridge = cmx_agent_im::ImBridge::new(app, provider, kind_label, allow.clone())
                .with_personal(personal)
                .with_full_access(full_access)
                .with_bindings(std::sync::Arc::new(bindings));
            bridge.run(stop_rx).await;
        });
        // 句柄入槽（供下次热重载停旧：发 stop 信号 + provider.stop() 断长连接）。
        if let Ok(mut slots) = IM_BRIDGES.lock() {
            slots.push(ImBridgeHandle { stop_tx, provider: Arc::clone(&ch.provider) });
        }
    }
}

/// IM 遥控配置（设置 → IM 遥控 面板）。`action` = `get`（脱敏读 im.json）/
/// `set`（保存 im.json + **热重载桥**，立即生效）。
///
/// 返回与 `dispatch_json` 同形的 JSON 串（`{ok,data}` / `{ok:false,error:{message}}`），
/// 前端按统一信封解析。配置 schema 见 `cmx_agent_im::ImRemoconConfig`（secret keep/set 语义
/// 照模型配置面板：面板只回显掩码，不回传明文）。身份模式固定个人（`personal=true`），
/// 白名单不进面板（im.json 手工维护，GUI set 不触碰已有值）。
#[tauri::command]
async fn im_config(
    action: String,
    payload: Option<String>,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let data_dir = cmx_agent_app::shared_data_dir();
    match action.as_str() {
        "get" => {
            let data = cmx_agent_im::load_im_config(&data_dir)
                .map(|c| c.masked())
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "configured": false,
                        "enabled": true,
                        "kind": "feishu",
                        "personal": true,
                        "full_access": true,
                        "app_id": "",
                        "app_secret_masked": "",
                        "base": "",
                        "qq_app_id": "",
                        "qq_secret_masked": "",
                        "qq_base": "",
                        "wechat_bot_id": "",
                        "wechat_token_masked": "",
                        "wechat_base": "",
                    })
                });
            let mut data = data;
            data["portal_base"] = serde_json::json!(portal_base()); // 只读回显（运行期不可改）
            data["env_active"] = serde_json::json!(cmx_agent_im::env_active());
            Ok(serde_json::json!({ "ok": true, "data": data }).to_string())
        }
        "set" => {
            let v: serde_json::Value = payload
                .and_then(|p| serde_json::from_str(&p).ok())
                .ok_or_else(|| "invalid payload".to_string())?;
            let get_str = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string());
            // 读旧配置（keep 语义沿用已存 secret；首次保存则从面板取 set 值）。
            let mut cfg = cmx_agent_im::load_im_config(&data_dir).unwrap_or_default();
            cfg.enabled = v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true);
            cfg.personal = true; // 身份模式固定个人：消息以桌面登录账号身份跑回合。
            // 无人值守全权（默认开）：IM 回合沙箱完全放行、从不弹审批卡；false = 跟随桌面全局档。
            cfg.full_access = v.get("full_access").and_then(|x| x.as_bool()).unwrap_or(true);
            // 多通道（2026-09-10）：面板多选 → `active` 列表；`kind` 同步为首个启用项
            // （兼容旧版本读文件的语义）。空列表 = 未勾任何通道。
            if let Some(active) = v.get("active").and_then(|x| x.as_array()) {
                cfg.active = active
                    .iter()
                    .filter_map(|x| x.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect();
                cfg.kind = cfg.active.first().cloned().unwrap_or_default();
            } else if let Some(kind) = get_str("kind").filter(|s| !s.is_empty()) {
                // 兼容旧前端只报 kind：单选语义照旧。
                cfg.kind = kind;
                cfg.active = vec![cfg.kind.clone()];
            }
            if let Some(app_id) = get_str("app_id") {
                cfg.feishu.app_id = app_id;
            }
            if let Some(base) = get_str("base") {
                cfg.feishu.base = base;
            }
            // secret：action=set 用面板新值；keep 沿用已存值（面板只回显掩码）。
            if get_str("app_secret_action").as_deref() == Some("set") {
                cfg.feishu.app_secret = get_str("app_secret_value").unwrap_or_default();
            }
            // QQ：字段名 qq_*（与飞书的 app_id 区分，面板按 kind 分块提交）。
            // 沙箱/正式基址不进面板（正式用默认 api.sgroup.qq.com；联调沙箱手工改 im.json 的 qq.base）。
            if let Some(qq_app_id) = get_str("qq_app_id") {
                cfg.qq.app_id = qq_app_id;
            }
            if get_str("qq_secret_action").as_deref() == Some("set") {
                cfg.qq.app_secret = get_str("qq_secret_value").unwrap_or_default();
            }
            // 白名单不进 GUI：GUI set 不触碰 cfg.allow（im.json 手工维护，已有值保留）。
            // 门户服务器地址：运行期不可改（构建期烧录），set 请求里的 portal_base 一律忽略。
            // 启用态下按通道逐个校验凭证齐备（禁用态允许存半成品）。
            // 多通道：任一勾选通道缺凭证即拒——避免「以为两个都在线，其实只起了一个」。
            // wechat 凭证不进面板（im-login 扫码写入）；GUI set 不触碰 cfg.wechat，此处只校验。
            if cfg.enabled {
                let missing = cfg.active().into_iter().find_map(|k| match k.as_str() {
                    "feishu" if cfg.feishu.app_id.is_empty() => Some("飞书 App ID"),
                    "feishu" if cfg.feishu.app_secret.is_empty() => Some("飞书 App Secret"),
                    "qq" if cfg.qq.app_id.is_empty() => Some("QQ AppID"),
                    "qq" if cfg.qq.app_secret.is_empty() => Some("QQ AppSecret"),
                    "wechat" if cfg.wechat.bot_token.is_empty() => {
                        Some("微信 bot_token（先运行 cmx-agent im-login 扫码登录）")
                    }
                    _ => None,
                });
                if let Some(field) = missing {
                    return Ok(err_json("bad_request", &format!("{field} 不能为空")));
                }
            }
            cmx_agent_im::save_im_config(&data_dir, &cfg)?;
            // 热重载：保存即生效——停旧桥 + 按新 im.json 起新桥（无需重启应用）。
            // env 激活时不重载（env 优先且不可热换，面板已提示）。
            let app = Arc::clone(&state.app);
            let note = if cmx_agent_im::env_active() {
                "已保存。⚠ 检测到环境变量 CMX_AGENT_IM_*（开发模式）优先生效，im.json 暂不生效。"
            } else {
                start_im_if_configured(net_rt(), app);
                "已保存，IM 遥控已按新配置重载（立即生效）"
            };
            Ok(serde_json::json!({ "ok": true, "data": { "note": note } }).to_string())
        }
        other => Ok(err_json("bad_request", &format!("未知 action：{other}"))),
    }
}

/// 微信扫码登录会话槽（GUI 分步驱动：`start` 取码入槽 → 前端 ~2s 一次 `poll`；
/// `confirmed` 落盘后出槽，终态错误作废会话）。
static WECHAT_QR: std::sync::Mutex<Option<cmx_agent_im::WechatQrSession>> = std::sync::Mutex::new(None);

/// QQ 机器人扫码绑定会话槽（官方 `q.qq.com/lite` 绑定任务：task_id + bind_key）。
/// 分步驱动与 [`WECHAT_QR`] 同款：`start` 入槽 → 前端 ~2s 一次 `poll` → confirmed 作废。
static QQ_BIND: std::sync::Mutex<Option<cmx_agent_im::QqBindSession>> = std::sync::Mutex::new(None);

/// QQ 机器人扫码登录（设置 → IM 遥控 → QQ 卡片「扫码登录」；官方 OpenClaw 通道）。
/// `action`：
/// - `start`：创建绑定任务，返回 `{status:"qr", qr:<授权页 URL>}`（前端渲染成二维码，
///   手机 QQ 扫码打开并确认；重开会覆盖旧会话）。
/// - `poll`：轮询一次，返回 `{status:"waiting"}|{status:"confirmed", app_id}`；
///   二维码过期等终态走错误信封（前端据此停止轮询，可重按「扫码登录」）。
///
/// confirmed 时解密出的 AppID/AppSecret 写入 im.json `qq` 字段并把 `qq` 追加进 `active`
/// （官方 WebSocket 接入协议不变，只是凭证来源从手填变成扫码下发），启用态下立即热重载。
/// 网络统一走 `net_rt()`（理由见 [`im_wechat_login`] 注释）。
#[tauri::command]
async fn im_qq_login(action: String, state: State<'_, AppState>) -> Result<String, String> {
    let data_dir = cmx_agent_app::shared_data_dir();
    let app = Arc::clone(&state.app);
    tauri::async_runtime::spawn_blocking(move || {
        net_rt().block_on(async move {
            match action.as_str() {
                "start" => {
                    let sess = cmx_agent_im::create_bind_task(None).await.map_err(|e| {
                        tracing::warn!("im_qq_login start 失败：{e}");
                        err_json("login_failed", &e)
                    })?;
                    let qr = cmx_agent_im::connect_url(&sess.task_id, None);
                    if let Ok(mut slot) = QQ_BIND.lock() {
                        *slot = Some(sess);
                    }
                    Ok(serde_json::json!({ "ok": true, "data": { "status": "qr", "qr": qr } }).to_string())
                }
                "poll" => {
                    // 锁不跨 await：先 take 出会话，按结果放回（继续轮询）或作废。
                    let sess = QQ_BIND.lock().ok().and_then(|mut s| s.take());
                    let Some(sess) = sess else {
                        return Ok(
                            serde_json::json!({ "ok": true, "data": { "status": "idle" } }).to_string()
                        );
                    };
                    match cmx_agent_im::poll_bind_result(&sess, None).await {
                        Ok(cmx_agent_im::QqBindEvent::Pending) => {
                            if let Ok(mut slot) = QQ_BIND.lock() {
                                *slot = Some(sess);
                            }
                            Ok(serde_json::json!({ "ok": true, "data": { "status": "waiting" } }).to_string())
                        }
                        Ok(cmx_agent_im::QqBindEvent::Expired) => {
                            // 任务过期：会话已作废（终态），前端停轮询，可重按「扫码登录」。
                            Ok(err_json("login_failed", "二维码已过期，请重新扫码"))
                        }
                        Ok(cmx_agent_im::QqBindEvent::Completed { app_id, secret }) => {
                            // 凭证落盘 im.json + qq 追加进 active（官方 WS 接入协议不变）。
                            let mut cfg = cmx_agent_im::load_im_config(&data_dir).unwrap_or_default();
                            cfg.qq.app_id = app_id.clone();
                            cfg.qq.app_secret = secret;
                            if !cfg.active.iter().any(|k| k == "qq") {
                                cfg.active.push("qq".into());
                            }
                            if cfg.kind.trim().is_empty() {
                                cfg.kind = "qq".into();
                            }
                            cmx_agent_im::save_im_config(&data_dir, &cfg)
                                .map_err(|e| err_json("login_failed", &e))?;
                            // 启用态且 env 未接管 → 立即热重载（扫码即上线，无需再点保存）。
                            let note = if !cfg.enabled {
                                "已保存（遥控总开关当前关闭，启用后生效）"
                            } else if cmx_agent_im::env_active() {
                                "已保存。⚠ 检测到环境变量 CMX_AGENT_IM_* 优先生效，im.json 暂不生效。"
                            } else {
                                start_im_if_configured(net_rt(), app);
                                "已保存，QQ 通道已上线"
                            };
                            Ok(serde_json::json!({
                                "ok": true,
                                "data": { "status": "confirmed", "app_id": app_id, "note": note }
                            }).to_string())
                        }
                        Err(e) => {
                            // 轮询单次失败（网络抖动）：会话放回 + 返回 waiting 让前端继续轮，
                            // 错误只进日志——err_json 会当终态停轮询，把正常登录打死。
                            if let Ok(mut slot) = QQ_BIND.lock() {
                                *slot = Some(sess);
                            }
                            tracing::warn!("im_qq_login 轮询失败（下轮重试）：{e}");
                            Ok(serde_json::json!({ "ok": true, "data": { "status": "waiting" } }).to_string())
                        }
                    }
                }
                other => Ok(err_json("bad_request", &format!("未知 action：{other}"))),
            }
        })
    })
    .await
    .map_err(|e| format!("task failed: {e}"))?
}

/// 微信扫码登录（设置 → IM 遥控 → 微信卡片「扫码登录」）。`action`：
/// - `start`：取二维码，返回 `{status:"qr", qr:<PNG data-url>}`（重开会覆盖旧会话）。
/// - `poll`：轮询一次，返回 `{status:"waiting"|"scaned"|"refreshed"(带新 qr)|
///   "confirmed"(带 bot_id)}`；过期/超时等终态错误走错误信封（前端据此停止轮询）。
///
/// confirmed 时凭证写入 im.json `wechat` 字段并把 `wechat` 追加进 `active`（与 CLI
/// `im-login` 同款），启用态下立即热重载 IM 桥（env 激活时 im.json 不生效，不动桥）。
/// 网络统一走 `net_rt()`（Tauri 异步驱动上直接跑 reqwest 会挂起，见 [`net_rt`] 注释）。
#[tauri::command]
async fn im_wechat_login(action: String, state: State<'_, AppState>) -> Result<String, String> {
    let data_dir = cmx_agent_app::shared_data_dir();
    let app = Arc::clone(&state.app);
    tauri::async_runtime::spawn_blocking(move || {
        net_rt().block_on(async move {
            match action.as_str() {
                "start" => {
                    let base = cmx_agent_im::load_im_config(&data_dir)
                        .map(|c| c.wechat.base)
                        .filter(|b| !b.trim().is_empty());
                    let provider = cmx_agent_im::WechatProvider::new("", base);
                    let sess = provider
                        .qr_begin()
                        .await
                        .map_err(|e| {
                            tracing::warn!("im_wechat_login start 失败：{e}");
                            err_json("login_failed", &e)
                        })?;
                    let qr = sess.qr_content().to_string();
                    if let Ok(mut slot) = WECHAT_QR.lock() {
                        *slot = Some(sess);
                    }
                    Ok(serde_json::json!({ "ok": true, "data": { "status": "qr", "qr": qr } }).to_string())
                }
                "poll" => {
                    // 锁不跨 await：先 take 出会话，按结果放回（继续轮询）或作废。
                    let sess = WECHAT_QR.lock().ok().and_then(|mut s| s.take());
                    let Some(mut sess) = sess else {
                        return Ok(
                            serde_json::json!({ "ok": true, "data": { "status": "idle" } }).to_string()
                        );
                    };
                    match sess.poll_once().await {
                        Ok(cmx_agent_im::QrEvent::Confirmed(login)) => {
                            // 凭证落盘 im.json + wechat 追加进 active（与 CLI im-login 同款）。
                            let mut cfg = cmx_agent_im::load_im_config(&data_dir).unwrap_or_default();
                            cfg.wechat = cmx_agent_im::WechatCreds {
                                bot_token: login.bot_token,
                                bot_id: login.bot_id.clone(),
                                user_id: login.user_id,
                                base: login.base,
                            };
                            if !cfg.active.iter().any(|k| k == "wechat") {
                                cfg.active.push("wechat".into());
                            }
                            if cfg.kind.trim().is_empty() {
                                cfg.kind = "wechat".into();
                            }
                            cmx_agent_im::save_im_config(&data_dir, &cfg)
                                .map_err(|e| err_json("login_failed", &e))?;
                            // 启用态且 env 未接管 → 立即热重载（扫码即上线，无需再点保存）。
                            let note = if !cfg.enabled {
                                "已保存（遥控总开关当前关闭，启用后生效）"
                            } else if cmx_agent_im::env_active() {
                                "已保存。⚠ 检测到环境变量 CMX_AGENT_IM_* 优先生效，im.json 暂不生效。"
                            } else {
                                start_im_if_configured(net_rt(), app);
                                "已保存，微信通道已上线"
                            };
                            Ok(serde_json::json!({
                                "ok": true,
                                "data": { "status": "confirmed", "bot_id": login.bot_id, "note": note }
                            }).to_string())
                        }
                        Ok(ev) => {
                            // Refreshed 后会话里是新码：先取内容再放回槽。
                            let qr = if let cmx_agent_im::QrEvent::Refreshed = ev {
                                Some(sess.qr_content().to_string())
                            } else {
                                None
                            };
                            if let Ok(mut slot) = WECHAT_QR.lock() {
                                *slot = Some(sess);
                            }
                            let status = match ev {
                                cmx_agent_im::QrEvent::Scaned => "scaned",
                                cmx_agent_im::QrEvent::Refreshed => "refreshed",
                                _ => "waiting",
                            };
                            let mut data = serde_json::json!({ "status": status });
                            if let Some(qr) = qr {
                                data["qr"] = serde_json::json!(qr);
                            }
                            Ok(serde_json::json!({ "ok": true, "data": data }).to_string())
                        }
                        // 终态错误（超时/多次过期/未知状态）：会话已作废，前端停止轮询。
                        Err(e) => {
                            tracing::warn!("im_wechat_login 轮询失败：{e}");
                            Ok(err_json("login_failed", &e))
                        }
                    }
                }
                other => Ok(err_json("bad_request", &format!("未知 action：{other}"))),
            }
        })
    })
    .await
    .map_err(|e| format!("task failed: {e}"))?
}

/// 统一错误信封（与 AppResponse::err 同形，前端 call 风格解析）。
fn err_json(code: &str, message: &str) -> String {
    serde_json::json!({ "ok": false, "error": { "code": code, "message": message } }).to_string()
}
/// 应用状态：一个共享的 AgentApp（认证态用内部 Mutex，可跨命令共享）+ 会话回放完成信号。
struct AppState {
    app: Arc<AgentApp>,
    /// 启动会话回放（后台 task，见 setup）结束置 true——成功/失败都置，
    /// [`session_restore_done`] 据此放行前端首次路由。
    restore_done: tokio::sync::watch::Receiver<bool>,
}

/// 专用 tokio 运行时（`enable_all` = io + time driver），供连接器 / 认证 reqwest 等网络工具可靠运行。
///
/// **为何需要**：Tauri 命令跑在 Tauri 自身的异步运行时上；实践中在其上直接跑 reqwest 会挂起
/// （运行时驱动/上下文不匹配）。把走网络的 `dispatch_json` 放到本自建 `enable_all` 运行时上执行，
/// 经 spawn_blocking 桥回 Tauri 命令，彻底规避该问题。
fn net_rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build net runtime")
    })
}

/// 在专用运行时上派发一段 JSON 请求（脱离 Tauri 异步驱动线程；reqwest 可用）。
async fn dispatch_on_net(app: Arc<AgentApp>, payload: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || net_rt().block_on(dispatch_json(&app, &payload)))
        .await
        .map_err(|e| format!("task failed: {e}"))
}

/// 通用前门命令：把一串 JSON 请求交给核派发，回一串 JSON 响应。
/// 前端：`window.__TAURI__.core.invoke("agent", { payload: JSON.stringify({cmd:"send",...}) })`。
#[tauri::command]
async fn agent(payload: String, state: State<'_, AppState>) -> Result<String, String> {
    let preview: String = payload.chars().take(80).collect();
    eprintln!("[agent] invoke: {preview}");
    dispatch_on_net(state.app.clone(), payload).await
}

/// 当前操作系统标识（"macos" | "windows" | "linux" | ...= `std::env::consts::OS`）。
///
/// 前端据此决定标题条形态：macOS 用系统红绿灯（Overlay，见 tauri.conf.json）；Windows/Linux 关闭系统
/// 装饰（tauri.windows.conf.json / tauri.linux.conf.json 的 decorations:false）后需自绘最小化/最大化/
/// 关闭按钮 + 缩放热区。不用 `tauri-plugin-os`（避免新增依赖首次联网编译）——编译期常量足够。
#[tauri::command]
fn platform() -> &'static str {
    std::env::consts::OS
}

/// 会话回放完成信号（前端 login.js 首次路由前等待）：启动后台回放（成功/失败皆放行）结束即返回。
/// 让已登录用户启动不闪登录页（先等回放再查 current_user）；断网时回放最多等 connect_timeout=5s，
/// 前端另有 8s JS 兜底——窗口全程可交互，回放不再阻塞事件循环（见 setup 注释）。
#[tauri::command]
async fn session_restore_done(state: State<'_, AppState>) -> Result<bool, String> {
    let mut rx = state.restore_done.clone();
    if *rx.borrow() {
        return Ok(true);
    }
    // 发送端随回放 task 结束而 drop；异常未置位时 changed() 报错 → false 放行（fail-open）。
    Ok(rx.changed().await.is_ok())
}

/// 调起操作系统原生的文件夹选择器，返回用户选择的绝对路径；用户取消返回 `None`。
///
/// 实现收敛在 `cmx-agent-app::workspace`（与 Web 壳共用一份，Windows 用系统 PowerShell
/// 调 IFileDialog COM 现代样式，macOS 用 AppleScript，Linux 优先 Zenity 其次 KDialog），
/// 返回路径仍统一交给 `add_local_workspace` 校验。
#[tauri::command]
async fn pick_local_directory() -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(cmx_agent_app::workspace::native_pick_local_directory)
        .await
        .map_err(|e| format!("文件夹选择任务失败：{e}"))?
}

/// 登录命令（SPA 单窗口）：校验凭据 → 返回用户 JSON（前端 js/login.js 切视图）；失败返回错误文案。
#[tauri::command]
async fn login(
    state: State<'_, AppState>,
    username: String,
    password: String,
) -> Result<String, String> {
    eprintln!("[login] attempt user={username}");
    let payload = serde_json::json!({ "cmd": "login", "username": username, "password": password })
        .to_string();
    let out = dispatch_on_net(state.app.clone(), payload).await?;
    let v: serde_json::Value = serde_json::from_str(&out).map_err(|e| e.to_string())?;
    if v.get("ok").and_then(|b| b.as_bool()) == Some(true) {
        let user = v
            .get("data")
            .and_then(|d| d.get("user"))
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        Ok(user.to_string())
    } else {
        let msg = v
            .get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("登录失败")
            .to_string();
        eprintln!("[login] failed: {msg}");
        Err(msg)
    }
}

/// 显示前把默认窗口尺寸收敛进所在屏的**真实工作区**（任务栏/菜单栏/Dock 之外），并在
/// 工作区内居中——三端统一（Windows rcWork / Linux GDK workarea / macOS visibleFrame）。
///
/// 背景：conf 1440×900 是逻辑像素，高分屏扣掉系统面板后可视区放不下，居中摆会被底部
/// 面板遮一截（200% 缩放屏上整条压任务栏下）。必须赶在 show 前做完，用户无感。
/// 为何不用「显示器尺寸扣固定余量」：面板物理高度随缩放走（48 逻辑 × 200%=96 物理），
/// 估算必偏；`center()` 又是对整屏居中，误差两端摊、底部仍被遮。
fn fit_default_window(win: &tauri::WebviewWindow) {
    match work_area_logical(win) {
        Some(wa) => fit_into_work_area(win, wa),
        // 取不到工作区（罕见/平台兜底）：维持 conf 原样，宁缺勿错。
        None => eprintln!("[win-fit] no work-area available, keep conf size"),
    }
}

/// 共享收敛：把窗口**外框**钳进 wa（逻辑矩形 x,y,w,h）并在其中居中。
///
/// 关键语义：tauri 的 `set_size` 设的是**内框**（客户区）。无装饰但可缩放的窗口在平台层
/// 保留边框热区（Windows WS_THICKFRAME 不可见边框实测 +26×+72 物理 @200%），直接设工作
/// 区逻辑尺寸，外框会再大出一圈、底部仍压面板。须按 outer−inner 差值反推内框目标，让
/// **外框**恰好铺满工作区（多余边框是透明热区，超出屏幕无感知）；floor 舍入恒朝「更小」，
/// 任何 DPI 下都不会再溢出工作区半像素。定位不能用 `center()`（对整屏居中，会把窗口重新
/// 推回面板下），按工作区手动定位。
fn fit_into_work_area(win: &tauri::WebviewWindow, wa: (f64, f64, f64, f64)) {
    let Ok(cur_phys) = win.outer_size() else { return };
    let Ok(sf) = win.scale_factor() else { return };
    let cur = cur_phys.to_logical::<f64>(sf);
    let w = cur.width.min(wa.2);
    let h = cur.height.min(wa.3);
    let changed = h < cur.height - 0.5 || w < cur.width - 0.5;
    let inner = win.inner_size().unwrap_or(cur_phys);
    let dw = f64::from(cur_phys.width.saturating_sub(inner.width));
    let dh = f64::from(cur_phys.height.saturating_sub(inner.height));
    let iw = ((w * sf - dw) / sf).floor();
    let ih = ((h * sf - dh) / sf).floor();
    eprintln!(
        "[win-fit] wa={wa:?} cur={cur:?} -> outer=({w},{h}) inner=({iw},{ih}) delta=({dw},{dh}) changed={changed}"
    );
    if changed {
        let _ = win.set_size(tauri::LogicalSize::new(iw, ih));
        let x = wa.0 + (wa.2 - w) / 2.0;
        let y = wa.1 + (wa.3 - h) / 2.0;
        let _ = win.set_position(tauri::LogicalPosition::new(x.round(), y.round()));
    }
}

/// Windows：`GetMonitorInfoW` 的 rcWork——任务栏位置/高度/自动隐藏/副屏差异全部精确。
/// rcWork 是虚拟屏**物理**坐标，除以缩放系数折回逻辑值。
#[cfg(windows)]
fn work_area_logical(win: &tauri::WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    use windows_sys::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    let hwnd = win.hwnd().ok()?.0 as windows_sys::Win32::Foundation::HWND;
    unsafe {
        let hmon = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if hmon.is_null() {
            return None;
        }
        let mut mi: MONITORINFO = std::mem::zeroed();
        mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut mi) == 0 {
            return None;
        }
        let sf = win.scale_factor().ok()?;
        Some((
            mi.rcWork.left as f64 / sf,
            mi.rcWork.top as f64 / sf,
            (mi.rcWork.right - mi.rcWork.left) as f64 / sf,
            (mi.rcWork.bottom - mi.rcWork.top) as f64 / sf,
        ))
    }
}

/// Linux：GDK `Monitor::workarea()`（自动扣除 GNOME 顶栏/底栏、Dock 等面板）。GDK 坐标
/// 本就是逻辑像素，无需缩放换算。窗口隐藏未 realize 时可能拿不到所属屏，回退主屏。
/// **Wayland 注意**：合成器限制程序自定窗口位置（set_position 可能被忽略），但尺寸收敛
/// 仍生效——遮挡问题照样解决，摆位交给桌面（通常居中，正合适）。
#[cfg(target_os = "linux")]
fn work_area_logical(win: &tauri::WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    use gtk::prelude::*;
    let gtk_win = win.gtk_window().ok()?;
    let display = gtk_win.display();
    let monitor = gtk_win
        .window()
        .and_then(|w| display.monitor_at_window(&w))
        .or_else(|| display.primary_monitor())?;
    let wa = monitor.workarea();
    Some((
        wa.x() as f64,
        wa.y() as f64,
        wa.width() as f64,
        wa.height() as f64,
    ))
}

/// macOS：`NSScreen.visibleFrame`（自动扣除菜单栏 + Dock，单位就是逻辑点）。
/// AppKit 坐标系原点在**左下**、y 向上；Tauri 定位用左上原点，须把 workarea 顶边换算成
/// 「屏高 − (y + 高)」。窗口尚未上屏时 `screen()` 为空，回退主屏。
#[cfg(target_os = "macos")]
fn work_area_logical(win: &tauri::WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    use objc2_app_kit::{NSWindow, NSScreen};
    let ns_win: *mut NSWindow = win.ns_window().ok()?.cast();
    // SAFETY：ns_window 由 Tauri 在主线程创建，setup 同在主线程；此处仅读取屏幕矩形。
    // NSScreen::mainScreen 需 MainThreadMarker（objc2 0.6 API）——本函数只在 setup 主线程被调。
    unsafe {
        let mtm = objc2::MainThreadMarker::new_unchecked();
        let screen = (*ns_win).screen().or_else(|| NSScreen::mainScreen(mtm))?;
        let vf = screen.visibleFrame();
        let top_y = screen.frame().size.height - (vf.origin.y + vf.size.height);
        Some((vf.origin.x, top_y, vf.size.width, vf.size.height))
    }
}

/// 其余平台（Tauri 实际不支持，仅为可编译性兜底）：无工作区信息，跳过收敛。
#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn work_area_logical(_: &tauri::WebviewWindow) -> Option<(f64, f64, f64, f64)> {
    None
}

fn build_app() -> AgentApp {
    // 双壳统一数据根（与 Web 壳同一份：model.json / 会话共享；CMX_AGENT_DATA_DIR 可覆盖，隔离测试用）。
    let data_dir = cmx_agent_app::shared_data_dir();
    let workdir = data_dir.join("workspaces").join("default");
    std::fs::create_dir_all(&workdir).ok();

    // E0：按 env / <data_dir>/model.json 选真实模型（OpenAI 兼容：DeepSeek/OpenAI/Qwen/本地）或回退 DemoModel。
    let model = cmx_agent_app::select_model(Some(&data_dir));

    // U3：从 <data_dir>/mcp.json 连接外部 MCP server（sync main → 在 net_rt 上跑异步连接）。
    // U15：plugins 目录里 kind:"mcp" 的插件清单也一并连上（统一插件面入口）。
    let mcp_path = data_dir.join("mcp.json");
    let plugins_path = data_dir.join("plugins");
    let mcp_tools = net_rt().block_on(async move {
        let mut t = cmx_agent_mcp::load_and_connect(&mcp_path).await;
        t.extend(cmx_agent_plugin::connect_mcp_plugins(&plugins_path).await);
        t
    });

    let mut app = DesktopAppBuilder::new(workdir, data_dir.clone(), model)
        .connectors(cmx_agent_app::ConnectorConfig::default())
        // 登录门：对接门户 /api/auth（基址统一走 portal_base()：构建期烧录默认，运行期不可改）。
        .auth(AuthConfig { base_url: portal_base() })
        .interactive_approval() // X4：shell 等需审批工具挂起等前端点按
        .mcp_tools(mcp_tools)   // U3：外部 MCP 工具
        // U15 插件市场：配 env 则拉远程目录（未配回落 env/无市场，见 builder 双保险）。
        .plugin_market(std::env::var("CMX_AGENT_PLUGIN_MARKET").ok())
        // U13：opt-in 数据权限接地——CMX_AGENT_DATAAUTH_URL 指向 cmx-data-auth 即启用真 PEP。
        .maybe_data_auth(std::env::var("CMX_AGENT_DATAAUTH_URL").ok())
        .build()
        .expect("build agent app");

    // 登出钩子：停 IM 桥（发 stop 信号 + 断 provider 长连接）——旧实现登出后桥仍以
    // 旧身份轮询/跑回合，换号后 IM 消息会按前任登录人身份处理。
    app.add_logout_hook(Box::new(stop_im_bridge));

    // IM 绑定 client（个人模式不调用，绑定模式走它调门户 /api/agent/bindings/*）。
    // 门户基址与登录门同源（portal_base()）。
    app = app.with_im_binding(cmx_agent_app::ImBindingClient::new(portal_base()));

    app
}

/// 流式会话命令（办公助手对话）：在 net_rt 上跑一个回合，回合内每个事件经 Tauri 事件 `channel`
/// 实时 emit 给前端（对齐 cmx-ai 的 SSE 事件流）。末尾补 `{kind:"stream_done"|"stream_error"}`。
#[tauri::command]
async fn send_stream(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    text: String,
    channel: String,
) -> Result<(), String> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    // 转发：rx → Tauri emit（在 Tauri 异步运行时上）。
    let apph = app.clone();
    let chan = channel.clone();
    let fwd = tauri::async_runtime::spawn(async move {
        while let Some(v) = rx.recv().await {
            let _ = apph.emit(&chan, v);
        }
    });
    // 跑回合：spawn_blocking + net_rt（reqwest 可用），sink 把事件送进 tx。
    let agent = state.app.clone();
    let run = tauri::async_runtime::spawn_blocking(move || {
        net_rt().block_on(async move {
            let sink = std::sync::Arc::new(cmx_agent_app::ChannelSink::new(tx.clone()));
            let r = agent.send_streaming(&session_id, &text, sink).await;
            match &r {
                Ok(_) => {
                    let _ = tx.send(serde_json::json!({"kind":"stream_done"}));
                }
                Err(e) => {
                    let _ = tx.send(serde_json::json!({"kind":"stream_error","message": e.to_string()}));
                }
            }
            // tx 在此 drop（sink 也已 drop）→ rx 关闭 → fwd 结束
            r.map(|_| ()).map_err(|e| e.to_string())
        })
    })
    .await
    .map_err(|e| format!("task failed: {e}"))?;
    let _ = fwd.await;
    run
}

fn main() {
    // 初始化 tracing：默认 info，RUST_LOG 可调。IM 桥/Stream 的连接、未授权 chat_id 等都靠它打到 stderr。
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .with_target(false)
        .init();

    let app = build_app();

    // 自检模式：`--selftest` 跑一次 list_connectors；`--selftest-login <user> <pass>` 跑一次登录。
    // 不开窗，直接在 net_rt 上执行并打印——把「网络/认证」与「窗口/运行时」问题分开验证。
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--selftest") {
        let app = app.into_shared();
        let out = net_rt()
            .block_on(async move { dispatch_json(&app, r#"{"cmd":"list_connectors"}"#).await });
        println!("{out}");
        return;
    }
    if let Some(i) = args.iter().position(|a| a == "--selftest-login") {
        let user = args.get(i + 1).cloned().unwrap_or_else(|| "admin".into());
        let pass = args
            .get(i + 2)
            .cloned()
            .unwrap_or_else(|| "Admin@12345".into());
        let app = app.into_shared();
        let payload =
            serde_json::json!({"cmd":"login","username":user,"password":pass}).to_string();
        let out = net_rt().block_on(async move { dispatch_json(&app, &payload).await });
        println!("{out}");
        return;
    }

    // 预热专用运行时（在 Tauri 运行时启动前构建，确保不在任何 tokio 上下文内创建）。
    let _ = net_rt();
    eprintln!("[main] net_rt ready; launching login window (main hidden until login)（登录在前端 #/login 路由，§11.4）");

    let app = app.into_shared();

    // U16：IM 遥控（飞书/微信/钉钉…）。按 env 装配——配了 CMX_AGENT_IM_KIND 等就启动 ImBridge
    // 后台 task（与 CLI `im` 模式同一套装配），与桌面 UI 共用同一个 AgentApp：IM 消息跑同一回合循环，
    // 事件经 SessionEventBus → session_event Tauri 事件实时推前端，回复经 ImBridge 回发 IM。
    // 未配 IM env → 跳过，桌面壳照常纯本地用。错误只打日志不阻断 UI（IM 是可选遥控通道）。
    start_im_if_configured(net_rt(), Arc::clone(&app));

    // 会话回放完成信号（watch）：后台回放 task 结束置 true（成功/失败都置）。
    // 前端 login.js 首次路由前 invoke session_restore_done 等它——已登录用户不闪登录页。
    let (restore_tx, restore_rx) = tokio::sync::watch::channel(false);

    tauri::Builder::default()
        // 自动更新（方案 C2）：updater 插件（check/download/验签/install）+ process 插件（relaunch 备用）。
        // 前端统一走 update_common.rs 的命令入口，不直接调插件 JS API（Linux 支线将来同走命令入口）。
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .manage(AppState { app, restore_done: restore_rx })
        .invoke_handler(tauri::generate_handler![
            agent,
            login,
            send_stream,
            platform,
            session_restore_done,
            im_config,
            im_wechat_login,
            im_qq_login,
            pick_local_directory,
            update_common::get_app_version,
            update_common::check_update,
            update_common::download_and_install
        ])
        // U16：会话事件总线 → 前端实时通道。常驻 task 订阅 AgentApp 的 event_bus，把任意来源
        //（本地 / IM 桥 / 后续 webhook）的会话事件 emit 成全局 `session_event` Tauri 事件。
        // 前端 `index.html` 监听它，按 session_id 分流渲染——实现「飞书发消息实时显示到对话界面」，
        // 且 IM 无关：微信/钉钉接入后走同一条路，零额外改动。
        // 同时设置 cmx 图标（Linux 任务栏/标题栏；bundle.icon 仅打包时生效）。
        .setup(move |app| {
            // Windows/Linux 关主窗系统装饰（前端自绘三键 + 缩放热区）；macOS 走 Overlay 交通灯。
            // 窗口 visible:false 创建（conf），装饰处理完再 show——消除首帧白屏 + 系统标题栏闪烁。
            if let Some(main) = app.get_webview_window("main") {
                // 显示前把默认窗口尺寸收敛进所在屏的**真实工作区**（任务栏之外）并重新定位。
                // conf 的 1440×900 是逻辑像素：200% 缩放屏（2880×1800）上=物理满屏，底部整条
                // 压在任务栏下。必须赶在 show 前做完，用户看见窗口时已是修正位。
                fit_default_window(&main);
                if !cfg!(target_os = "macos") {
                    let _ = main.set_decorations(false);
                }
                let _ = main.show();
                let _ = main.set_focus();
            }
            // P1 更新上报：启动即异步消费 pending 标记（升级成功才计数），不阻塞启动。
            tauri::async_runtime::spawn(update_common::report_pending_update(app.handle().clone()));
            let app_state = app.state::<AppState>();
            let app_ref = app_state.app.clone();
            let handle = app.handle().clone();
            // 会话回放：本地 auth.json 有效（access 可用或 refresh 续签成功）→ 恢复认证态。
            // **后台跑，不阻塞 setup**：断网时 /me 是 TCP 黑洞（无响应包，等 connect_timeout=5s），
            // 同步 block_on 会把事件循环冻到超时——窗口白屏假死、关闭也无效。改为 net_rt 后台
            // task：成功 emit logged-in（前端 main.js 已监听，切主视图并刷新数据）；无论成败都置
            // restore_done 信号（前端 login.js 首次路由前等它，避免已登录用户闪登录页）。
            let restore_app = app_ref.clone();
            let restore_emit = handle.clone();
            net_rt().spawn(async move {
                let restored = restore_app.try_restore_session().await;
                let _ = restore_tx.send(true);
                if restored {
                    eprintln!("[session] 会话回放成功（认证态已恢复）");
                    let user = restore_app.current_user().unwrap_or(serde_json::Value::Null);
                    let _ = restore_emit.emit("logged-in", user);
                }
            });
            net_rt().spawn(async move {
                let mut rx = app_ref.event_bus().subscribe();
                loop {
                    match rx.recv().await {
                        Ok(env) => {
                            eprintln!("[bus] emit session_event session={} kind={}", env.session_id, serde_json::to_string(&env.event.kind).unwrap_or_default());
                            let _ = handle.emit("session_event", env);
                        }
                        // Lagged = 订阅者落后被广播器丢帧：必须续收而非退出——旧实现 `while let Ok`
                        // 在 Lagged 时直接终结转发 task，审批卡/消息/工具结果从此不再推前端，
                        // 挂起的审批只能等 300s 超时拒绝（长回合/多通道并发正是 lag 高发场景）。
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            eprintln!("[bus] session_event 落后 {n} 帧，已续收（通知前端重拉补齐）");
                            // 丢帧段无法凭空补：广播 `session_resync`，前端把打开中的会话 tab
                            // 标脏并整屏重拉落库事件（红蓝审查 P3-5——旧实现只打日志，丢的
                            // 事件要等用户手动重开 tab 才回来）。
                            let _ = handle.emit("session_resync", ());
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
                eprintln!("[main] session_event 转发 task 结束（总线已关闭）");
            });
            let img = tauri::include_image!("icons/icon.png");
            for (_, win) in app.webview_windows() {
                let _ = win.set_icon(img.clone());
            }
            Ok(())
        })
        // 关窗守卫：用户关闭任何可见窗口 = 退出应用（无托盘、无常驻诉求）。
        // 必须无条件硬退出，不能按登录态放行：会话回放/断网保留会让"登录窗可见且已认证"
        // 真实存在，放行关闭 = 全窗消失但进程残留；exit(0) 优雅退出在 webview 挂死
        // （如下载卡住）时也会卡在销毁阶段——std::process::exit 直接终结，不留假死。
        // 本应用状态（auth.json / update-pending.json）均为写穿落盘，硬退出无丢失风险。
        .on_window_event(|_window, event| {
            if let tauri::WindowEvent::CloseRequested { .. } = event {
                std::process::exit(0);
            }
        })
        .run(tauri::generate_context!())
        .expect("run tauri app");
}
