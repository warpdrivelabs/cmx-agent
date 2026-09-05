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
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use cmx_agent_app::{AgentApp, DesktopAppBuilder, dispatch_json};

/// 前端单页（内嵌，免额外静态文件依赖）。
const INDEX_HTML: &str = include_str!("../ui/index.html");
/// 登录页（内嵌）。参照 CMXPortalManager 登录方式，对接门户 /api/auth。
const LOGIN_HTML: &str = include_str!("../ui/login.html");
/// cmx 品牌图标（左下按钮）。从项目 images/ 内嵌。
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

    // 数据目录 & 工作区（沙箱根）。放本机标准临时区下，避免污染仓库。
    let base = std::env::temp_dir().join("cmx-agent-desktop");
    let data_dir = base.join("data");
    let workdir = base.join("workspace");
    std::fs::create_dir_all(&workdir).expect("create workdir");

    let app = build_app(&workdir, &data_dir).await;
    let state = AppState { app: Arc::new(app) };

    let router = Router::new()
        .route("/", get(index))
        .route("/login", get(login_page))
        .route("/cmx.png", get(cmx_png))
        .route("/api", post(api))
        .route("/api/stream", post(api_stream))
        .route("/health", get(|| async { "ok" }))
        .with_state(state);

    // 绑定随机可用端口（127.0.0.1，本机独占，不对外）。
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr: SocketAddr = listener.local_addr().expect("local addr");
    let url = format!("http://{addr}/");
    tracing::info!("cmx-agent 桌面界面已就绪：{url}");
    println!("\n  cmx 企业桌面智能体 · Web 壳\n  ▶ {url}\n");

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
    let mcp_tools = cmx_agent_mcp::load_and_connect(&data_dir.join("mcp.json")).await;
    DesktopAppBuilder::new(workdir, data_dir, model)
        .connectors(cmx_agent_app::ConnectorConfig::default())
        .auth(cmx_agent_app::AuthConfig::default())
        .interactive_approval() // X4：bash 等需审批工具挂起等前端点按
        .mcp_tools(mcp_tools)   // U3：外部 MCP 工具
        .build()
        .expect("build agent app")
}

async fn index() -> Html<&'static str> {
    Html(INDEX_HTML)
}

async fn login_page() -> Html<&'static str> {
    Html(LOGIN_HTML)
}

async fn cmx_png() -> impl IntoResponse {
    ([(axum::http::header::CONTENT_TYPE, "image/png")], CMX_PNG)
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

/// 用 Chrome `--app=` 模式开一个无边框独立窗口（观感=桌面 App）。找不到 Chrome 则退回默认浏览器。
fn open_desktop_window(url: &str) {
    #[cfg(target_os = "macos")]
    let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

    #[cfg(target_os = "macos")]
    {
        if std::path::Path::new(chrome).exists() {
            let profile = std::env::temp_dir().join("cmx-agent-chrome-profile");
            let ok = std::process::Command::new(chrome)
                .arg(format!("--app={url}"))
                .arg(format!("--user-data-dir={}", profile.display()))
                .arg("--window-size=1000,720")
                .arg("--no-first-run")
                .arg("--no-default-browser-check")
                .spawn()
                .is_ok();
            if ok {
                return;
            }
        }
        // 退回系统默认打开方式
        let _ = std::process::Command::new("open").arg(url).spawn();
    }

    #[cfg(not(target_os = "macos"))]
    {
        // 其他平台：尽力用系统默认浏览器打开
        #[cfg(target_os = "windows")]
        let _ = std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let _ = std::process::Command::new("xdg-open").arg(url).spawn();
    }
}
