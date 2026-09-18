//! cmx-agent Web 桌面壳（离线可跑的桌面界面）。
//!
//! 「同核多壳」：本壳与 Tauri 壳调**同一个** `cmx-agent-app::dispatch_json`——只是把边界从 Tauri invoke
//! 换成 HTTP POST /api。启动后用 Chrome `--app=` 模式打开一个**无浏览器边框的独立窗口**，观感即桌面 App。
//! 待 tauri 联网装好后，可无缝换成原生 WebView 壳（业务零改动）。
//!
//! 用法：`cargo run --offline -p cmx-agent-web`（自动开窗）；`--no-open` 只起服务不开窗。
//! 端口固定 8099（引擎段 8091-8098 / launcher 8100 之外的空位，联调可预期）；
//! `--port N` 或 `CMX_AGENT_WEB_PORT` 可覆盖；端口被占用（如双开实例）回退随机端口。

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::{Html, IntoResponse};
use axum::routing::{get, post};
use cmx_agent_app::{AgentApp, DesktopAppBuilder, dispatch_json};

/// 默认固定端口：与各引擎/门户/launcher 的已占段错开，双壳/浏览器书签/代理配置都好写死。
const DEFAULT_PORT: u16 = 8099;

/// 前端单页 SPA（内嵌，css/js 拆分为静态资源经 include_bytes! 内嵌）。
const INDEX_HTML: &str = include_str!("../ui/index.html");
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
    let want_port = resolve_port(
        &std::env::args().collect::<Vec<_>>(),
        std::env::var("CMX_AGENT_WEB_PORT").ok().as_deref(),
    );

    // 数据根 & 工作目录：与 Tauri 壳同一数据根（ProjectDirs，CMX_AGENT_DATA_DIR 可覆盖）——
    // model.json / 会话双壳共享，配置一次两壳生效。此前落 %TEMP% 会被清临时目录连坐丢失。
    let data_dir = cmx_agent_app::shared_data_dir();
    let workdir = data_dir.join("workspaces").join("default");
    std::fs::create_dir_all(&workdir).expect("create workdir");

    let app = build_app(&workdir, &data_dir).await;
    let state = AppState { app: app.into_shared() };

    let router = Router::new()
        .route("/", get(index))
        .route("/cmx.png", get(cmx_png))
        .route("/css/{*path}", get(css_asset))
        .route("/js/{*path}", get(js_asset))
        .route("/api", post(api))
        .route("/api/stream", post(api_stream))
        .route("/api/subscribe", get(sse_subscribe))
        .route("/health", get(|| async { "ok" }))
        .with_state(state)
        .layer(axum::middleware::from_fn(loopback_guard));

    // 绑定固定端口（默认 8099）：被占用（如双开实例）时告警并回退随机端口，保证总能打开界面。
    let listener = match tokio::net::TcpListener::bind(("127.0.0.1", want_port)).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!("固定端口 {want_port} 被占用（{e}），回退随机端口。可用 --port / CMX_AGENT_WEB_PORT 换一个。");
            tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind loopback")
        }
    };
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
    let mut mcp_tools = cmx_agent_mcp::load_and_connect(&data_dir.join("mcp.json")).await;
    // U15：plugins 目录里 kind:"mcp" 的插件清单，也作为 MCP server 连上（统一插件面入口）。
    mcp_tools.extend(cmx_agent_plugin::connect_mcp_plugins(&data_dir.join("plugins")).await);
    // U13：opt-in 数据权限接地——设 CMX_AGENT_DATAAUTH_URL 指向 cmx-data-auth 即启用真 PEP；否则 allow_all 占位。
    // 门户基址：CMX_AGENT_PORTAL_BASE 可覆盖（登录门与 IM 绑定同源；默认团队门户）。
    // 原实现只覆盖了 IM 绑定、登录门写死 default（注释声称「可覆盖」与行为不符）——本次修正。
    let portal_base = std::env::var("CMX_AGENT_PORTAL_BASE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cmx_agent_app::AuthConfig::default().base_url);
    let mut app = DesktopAppBuilder::new(workdir, data_dir, model)
        .connectors(cmx_agent_app::ConnectorConfig::default())
        .auth(cmx_agent_app::AuthConfig { base_url: portal_base.clone() })
        // 登录页「注册账号」入口显隐（env CMX_AGENT_REGISTER_ENABLED，缺省展示；成败由门户 whitelist 决定）。
        .with_register_enabled(register_enabled_from_env())
        .interactive_approval() // X4：shell 等需审批工具挂起等前端点按
        .mcp_tools(mcp_tools) // U3：外部 MCP 工具
        // U15 插件市场：配 env 则拉远程目录（未配回落 env/无市场，见 builder 双保险）。
        .plugin_market(std::env::var("CMX_AGENT_PLUGIN_MARKET").ok())
        .maybe_data_auth(std::env::var("CMX_AGENT_DATAAUTH_URL").ok())
        .user_config_base(data_dir.join("users")) // per-user 模型配置：<data_dir>/users/<username>/model.json
        .build()
        .expect("build agent app");

    // IM 绑定面板（设置 → IM 绑定）：不注入则绑定三命令一律报「未启用 IM 绑定」。
    app = app.with_im_binding(cmx_agent_app::ImBindingClient::new(portal_base));

    app
}

/// 登录页注册入口显隐：env `CMX_AGENT_REGISTER_ENABLED`（"false"/"0"/"off"/"no" = 隐藏，缺省展示）。
fn register_enabled_from_env() -> bool {
    match std::env::var("CMX_AGENT_REGISTER_ENABLED") {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "false" | "0" | "off" | "no"
        ),
        Err(_) => true,
    }
}

/// 环回守卫：Host 必须是本机回环；带 Origin 头（浏览器跨站场景）必须是回环源。
/// 此前 /api 零校验——任意网页可用跨站 text/plain 简单请求（无预检）驱动本机 agent
/// （set_policy / add_local_workspace / send 等）。同时给所有响应补 CSP，
/// 与 Tauri 壳对齐（script-src 不放 unsafe-inline）。
async fn loopback_guard(
    req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let is_loopback = |s: &str| {
        s.starts_with("127.0.0.1") || s.starts_with("localhost") || s.starts_with("[::1]")
    };
    let host_ok = req
        .headers()
        .get(axum::http::header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(is_loopback)
        .unwrap_or(false);
    // 无 Origin = 非浏览器客户端（curl / 地址栏导航），放行；有则必须回环同源。
    let origin_ok = req
        .headers()
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|o| is_loopback(o.trim_start_matches("http://")) || is_loopback(o))
        .unwrap_or(true);
    if host_ok && origin_ok {
        let mut resp = next.run(req).await;
        resp.headers_mut().insert(
            axum::http::header::CONTENT_SECURITY_POLICY,
            axum::http::HeaderValue::from_static(
                "default-src 'self'; style-src 'self' 'unsafe-inline'; script-src 'self'; img-src 'self' data:; frame-src 'self' https: http://127.0.0.1:* http://localhost:*",
            ),
        );
        return resp;
    }
    tracing::warn!("已拒绝非回环请求（Host/Origin 校验失败）");
    (
        axum::http::StatusCode::FORBIDDEN,
        "forbidden: loopback only",
    )
        .into_response()
}

async fn index() -> impl IntoResponse {
    no_cache(Html(INDEX_HTML).into_response())
}

/// 静态资产禁缓存：ui 真源经 include_bytes! 烧进二进制，重启即换内容——缓存旧 JS/CSS 会
/// 让「改了没生效」假象（旧实现无缓存头，浏览器启发式缓存曾强逼用户手动强刷）。
/// `no-cache` = 可存但每次回源校验；无 ETag/Last-Modified 时等价于每次拉新。
fn no_cache(mut resp: axum::response::Response) -> axum::response::Response {
    resp.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-cache"),
    );
    resp
}

async fn cmx_png() -> impl IntoResponse {
    no_cache(
        (
            [(axum::http::header::CONTENT_TYPE, "image/png")],
            CMX_PNG,
        )
            .into_response(),
    )
}

/// 静态 CSS 资源路由：path 匹配到 include_bytes! 内嵌内容，Content-Type text/css。
async fn css_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    let body: &[u8] = match path.as_str() {
        "tokens.css" => include_bytes!("../ui/css/tokens.css"),
        "layout.css" => include_bytes!("../ui/css/layout.css"),
        "modals.css" => include_bytes!("../ui/css/modals.css"),
        "chat.css" => include_bytes!("../ui/css/chat.css"),
        "panels.css" => include_bytes!("../ui/css/panels.css"),
        "login.css" => include_bytes!("../ui/css/login.css"),
        "dropdown.css" => include_bytes!("../ui/css/dropdown.css"),
        "settings.css" => include_bytes!("../ui/css/settings.css"),
        _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };
    no_cache(
        (
            [(axum::http::header::CONTENT_TYPE, "text/css")],
            body,
        )
            .into_response(),
    )
}

/// 静态 JS 资源路由：path 匹配到 include_bytes! 内嵌内容，Content-Type application/javascript。
async fn js_asset(
    axum::extract::Path(path): axum::extract::Path<String>,
) -> impl IntoResponse {
    let body: &[u8] = match path.as_str() {
        "bridge.js" => include_bytes!("../ui/js/bridge.js"),
        "tabs.js" => include_bytes!("../ui/js/tabs.js"),
        "markdown.js" => include_bytes!("../ui/js/markdown.js"),
        "render.js" => include_bytes!("../ui/js/render.js"),
        "dropdown.js" => include_bytes!("../ui/js/dropdown.js"),
        "session.js" => include_bytes!("../ui/js/session.js"),
        "panels.js" => include_bytes!("../ui/js/panels.js"),
        "model.js" => include_bytes!("../ui/js/model.js"),
        "workspace.js" => include_bytes!("../ui/js/workspace.js"),
        "im.js" => include_bytes!("../ui/js/im.js"),
        "vendor/qrcode.min.js" => include_bytes!("../ui/js/vendor/qrcode.min.js"),
        "login.js" => include_bytes!("../ui/js/login.js"),
        "update.js" => include_bytes!("../ui/js/update.js"),
        "settings.js" => include_bytes!("../ui/js/settings.js"),
        "agents.js" => include_bytes!("../ui/js/agents.js"),
        "platform.js" => include_bytes!("../ui/js/platform.js"),
        "main.js" => include_bytes!("../ui/js/main.js"),
        _ => return axum::http::StatusCode::NOT_FOUND.into_response(),
    };
    no_cache(
        (
            [(
                axum::http::header::CONTENT_TYPE,
                "application/javascript",
            )],
            body,
        )
            .into_response(),
    )
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
async fn api_stream(State(state): State<AppState>, body: String) -> axum::response::Response {
    // 前门硬登录门（红蓝审查 P1-1）：本端点直调 send_streaming、不经 dispatch_json 的统一
    // 登录门，旧实现未登录也能驱动回合跑工具。401 由前端 streamSend 的 r.ok 检查合成
    // stream_error 呈现。
    if state.app.auth_configured() && !state.app.is_authenticated() {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
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
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// 会话事件总线订阅（U16 落地，红蓝审查 P1-2）：`GET /api/subscribe` SSE 把
/// `SessionEventBus` 的所有会话事件（任意来源：本地 / IM 桥 / 后台子任务回执）实时推给前端，
/// 前端 EventSource 按 session_id 分流到对应 tab。此前只有 Tauri 壳有 session_event 实时通路，
/// Web 壳事件不落库就不可见（后台回执「没返回」的根因）。Lagged 跳过续收（历史兜底可重拉）。
async fn sse_subscribe(State(state): State<AppState>) -> axum::response::Response {
    // 前门硬登录门（与 /api/stream 同规）：事件流含全部会话内容，未登录不得订阅。
    if state.app.auth_configured() && !state.app.is_authenticated() {
        return axum::http::StatusCode::UNAUTHORIZED.into_response();
    }
    use axum::response::sse::{Event, KeepAlive, Sse};
    use tokio::sync::broadcast::error::RecvError;
    use tokio_stream::wrappers::UnboundedReceiverStream;
    use tokio_stream::StreamExt;

    let mut rx = state.app.event_bus().subscribe();
    let (tx, rx_out) = tokio::sync::mpsc::unbounded_channel::<Event>();
    // 转发 task：broadcast → SSE 帧。客户端断开（tx send 失败）或总线关闭即退出。
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(env) => {
                    let data = serde_json::to_string(&env).unwrap_or_default();
                    if tx.send(Event::default().data(data)).is_err() {
                        break;
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::warn!("[subscribe] 订阅落后 {n} 帧，已跳过（前端可重开会话补齐）");
                }
                Err(RecvError::Closed) => break,
            }
        }
    });
    let stream = UnboundedReceiverStream::new(rx_out)
        .map(Ok::<Event, std::convert::Infallible>);
    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// 端口解析：`--port N` 参数 > `CMX_AGENT_WEB_PORT` 环境变量 > 默认 [`DEFAULT_PORT`]。
/// 非法值不报错，回退下一优先级（桌面应用启动路径宁可用默认端口也别崩）。
fn resolve_port(args: &[String], env: Option<&str>) -> u16 {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let raw = if a == "--port" {
            it.next().map(String::as_str)
        } else {
            a.strip_prefix("--port=")
        };
        if let Some(p) = raw.and_then(|v| v.trim().parse::<u16>().ok()) {
            return p;
        }
    }
    if let Some(p) = env.and_then(|s| s.trim().parse::<u16>().ok()) {
        return p;
    }
    DEFAULT_PORT
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn workspace_ui_script_is_embedded_and_served() {
        let response = js_asset(axum::extract::Path("workspace.js".to_string()))
            .await
            .into_response();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        assert!(!body.is_empty());
        let source = String::from_utf8_lossy(&body);
        assert!(source.contains("function initWorkspaceUI()"));
    }

    #[test]
    fn resolve_port_prefers_flag_then_env_then_default() {
        let args = |v: &[&str]| -> Vec<String> { v.iter().map(|s| (*s).to_string()).collect() };
        // 参数两种写法
        assert_eq!(resolve_port(&args(&["prog", "--port", "9001"]), None), 9001);
        assert_eq!(resolve_port(&args(&["prog", "--port=9002"]), None), 9002);
        // 环境变量（含空白容忍）
        assert_eq!(resolve_port(&args(&["prog"]), Some(" 9003 ")), 9003);
        // 缺省固定端口
        assert_eq!(resolve_port(&args(&["prog"]), None), DEFAULT_PORT);
        assert_eq!(DEFAULT_PORT, 8099);
        // 非法值回退下一优先级
        assert_eq!(resolve_port(&args(&["prog", "--port", "abc"]), None), DEFAULT_PORT);
        assert_eq!(resolve_port(&args(&["prog", "--port", "abc"]), Some("9004")), 9004);
        assert_eq!(resolve_port(&args(&["prog"]), Some("bad")), DEFAULT_PORT);
        // `--port` 缺值也回退
        assert_eq!(resolve_port(&args(&["prog", "--port"]), None), DEFAULT_PORT);
    }
}
