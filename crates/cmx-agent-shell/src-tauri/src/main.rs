//! cmx-agent 原生 Tauri 桌面壳入口。
//!
//! 「同核多壳」：本文件把 Tauri 的 invoke 边界接到 cmx-agent-app 的 `dispatch_json`——无业务逻辑。
//! 业务全在已测的 cmx-agent-app（façade / JSONL 落库 / 前门协议 / 五层守卫 / 认证），与 Web 壳共用同一核。
//! 前端 `ui/index.html` 里 `call()` 检测 `window.__TAURI__` 走 invoke，否则回退 HTTP。
//!
//! 登录门（参照 CMXPortalManager 的登录方式）：启动时**主窗口隐藏**、只显示 `login` 窗口；`login.html`
//! 提交 → `invoke("login")` → 校验凭据（对接门户 /api/auth/login）→ **成功后显示已存在的主窗口并关闭登录窗**。

// Windows 发布版按"窗口程序"链接，双击不挂控制台终端；调试版保留终端看 eprintln 日志。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::sync::OnceLock;

use cmx_agent_app::{AgentApp, AuthConfig, DesktopAppBuilder, dispatch_json};
use tauri::{Emitter, Manager, State};

/// IM 桥状态（设置面板显示）：启动结果一次性写入（改配置需重启，状态随进程不变）。
static IM_STATUS: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn set_im_status(s: String) {
    let _ = IM_STATUS.set(s);
}

fn im_status() -> String {
    IM_STATUS
        .get()
        .cloned()
        .unwrap_or_else(|| "未启动（未配置 IM 遥控）".into())
}

/// IM 遥控装配（U16）：读 `CMX_AGENT_IM_*` env（开发联调）或 `<data_dir>/im.json`（GUI 设置面板），
/// 配了就启动 `ImBridge` 后台 task，与桌面 UI 共用同一 `AgentApp`。IM 消息跑同一回合循环 →
/// 事件经 SessionEventBus → `session_event` Tauri 事件实时推前端，回复经 ImBridge 回发 IM。
/// 未配 / 装配失败 → 仅打日志 + 记状态，不阻断桌面 UI（IM 是可选遥控通道）。
fn start_im_if_configured(rt: &'static tokio::runtime::Runtime, app: Arc<AgentApp>) {
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
    let kind_label = resolved.kind.label();
    let allow = resolved.allow;
    // 用户绑定：门户可达即启用（按发送者 open_id 鉴权 + 以绑定用户身份跑回合）。
    // 绑定模式下白名单 allow 退化为会话级二次过滤；未配白名单则全放行（门=已绑定身份）。
    let portal_base = std::env::var("CMX_AGENT_PORTAL_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:8080".into());
    let bindings = cmx_agent_im::PortalBindingResolver::new(portal_base.clone());
    set_im_status(format!(
        "运行中：provider={kind_label}（绑定模式，配置来源={}，portal={portal_base}）",
        resolved.source
    ));
    eprintln!("[im] 启动 IM 遥控（provider={kind_label}，绑定模式，配置来源={}，portal={portal_base}）", resolved.source);
    let provider = resolved.provider;
    rt.spawn(async move {
        // 飞书 Stream：起常驻后台 task；Telegram 长轮询：trait 默认 no-op。
        if let Err(e) = provider.start().await {
            eprintln!("[im] provider.start 失败：{e}");
            return;
        }
        let bridge =
            cmx_agent_im::ImBridge::new(app, provider, kind_label, allow).with_bindings(std::sync::Arc::new(bindings));
        bridge.run().await;
    });
}

/// IM 遥控配置（设置 → IM 遥控 面板）。`action` = `get`（脱敏读 im.json + 桥状态）/
/// `set`（保存 im.json，重启生效）/ `test`（飞书凭证连通性预检，秒级不建长连接）。
///
/// 返回与 `dispatch_json` 同形的 JSON 串（`{ok,data}` / `{ok:false,error:{message}}`），
/// 前端按统一信封解析。配置 schema 见 `cmx_agent_im::ImRemoconConfig`（secret keep/set 语义
/// 照模型配置面板：面板只回显掩码，不回传明文）。
#[tauri::command]
async fn im_config(action: String, payload: Option<String>) -> Result<String, String> {
    let data_dir = cmx_agent_app::shared_data_dir();
    match action.as_str() {
        "get" => {
            let mut data = cmx_agent_im::load_im_config(&data_dir)
                .map(|c| c.masked())
                .unwrap_or_else(|| {
                    serde_json::json!({
                        "configured": false,
                        "enabled": true,
                        "kind": "feishu",
                        "app_id": "",
                        "app_secret_masked": "",
                        "base": "",
                        "telegram_token_masked": "",
                        "allow": "",
                    })
                });
            data["bridge"] = serde_json::json!(im_status());
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
            if let Some(kind) = get_str("kind").filter(|s| !s.is_empty()) {
                cfg.kind = kind;
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
            if get_str("telegram_token_action").as_deref() == Some("set") {
                cfg.telegram.token = get_str("telegram_token_value").unwrap_or_default();
            }
            // 白名单：逗号分隔 chat_id；空 = 不限（绑定模式门 = 已绑定身份）。
            cfg.allow = get_str("allow")
                .map(|s| {
                    s.split(',')
                        .map(|x| x.trim().to_string())
                        .filter(|x| !x.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            // 启用态下按 provider 校验凭证齐备（禁用态允许存半成品）。
            if cfg.enabled {
                let missing = match cfg.kind.trim() {
                    "telegram" if cfg.telegram.token.is_empty() => Some("Bot Token"),
                    "feishu" if cfg.feishu.app_id.is_empty() => Some("App ID"),
                    "feishu" if cfg.feishu.app_secret.is_empty() => Some("App Secret"),
                    _ => None,
                };
                if let Some(field) = missing {
                    return Ok(err_json("bad_request", &format!("{field} 不能为空")));
                }
            }
            cmx_agent_im::save_im_config(&data_dir, &cfg)?;            let note = if cmx_agent_im::env_active() {
                "已保存。⚠ 检测到环境变量 CMX_AGENT_IM_*（开发模式）优先生效，im.json 暂不生效。"
            } else {
                "已保存，重启应用后生效"
            };
            Ok(serde_json::json!({ "ok": true, "data": { "note": note } }).to_string())
        }
        "test" => {
            let v: serde_json::Value = payload
                .and_then(|p| serde_json::from_str(&p).ok())
                .ok_or_else(|| "invalid payload".to_string())?;
            let get_str = |k: &str| {
                v.get(k)
                    .and_then(|x| x.as_str())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            };
            // 面板密文未解锁 → 用 im.json 已存 secret 测；解锁输入了新值 → 用新值测。
            let saved = cmx_agent_im::load_im_config(&data_dir);
            let app_id = get_str("app_id").or_else(|| {
                saved.as_ref().map(|c| c.feishu.app_id.trim().to_string()).filter(|s| !s.is_empty())
            });
            let secret = get_str("app_secret").or_else(|| {
                saved.as_ref().map(|c| c.feishu.app_secret.clone()).filter(|s| !s.trim().is_empty())
            });
            let base = get_str("base").or_else(|| {
                saved.as_ref().map(|c| c.feishu.base.trim().to_string()).filter(|s| !s.is_empty())
            });
            let (Some(app_id), Some(secret)) = (app_id, secret) else {
                return Ok(err_json("bad_request", "App ID / App Secret 不能为空（先填写或保存）"));
            };
            // 网络走专用 net_rt（与 dispatch_on_net 同范式，reqwest 不落在 Tauri 运行时上）。
            let out = tauri::async_runtime::spawn_blocking(move || {
                net_rt().block_on(cmx_agent_im::test_feishu(&app_id, &secret, base.as_deref()))
            })
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string());
            Ok(serde_json::json!({ "ok": true, "data": { "result": out } }).to_string())
        }
        other => Ok(err_json("bad_request", &format!("未知 action：{other}"))),
    }
}

/// 统一错误信封（与 AppResponse::err 同形，前端 call 风格解析）。
fn err_json(code: &str, message: &str) -> String {
    serde_json::json!({ "ok": false, "error": { "code": code, "message": message } }).to_string()
}
/// 应用状态：一个共享的 AgentApp（认证态用内部 Mutex，可跨命令共享）。
struct AppState {
    app: Arc<AgentApp>,
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

/// 启动守卫（主窗口前端调用）：若当前**未认证**，则隐藏主窗口、显示并聚焦登录窗口。
///
/// 登录门是「每次启动先见登录窗」的既定行为；但主窗口的 `index.html` 启动即渲染、不检查登录态，
/// 而 `visible:false` 的配置在部分平台/场景下不保证前面隐藏。此命令把启动行为与登出行为对齐：
/// 未登录 → 回到登录窗（同 `logout_to_login`）。前端在 `refreshUser()` 拉到空用户时调用。
#[tauri::command]
fn guard_login(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.app.is_authenticated() {
        return Ok(()); // 已登录：保持主窗口
    }
    // 未登录：隐藏主窗口，重建/显示登录窗。
    if let Some(login_win) = app.get_webview_window("login") {
        let _ = login_win.show();
        let _ = login_win.set_focus();
    } else {
        tauri::WebviewWindowBuilder::new(
            &app,
            "login",
            tauri::WebviewUrl::App("login.html".into()),
        )
        .title("登录 · cmx 企业桌面智能体")
        .inner_size(980.0, 640.0)
        .resizable(false)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    Ok(())
}

/// 登录命令（登录窗口专用）：校验凭据 → **成功则显示主窗口并关闭登录窗**，返回用户 JSON；失败返回错误文案。
///
/// 这正是「登录成功后打开现有窗口」：主窗口在启动时已创建但 `visible:false`，此处仅 `show()` 之。
#[tauri::command]
async fn login(
    app: tauri::AppHandle,
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
        eprintln!("[login] ok; revealing main window");
        // 显示已存在的主窗口 + 关闭登录窗。通知主窗口刷新用户信息。
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
            let _ = main.set_focus();
            let _ = main.emit("logged-in", user.clone());
        }
        if let Some(login_win) = app.get_webview_window("login") {
            let _ = login_win.close();
        }
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

/// 登出并回到登录窗（主窗口菜单「退出登录」调用）：清认证态 → 隐藏主窗 → 重建/显示登录窗。
#[tauri::command]
async fn logout_to_login(app: tauri::AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    state.app.logout();
    if let Some(login_win) = app.get_webview_window("login") {
        let _ = login_win.show();
        let _ = login_win.set_focus();
    } else {
        tauri::WebviewWindowBuilder::new(
            &app,
            "login",
            tauri::WebviewUrl::App("login.html".into()),
        )
        .title("登录 · cmx 企业桌面智能体")
        .inner_size(980.0, 640.0)
        .resizable(false)
        .center()
        .build()
        .map_err(|e| e.to_string())?;
    }
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }
    Ok(())
}

fn build_app() -> AgentApp {
    // 双壳统一数据根（与 Web 壳同一份：model.json / 会话共享；CMX_AGENT_DATA_DIR 可覆盖，隔离测试用）。
    let data_dir = cmx_agent_app::shared_data_dir();
    let workdir = data_dir.join("workspace");
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
        .auth(AuthConfig::default()) // 登录门：对接门户 :8080 /api/auth
        .interactive_approval() // X4：shell 等需审批工具挂起等前端点按
        .mcp_tools(mcp_tools)   // U3：外部 MCP 工具
        // U13：opt-in 数据权限接地——env CMX_AGENT_DATAAUTH_URL 指向 cmx-data-auth 即启用真 PEP。
        .maybe_data_auth(std::env::var("CMX_AGENT_DATAAUTH_URL").ok())
        .build()
        .expect("build agent app");

    // IM 绑定面板（设置 → IM 绑定）：gen_code/list/unbind 三命令（经 dispatch_json）。
    // 门户基址与登录门同源（AuthConfig::default().base_url），CMX_AGENT_PORTAL_BASE 可覆盖。
    let portal_base = std::env::var("CMX_AGENT_PORTAL_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| AuthConfig::default().base_url);
    app = app.with_im_binding(cmx_agent_app::ImBindingClient::new(portal_base));

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
        let app = Arc::new(app);
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
        let app = Arc::new(app);
        let payload =
            serde_json::json!({"cmd":"login","username":user,"password":pass}).to_string();
        let out = net_rt().block_on(async move { dispatch_json(&app, &payload).await });
        println!("{out}");
        return;
    }

    // 预热专用运行时（在 Tauri 运行时启动前构建，确保不在任何 tokio 上下文内创建）。
    let _ = net_rt();
    eprintln!("[main] net_rt ready; launching login window (main hidden until login)");

    let app = Arc::new(app);

    // U16：IM 遥控（飞书/微信/钉钉…）。按 env 装配——配了 CMX_AGENT_IM_KIND 等就启动 ImBridge
    // 后台 task（与 CLI `im` 模式同一套装配），与桌面 UI 共用同一个 AgentApp：IM 消息跑同一回合循环，
    // 事件经 SessionEventBus → session_event Tauri 事件实时推前端，回复经 ImBridge 回发 IM。
    // 未配 IM env → 跳过，桌面壳照常纯本地用。错误只打日志不阻断 UI（IM 是可选遥控通道）。
    start_im_if_configured(net_rt(), Arc::clone(&app));

    tauri::Builder::default()
        .manage(AppState { app })
        .invoke_handler(tauri::generate_handler![agent, login, logout_to_login, guard_login, send_stream, platform, im_config])
        // U16：会话事件总线 → 前端实时通道。常驻 task 订阅 AgentApp 的 event_bus，把任意来源
        //（本地 / IM 桥 / 后续 webhook）的会话事件 emit 成全局 `session_event` Tauri 事件。
        // 前端 `index.html` 监听它，按 session_id 分流渲染——实现「飞书发消息实时显示到对话界面」，
        // 且 IM 无关：微信/钉钉接入后走同一条路，零额外改动。
        // 同时设置 cmx 图标（Linux 任务栏/标题栏；bundle.icon 仅打包时生效）。
        .setup(|app| {
            let app_state = app.state::<AppState>();
            let app_ref = app_state.app.clone();
            let handle = app.handle().clone();
            net_rt().spawn(async move {
                let mut rx = app_ref.event_bus().subscribe();
                while let Ok(env) = rx.recv().await {
                    eprintln!("[bus] emit session_event session={} kind={}", env.session_id, serde_json::to_string(&env.event.kind).unwrap_or_default());
                    let _ = handle.emit("session_event", env);
                }
                eprintln!("[main] session_event 转发 task 结束（总线已关闭）");
            });
            let img = tauri::include_image!("icons/icon.png");
            for (_, win) in app.webview_windows() {
                let _ = win.set_icon(img.clone());
            }
            Ok(())
        })
        // 登录门守卫：未登录时关闭登录窗 = 退出应用（否则只剩隐藏的主窗，界面像卡死）。
        .on_window_event(|window, event| {
            if window.label() == "login" {
                if let tauri::WindowEvent::CloseRequested { .. } = event {
                    let authed = window.state::<AppState>().app.is_authenticated();
                    if !authed {
                        window.app_handle().exit(0);
                    }
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("run tauri app");
}
