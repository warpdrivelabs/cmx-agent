//! cmx-agent 无头前门。两种模式：
//!   1. **demo**（默认）：读一条指令，跑一个回合，把会话事件日志打成 JSONL（M0 冒烟，类比 `codex exec --json`）。
//!   2. **serve**：`cmx-agent serve <data_dir>`——从 stdin 逐行读 JSON 前门命令，经 app 层 `dispatch_json`
//!      派发（**与 Tauri 桌面壳调的同一函数**），逐行回 JSON 响应。这是 Headless/ACP 前门，也是 e2e 入口。
//!
//! 真实模型缝在后续里程碑以 HTTP 客户端注入，前门与内核不变。
//!
//! 用法：
//!   cmx-agent "帮我把 2 和 3 相加"                    # demo：模型脚本化调用 add 工具
//!   echo "现在几点" | cmx-agent                        # demo：从 stdin 读指令
//!   echo '{"cmd":"send","session_id":"s1","text":"算 2+3"}' | cmx-agent serve /tmp/cmx-agent-data

use std::io::{BufRead, Read, Write};
use std::sync::Arc;

use cmx_agent_app::DesktopAppBuilder;
use cmx_agent_core::{
    Agent, ApprovalGuard, ApprovalPolicy, AuthGuard, AutoApprover, GuardPipeline, HighRiskGuard,
    MockModel, ModelResponse, Policy, SandboxMode, Session, ToolCall,
};
use cmx_agent_tools::default_registry;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("info")
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s.as_str()) == Some("serve") {
        return serve(args.get(1).cloned()).await;
    }
    if args.first().map(|s| s.as_str()) == Some("im") {
        return im_mode(args.get(1).cloned()).await;
    }
    demo(args).await
}

/// im 模式：`cmx-agent im [data_dir]`——把 IM 桥接到真实模型 agent，长轮询/长连接遥控。
///
/// 配置经 `CMX_AGENT_IM_*` env（解析见 `cmx_agent_im::ImConfig`）：
/// - `CMX_AGENT_IM_KIND`：provider 类型，默认 `telegram`；可选 `feishu`。
/// - `CMX_AGENT_IM_ALLOW`：逗号分隔的 chat_id 白名单（安全必需）。
///   - 联调抓 chat_id：设 `CMX_AGENT_IM_ALLOW=__probe__`，发条消息，日志会打 `IM 未授权 chat_id=…`。
/// - `CMX_AGENT_IM_NO_ALLOW=1`：显式放开白名单（仅测试/纯内网，生产勿用）。
/// - 各 provider 凭证 env：
///   - Telegram：`CMX_AGENT_IM_TOKEN`（+ 可选 `CMX_AGENT_IM_BASE`）。
///   - 飞书：`CMX_AGENT_IM_FEISHU_APP_ID` + `CMX_AGENT_IM_FEISHU_APP_SECRET`（+ 可选 `_BASE`）。
async fn im_mode(data_dir: Option<String>) -> anyhow::Result<()> {
    let data_dir = data_dir.unwrap_or_else(|| {
        std::env::temp_dir()
            .join("cmx-agent-data")
            .to_string_lossy()
            .into_owned()
    });
    let workdir = std::path::Path::new(&data_dir).join("workspace");
    std::fs::create_dir_all(&workdir)?;

    // 真实模型（DeepSeek 等，按 env/model.json）——IM 遥控要真回答。
    let model = cmx_agent_app::select_model(Some(std::path::Path::new(&data_dir)));
    let app = Arc::new(
        DesktopAppBuilder::new(&workdir, &data_dir, model)
            .build()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?,
    );

    // IM 配置收口：provider 类型 + 白名单 + 凭证，统一由 ImConfig 解析。
    let im_cfg = cmx_agent_im::ImConfig::from_env().map_err(|e| anyhow::anyhow!(e))?;
    let provider = im_cfg.build_provider().map_err(|e| anyhow::anyhow!(e))?;
    let allow = im_cfg.allow;
    tracing::info!("cmx-agent IM provider = {:?}", im_cfg.kind);
    // 飞书 Stream：启动常驻后台 task；Telegram 长轮询：trait 默认 no-op。
    provider.start().await.map_err(|e| anyhow::anyhow!(e))?;
    let bridge = cmx_agent_im::ImBridge::new(app, provider, im_cfg.kind.label(), allow);
    bridge.run().await;
    Ok(())
}

/// serve 模式：stdin JSONL 命令 → app dispatch → stdout JSONL 响应（Headless 前门）。
async fn serve(data_dir: Option<String>) -> anyhow::Result<()> {
    let data_dir = data_dir.unwrap_or_else(|| {
        std::env::temp_dir()
            .join("cmx-agent-data")
            .to_string_lossy()
            .into_owned()
    });
    let workdir = std::path::Path::new(&data_dir).join("workspace");
    std::fs::create_dir_all(&workdir)?;

    // M1 用脚本化 Mock 模型演示；真实模型缝在此替换即可，前门不变。
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "call-1",
            "add",
            serde_json::json!({ "a": 2, "b": 3 }),
        )])
        .with_text("我来算一下。"),
        ModelResponse::text("2 + 3 = 5。已完成。"),
    ]));

    let app = DesktopAppBuilder::new(&workdir, &data_dir, model)
        .build()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let resp = cmx_agent_app::dispatch_json(&app, &line).await;
        writeln!(stdout, "{resp}")?;
        stdout.flush()?;
    }
    Ok(())
}

/// demo 模式（M0 冒烟）：读一条指令，跑一个回合，打印会话事件 JSONL。
async fn demo(args: Vec<String>) -> anyhow::Result<()> {
    let instruction = read_instruction(args);
    let instruction = instruction.trim();

    // M0 演示脚本：模型先调 add(2,3)，看到结果后用一句话收尾。
    // （真实缝在此替换为 HTTP 客户端；前门代码不变。）
    let model = Arc::new(MockModel::new([
        ModelResponse::calls(vec![ToolCall::with_id(
            "call-1",
            "add",
            serde_json::json!({ "a": 2, "b": 3 }),
        )])
        .with_text("我来算一下。"),
        ModelResponse::text("2 + 3 = 5。已完成。"),
    ]));

    let mut guards = GuardPipeline::new();
    guards
        .add(Arc::new(AuthGuard::allow_all()))
        .add(Arc::new(HighRiskGuard))
        .add(Arc::new(ApprovalGuard));

    let policy = Policy {
        sandbox: SandboxMode::WorkspaceWrite,
        approval: ApprovalPolicy::OnRequest,
        ..Default::default()
    };

    let agent = Agent::builder()
        .model(model)
        .tools(default_registry())
        .guards(guards)
        .approver(Arc::new(AutoApprover::approve()))
        .policy(policy)
        .build()?;

    let mut session = Session::new("cli-session")
        .with_system("你是 cmx 企业桌面智能体。安全第一，能用工具就用工具。");

    let outcome = agent.run_turn(&mut session, instruction).await?;

    // 打印会话事件日志（JSONL）——审计/回放的地基
    eprintln!("──── session event log ({} events) ────", session.log.len());
    for ev in session.log.iter() {
        println!("{}", serde_json::to_string(ev)?);
    }
    eprintln!(
        "──── turn #{} ended: {:?}, {} steps ────",
        outcome.turn, outcome.reason, outcome.steps
    );
    if let Some(t) = outcome.final_text {
        eprintln!("final: {t}");
    }
    Ok(())
}

fn read_instruction(args: Vec<String>) -> String {
    if !args.is_empty() {
        return args.join(" ");
    }
    let mut buf = String::new();
    if std::io::stdin().read_to_string(&mut buf).is_ok() && !buf.trim().is_empty() {
        return buf;
    }
    "帮我把 2 和 3 相加".to_string()
}
