//! cmx-agent 原生 Tauri 桌面壳入口。
//!
//! 「同核多壳」：本文件把 Tauri 的 invoke 边界接到 cmx-agent-app 的 `dispatch_json`——无业务逻辑。
//! 业务全在已测的 cmx-agent-app（façade / JSONL 落库 / 前门协议 / 五层守卫 / 认证），与 Web 壳共用同一核。
//! 前端 `ui/index.html` 里 `call()` 检测 `window.__TAURI__` 走 invoke，否则回退 HTTP。
//!
//! 登录门（参照 CMXPortalManager 的登录方式）：启动时**主窗口隐藏**、只显示 `login` 窗口；`login.html`
//! 提交 → `invoke("login")` → 校验凭据（对接门户 /api/auth/login）→ **成功后显示已存在的主窗口并关闭登录窗**。

use std::sync::Arc;
use std::sync::OnceLock;

use cmx_agent_app::{AgentApp, AuthConfig, DesktopAppBuilder, dispatch_json};
use tauri::{Emitter, Manager, State};

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
    let dirs = directories::ProjectDirs::from("com", "pansoft", "cmx-agent")
        .expect("resolve project dirs");
    let data_dir = dirs.data_dir().to_path_buf();
    let workdir = data_dir.join("workspace");
    std::fs::create_dir_all(&workdir).ok();

    // E0：按 env / <data_dir>/model.json 选真实模型（OpenAI 兼容：DeepSeek/OpenAI/Qwen/本地）或回退 DemoModel。
    let model = cmx_agent_app::select_model(Some(&data_dir));

    // U3：从 <data_dir>/mcp.json 连接外部 MCP server（sync main → 在 net_rt 上跑异步连接）。
    let mcp_path = data_dir.join("mcp.json");
    let mcp_tools = net_rt().block_on(async move { cmx_agent_mcp::load_and_connect(&mcp_path).await });

    DesktopAppBuilder::new(workdir, data_dir, model)
        .connectors(cmx_agent_app::ConnectorConfig::default())
        .auth(AuthConfig::default()) // 登录门：对接门户 :8080 /api/auth
        .interactive_approval() // X4：bash 等需审批工具挂起等前端点按
        .mcp_tools(mcp_tools)   // U3：外部 MCP 工具
        .build()
        .expect("build agent app")
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

    tauri::Builder::default()
        .manage(AppState { app: Arc::new(app) })
        .invoke_handler(tauri::generate_handler![agent, login, logout_to_login, send_stream])
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
