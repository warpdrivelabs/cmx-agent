//! cmx-agent Web 桌面壳（离线可跑的桌面界面）。
//!
//! 「同核多壳」：本壳与 Tauri 壳调**同一个** `cmx-agent-app::dispatch_json`——只是把边界从 Tauri invoke
//! 换成 HTTP POST /api。启动后用 Chrome `--app=` 模式打开一个**无浏览器边框的独立窗口**，观感即桌面 App。
//! 待 tauri 联网装好后，可无缝换成原生 WebView 壳（业务零改动）。
//!
//! 用法：`cargo run --offline -p cmx-agent-web`（自动开窗）；`--no-open` 只起服务不开窗。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::extract::Request;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use cmx_agent_app::{AgentApp, DesktopAppBuilder, dispatch_json};

// 新前端内嵌资产表（build.rs 生成，方案 §10.2）。当前生产用旧 UI（用户要求先回旧版），
// 新前端挂 /next 随时可切（build.rs fail-fast 同时保证 frontend/dist 不缺位）。
include!(concat!(env!("OUT_DIR"), "/ui_assets.rs"));

/// 旧前端单页（内嵌）：当前生产入口。
const INDEX_HTML: &str = include_str!("../ui/index.html");
/// 旧登录页（内嵌）：双窗口登录门配套。
const LOGIN_HTML: &str = include_str!("../ui/login.html");
/// cmx 品牌图标。
const CMX_PNG: &[u8] = include_bytes!("../../../images/cmx.png");

#[derive(Clone)]
struct AppState {
    app: Arc<AgentApp>,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let no_open = std::env::args().any(|a| a == "--no-open");

    // 数据根 & 工作区（沙箱根）：与 Tauri 壳同一数据根（ProjectDirs，CMX_AGENT_DATA_DIR 可覆盖）——
    // model.json / 会话双壳共享，配置一次两壳生效。此前落 %TEMP% 会被清临时目录连坐丢失。
    let data_dir = cmx_agent_app::shared_data_dir();
    let workdir = data_dir.join("workspace");
    std::fs::create_dir_all(&workdir).expect("create workdir");

    let app = build_app(&workdir, &data_dir).await;
    // 会话回放：本地 auth.json 有效则恢复登录态（浏览器打开即已登录，免每次访问先登录页）。
    let restored = app.try_restore_session().await;
    tracing::info!(
        "会话回放：{}",
        if restored { "成功，跳过登录页" } else { "无会话/已失效，走登录页" }
    );
    let state = AppState { app: Arc::new(app) };

    let router = Router::new()
        .route("/", get(old_index))
        .route("/login", get(old_login_page))
        .route("/cmx.png", get(cmx_png))
        .route("/api", post(api))
        .route("/api/stream", post(api_stream))
        .route("/api/subscribe", get(api_subscribe))
        .route("/health", get(|| async { "ok" }))
        // 无尾斜杠时相对路径 ./assets/* 会解析到根 /assets/*（404 白屏）——先重定向到 /next/
        .route("/next", get(|| async { axum::response::Redirect::to("/next/") }))
        .route("/next/", get(next_ui))
        .fallback(next_assets)
        .with_state(state);

    // 绑定地址：CMX_AGENT_WEB_BIND 指定固定地址（开发态 Vite proxy 用），未设随机端口（本机独占）。
    let bind_addr = std::env::var("CMX_AGENT_WEB_BIND").unwrap_or_else(|_| "127.0.0.1:0".to_string());
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("bind {bind_addr} 失败：{e}"));
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}/");
    tracing::info!("cmx-agent 桌面界面已就绪：{url}");
    println!("\n  TrueMate · cmx 企业桌面智能体 Web 壳\n  ▶ {url}\n");

    if !no_open {
        open_desktop_window(&url);
    } else {
        println!("  (--no-open：仅起服务，未开窗。用浏览器访问上面地址即可。)\n");
    }

    axum::serve(listener, router).await.expect("serve");
}

async fn build_app(workdir: &std::path::Path, data_dir: &std::path::Path) -> AgentApp {
    // E0：按 env / <data_dir>/model.json 选真实模型（OpenAI 兼容：DeepSeek/OpenAI/Qwen/本地）或回退 DemoModel。
    // 真实模型缝在 cmx-agent-model，壳与前端不变；识别「列出流程/对象/报表」等仍走连接器工具。
    let model = cmx_agent_app::select_model(Some(data_dir));
    // U3：从 <data_dir>/mcp.json 连接外部 MCP server（opt-in，文件不存在则无 MCP 工具）。
    let mut mcp_tools = cmx_agent_mcp::load_and_connect(&data_dir.join("mcp.json")).await;
    // U15：plugins 目录里 kind:"mcp" 的插件清单，也作为 MCP server 连上（统一插件面入口）。
    mcp_tools.extend(cmx_agent_plugin::connect_mcp_plugins(&data_dir.join("plugins")).await);
    // U13：opt-in 数据权限接地——设 CMX_AGENT_DATAAUTH_URL 指向 cmx-data-auth 即启用真 PEP；否则 allow_all 占位。
    let mut app = DesktopAppBuilder::new(workdir, data_dir, model)
        .connectors(cmx_agent_app::ConnectorConfig::default())
        .auth(cmx_agent_app::AuthConfig::default())
        .interactive_approval() // X4：shell 等需审批工具挂起等前端点按
        .mcp_tools(mcp_tools) // U3：外部 MCP 工具
        .maybe_data_auth(std::env::var("CMX_AGENT_DATAAUTH_URL").ok())
        .user_config_base(data_dir.join("users")) // per-user 模型配置：<data_dir>/users/<username>/model.json
        .build()
        .expect("build agent app");

    // IM 绑定面板（设置 → IM 绑定）：不注入则绑定三命令一律报「未启用 IM 绑定」。
    // 门户基址与登录门同源（AuthConfig::default().base_url），CMX_AGENT_PORTAL_BASE 可覆盖。
    let portal_base = std::env::var("CMX_AGENT_PORTAL_BASE")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| cmx_agent_app::AuthConfig::default().base_url);
    app = app.with_im_binding(cmx_agent_app::ImBindingClient::new(portal_base));

    app
}

async fn old_index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn old_login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

async fn cmx_png() -> impl IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, "image/png")], CMX_PNG)
}

/// 新前端入口（/next，切回时改为 /）。
async fn next_ui() -> impl IntoResponse {
    serve_asset("index.html")
}

/// 新前端静态资产：/next/** 查内嵌表；其余 404（旧 UI 的 /cmx.png 等已显式路由）。
async fn next_assets(req: Request<axum::body::Body>) -> axum::response::Response {
    let path = req.uri().path();
    let Some(rel) = path.strip_prefix("/next/") else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "not found".to_string(),
        )
            .into_response();
    };
    let rel = if rel.is_empty() { "index.html" } else { rel };
    serve_asset(rel)
}

/// 查内嵌资产表返回（hash 文件名原样；命中失败 404）。
fn serve_asset(rel: &str) -> axum::response::Response {
    match UI_ASSETS.iter().find(|a| a.path == rel) {
        Some(asset) => (
            [(axum::http::header::CONTENT_TYPE, asset.mime)],
            asset.bytes,
        )
            .into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            [(axum::http::header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            format!("asset not found: {rel}"),
        )
            .into_response(),
    }
}

/// 唯一 API：前端 POST 一段 JSON 命令，回一段 JSON 响应（= Tauri invoke 边界的 HTTP 版）。
async fn api(State(state): State<AppState>, body: String) -> impl IntoResponse {
    let resp = dispatch_json(&state.app, &body).await;
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        resp,
    )
}

/// 流式会话（办公助手对话）：POST `{session_id,text}` → `text/event-stream`，回合内每个事件
/// （模型消息 / 工具调用 / 工具结果 / 收尾）边产生边推。对齐 cmx-ai 的 SSE 事件流思路。
/// 末尾补发一个 `{"kind":"stream_done"}`（成功）或 `{"kind":"stream_error","message":..}`（失败）。
async fn api_stream(
    State(state): State<AppState>,
    body: String,
) -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio_stream::wrappers::UnboundedReceiverStream;
    use tokio_stream::StreamExt;

    #[derive(serde::Deserialize)]
    struct Req {
        session_id: String,
        text: String,
    }
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    let req: Result<Req, _> = serde_json::from_str(&body);

    let app = state.app.clone();
    tokio::spawn(async move {
        let Ok(req) = req else {
            let _ = tx.send(serde_json::json!({"kind":"stream_error","message":"invalid request json"}));
            return;
        };
        let sink = std::sync::Arc::new(cmx_agent_app::ChannelSink::new(tx.clone()));
        match app.send_streaming(&req.session_id, &req.text, sink).await {
            Ok(_) => {
                let _ = tx.send(serde_json::json!({"kind":"stream_done"}));
            }
            Err(e) => {
                let _ = tx.send(serde_json::json!({"kind":"stream_error","message": e.to_string()}));
            }
        }
    });

    let stream = UnboundedReceiverStream::new(rx)
        .map(|v| Ok::<Event, std::convert::Infallible>(Event::default().data(v.to_string())));
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// 被动实时通道（方案 §7.4）：订阅会话事件总线，任意来源（本地 / IM 桥 / 后续 webhook）
/// 的会话事件按 SSE 推给前端——IM 遥控「飞书发消息实时显示」的 Web 侧通路。
/// 固定资源段、只读长连接，符合新接口规范。断线由 EventSource 自动重连，前端重连后
/// 以 GetEvents 对账兜底，不依赖总线回放。
async fn api_subscribe(State(state): State<AppState>) -> impl IntoResponse {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio_stream::wrappers::UnboundedReceiverStream;
    use tokio_stream::StreamExt;

    // broadcast → mpsc 转发（tokio-stream 的 BroadcastStream 需 sync feature，未启用）：
    // 每订阅者一个转发 task，断连即 drop。
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let mut broadcast_rx = state.app.event_bus().subscribe();
    tokio::spawn(async move {
        loop {
            match broadcast_rx.recv().await {
                Ok(env) => {
                    let data = serde_json::to_string(&env).unwrap_or_default();
                    if tx.send(Event::default().data(data)).is_err() {
                        break; // 订阅者断开
                    }
                }
                // 广播 lagging：跳过（事件已落库，前端重连后 GetEvents 对账兜底）
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break, // 总线关闭
            }
        }
    });
    let stream = UnboundedReceiverStream::new(rx)
        .map(Ok::<Event, std::convert::Infallible>);
    Sse::new(stream).keep_alive(KeepAlive::default())
}

/// 用 Chrome `--app=` 模式开一个无边框独立窗口（观感=桌面 App）。找不到 Chrome 则退回默认浏览器。
fn open_desktop_window(url: &str) {
    #[cfg(target_os = "macos")]
    let chrome = std::path::Path::new("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome");

    #[cfg(target_os = "macos")]
    {
        if chrome.exists() && spawn_chromium_app(chrome, url) {
            return;
        }
        // 退回系统默认打开方式
        let _ = std::process::Command::new("open").arg(url).spawn();
    }

    #[cfg(target_os = "windows")]
    {
        // Chromium 系候选：Chrome 三个标准安装位（64 位 ProgramFiles / 32 位 ProgramFiles(x86) /
        // per-user LocalAppData）优先，Edge 两个 ProgramFiles 兜底；两者 --app= 参数完全一致。
        let candidates = [
            ("ProgramFiles", r"Google\Chrome\Application\chrome.exe"),
            ("ProgramFiles(x86)", r"Google\Chrome\Application\chrome.exe"),
            ("LOCALAPPDATA", r"Google\Chrome\Application\chrome.exe"),
            ("ProgramFiles", r"Microsoft\Edge\Application\msedge.exe"),
            ("ProgramFiles(x86)", r"Microsoft\Edge\Application\msedge.exe"),
        ]
        .into_iter()
        .filter_map(|(env, rel)| {
            std::env::var_os(env).map(|base| std::path::PathBuf::from(base).join(rel))
        })
        .find(|p| p.exists());
        if let Some(chrome) = candidates
            && spawn_chromium_app(&chrome, url)
        {
            return;
        }
        // 退回系统默认浏览器
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}

/// Chromium 系（Chrome/Edge）`--app=` 独立窗：独立临时 profile（不粘用户日常标签页/会话）+ 固定初始
/// 尺寸。返回是否启动成功。
fn spawn_chromium_app(program: &std::path::Path, url: &str) -> bool {
    let profile = std::env::temp_dir().join("cmx-agent-chrome-profile");
    std::process::Command::new(program)
        .arg(format!("--app={url}"))
        .arg(format!("--user-data-dir={}", profile.display()))
        .arg("--window-size=1000,720")
        .arg("--no-first-run")
        .arg("--no-default-browser-check")
        .spawn()
        .is_ok()
}
