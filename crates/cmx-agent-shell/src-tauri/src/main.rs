//! cmx-agent 原生 Tauri 桌面壳入口。
//!
//! 「同核多壳」：本文件把 Tauri 的 invoke 边界接到 cmx-agent-app 的 `dispatch_json`——无业务逻辑。
//! 业务全在已测的 cmx-agent-app（façade / JSONL 落库 / 前门协议 / 五层守卫），与 Web 壳共用同一核。
//! 前端 `crates/cmx-agent-shell/ui/index.html` 里 `call()` 检测 `window.__TAURI__` 走 invoke，否则回退 HTTP。

use std::sync::Arc;
use std::sync::OnceLock;

use cmx_agent_app::{dispatch_json, AgentApp, DesktopAppBuilder};
use tauri::State;

/// 应用状态：一个共享的 AgentApp（无内部可变状态，可跨命令共享）。
struct AppState {
    app: Arc<AgentApp>,
}

/// 专用 tokio 运行时（`enable_all` = io + time driver），供连接器 reqwest 等网络工具可靠运行。
///
/// **为何需要**：Tauri 命令跑在 Tauri 自身的异步运行时上；实践中在其上直接跑 reqwest 会挂起
/// （运行时驱动/上下文不匹配）。把走网络的 `dispatch_json` 放到本自建 `enable_all` 运行时上执行，
/// 经 oneshot 把结果桥回 Tauri 命令，彻底规避该问题。纯文件命令（会话增删查）也一并走这里，行为一致。
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

/// 唯一暴露给前端的命令：把一串 JSON 请求交给核派发，回一串 JSON 响应。
/// 前端：`window.__TAURI__.core.invoke("agent", { payload: JSON.stringify({cmd:"send",...}) })`。
///
/// 执行路径 = 自检验证过的那条：`spawn_blocking`（脱离 Tauri 异步驱动线程）+ `net_rt().block_on`
/// （在专用 enable_all 运行时上跑，reqwest 可用）。带 eprintln 诊断（写入 /tmp/cmx-tauri.log）。
#[tauri::command]
async fn agent(payload: String, state: State<'_, AppState>) -> Result<String, String> {
    let app = state.app.clone();
    let preview: String = payload.chars().take(80).collect();
    eprintln!("[agent] invoke received: {preview}");
    let result = tauri::async_runtime::spawn_blocking(move || {
        net_rt().block_on(async move { dispatch_json(&app, &payload).await })
    })
    .await;
    match result {
        Ok(out) => {
            eprintln!("[agent] done, {} bytes", out.len());
            Ok(out)
        }
        Err(e) => {
            eprintln!("[agent] JOIN ERROR: {e}");
            Err(format!("agent task failed: {e}"))
        }
    }
}

fn main() {
    // 数据目录：平台标准位置（macOS: ~/Library/Application Support/com.pansoft.cmx-agent/）。
    let dirs = directories::ProjectDirs::from("com", "pansoft", "cmx-agent")
        .expect("resolve project dirs");
    let data_dir = dirs.data_dir().to_path_buf();
    // 工作区（沙箱根）：数据目录下的 workspace/。真机可让用户改选。
    let workdir = data_dir.join("workspace");
    std::fs::create_dir_all(&workdir).ok();

    // M2 演示：关键词路由模型（识别「列出流程/对象/报表」→ 真调 cmx 连接器）。真实模型缝在此替换，壳不变。
    let model = Arc::new(cmx_agent_app::DemoModel);

    let app = DesktopAppBuilder::new(workdir, data_dir, model)
        .connectors(cmx_agent_app::ConnectorConfig::default())
        .build()
        .expect("build agent app");

    // 自检模式：`cmx-agent-shell --selftest` 不开窗，直接在 net_rt 上跑一次 list_connectors，打印结果。
    // 用于把「Tauri 运行时问题（已修）」与「网络不可达/权限问题（需另修）」区分开——不必点窗口。
    if std::env::args().any(|a| a == "--selftest") {
        let app = Arc::new(app);
        let out = net_rt().block_on(async move {
            dispatch_json(&app, r#"{"cmd":"list_connectors"}"#).await
        });
        println!("{out}");
        return;
    }

    // 预热专用运行时（在 Tauri 运行时启动前构建，确保不在任何 tokio 上下文内创建）。
    let _ = net_rt();
    eprintln!("[main] net_rt ready; launching window");

    tauri::Builder::default()
        .manage(AppState { app: Arc::new(app) })
        .invoke_handler(tauri::generate_handler![agent])
        .run(tauri::generate_context!())
        .expect("run tauri app");
}
