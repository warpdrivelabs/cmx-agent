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
    if args.first().map(|s| s.as_str()) == Some("im-login") {
        return im_login(args.get(1).cloned()).await;
    }
    demo(args).await
}

/// im 模式：`cmx-agent im [data_dir]`——把 IM 桥接到真实模型 agent，长轮询/长连接遥控。
///
/// 配置装配与桌面壳同一收口（`cmx_agent_im::resolve`）：**env 优先**（`CMX_AGENT_IM_*`，
/// 开发联调），回落 `<data_dir>/im.json`（`cmx-agent im-login` 扫码写入的微信凭证在此生效）。
/// - `CMX_AGENT_IM_KIND`：provider 类型，默认 `feishu`；可选 `qq` / `wechat`。
/// - `CMX_AGENT_IM_ALLOW`：逗号分隔的 chat_id 白名单（安全必需）。
///   - 联调抓 chat_id：设 `CMX_AGENT_IM_ALLOW=__probe__`，发条消息，日志会打 `IM 未授权 chat_id=…`。
/// - `CMX_AGENT_IM_NO_ALLOW=1`：显式放开白名单（仅测试/纯内网，生产勿用）。
/// - 各 provider 凭证 env：
///   - 飞书：`CMX_AGENT_IM_FEISHU_APP_ID` + `CMX_AGENT_IM_FEISHU_APP_SECRET`（+ 可选 `_BASE`）。
///   - QQ：`CMX_AGENT_IM_QQ_APP_ID` + `CMX_AGENT_IM_QQ_APP_SECRET`（+ 可选 `_BASE`）。
///   - 微信：`CMX_AGENT_IM_WECHAT_BOT_TOKEN`（+ 可选 `_BASE`；或直接 im-login 写 im.json）。
///
/// 多通道：每个启用通道各起一个桥 task（与桌面壳同构）。个人模式（im.json `personal=true`）
/// 依赖桌面登录态，CLI 无登录窗口——每条消息会回「请先在桌面应用登录」；CLI 联调建议
/// 走 env（绑定模式语义 + 白名单）。
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

    // IM 配置收口：env 优先，回落 im.json（与桌面壳同一装配口）。
    let resolved = cmx_agent_im::resolve(Some(std::path::Path::new(&data_dir)))
        .map_err(|e| anyhow::anyhow!(e))?;
    let labels: Vec<&str> = resolved.channels.iter().map(|c| c.kind.label()).collect();
    tracing::info!("cmx-agent IM provider = {}（来源={}）", labels.join("+"), resolved.source);

    // 绑定解析器（个人模式会被桥短路，仍装配以防模式切换复用代码路径）。
    let portal_base = std::env::var("CMX_AGENT_PORTAL_BASE")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| cmx_agent_app::AuthConfig::default().base_url);
    let allow = resolved.allow.clone();
    let personal = resolved.personal;

    // 每通道一个桥 task：飞书 Stream 先起常驻 task；微信长轮询 start 为 no-op。
    let mut handles = Vec::new();
    for ch in resolved.channels {
        let kind = ch.kind.label();
        if let Err(e) = ch.provider.start().await {
            anyhow::bail!("IM provider[{kind}].start 失败：{e}");
        }
        let bridge = cmx_agent_im::ImBridge::new(Arc::clone(&app), Arc::clone(&ch.provider), kind, allow.clone())
            .with_personal(personal)
            .with_bindings(Arc::new(cmx_agent_im::PortalBindingResolver::new(portal_base.clone())));
        let (_stop_tx, stop_rx) = tokio::sync::watch::channel(false); // CLI 无热重载，信号永不置位
        handles.push(tokio::spawn(async move { bridge.run(stop_rx).await }));
    }
    for h in handles {
        let _ = h.await;
    }
    Ok(())
}

/// qrcode-generator（MIT，vendor 于 cmx-agent-web/ui/js/vendor/，GUI 与 CLI 共用一份）。
const QR_LIB: &str = include_str!("../../cmx-agent-web/ui/js/vendor/qrcode.min.js");

/// im-login 模式：`cmx-agent im-login [data_dir]`——微信 ClawBot 扫码登录，
/// 凭证写入 `<data_dir>/im.json` 的 `wechat` 字段（桌面壳 / CLI 经 resolve 复用）。
async fn im_login(data_dir: Option<String>) -> anyhow::Result<()> {
    let data_dir = data_dir.unwrap_or_else(|| {
        std::env::temp_dir()
            .join("cmx-agent-data")
            .to_string_lossy()
            .into_owned()
    });
    let dir = std::path::Path::new(&data_dir);
    std::fs::create_dir_all(dir).map_err(|e| anyhow::anyhow!("创建数据目录失败：{e}"))?;

    // 已存的自定义基址沿用（默认 ilinkai.weixin.qq.com）。
    let prev_base = cmx_agent_im::load_im_config(dir)
        .map(|c| c.wechat.base)
        .filter(|b| !b.trim().is_empty());
    let provider = cmx_agent_im::WechatProvider::new("", prev_base);
    println!("正在获取微信登录二维码…");
    // 二维码内容是服务端下发的授权页 URL（非图片）→ 生成自包含 HTML，浏览器打开即见码。
    let html_path = dir.join("wechat_login_qr.html");
    let mut printed = false;
    let login = provider
        .login_qr_with(|content| {
            std::fs::write(&html_path, cmx_agent_im::qr_login_html(content, QR_LIB))
                .map_err(|e| format!("写二维码页面失败：{e}"))?;
            if !printed {
                println!("二维码页面已生成：{}", html_path.display());
                println!("（用浏览器打开，微信扫码并在手机确认；约 5 分钟有效，过期会自动刷新页面）");
                printed = true;
            } else {
                println!("二维码已过期，页面已自动刷新。");
            }
            Ok(())
        })
        .await
        .map_err(|e| anyhow::anyhow!(e))?;

    // 凭证落盘 im.json（bot_token 属用户本机凭证，与 model.json 存 api_key 同级）。
    let mut cfg = cmx_agent_im::load_im_config(dir).unwrap_or_default();
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
    cmx_agent_im::save_im_config(dir, &cfg).map_err(|e| anyhow::anyhow!(e))?;

    println!("✓ 微信登录成功 bot_id={}", login.bot_id);
    println!("  凭证已写入 {}", cmx_agent_im::im_config_path(dir).display());
    println!("  桌面壳重启/重载即启用微信通道；CLI 联调可另配 CMX_AGENT_IM_KIND=wechat + CMX_AGENT_IM_ALLOW=<chat_id>");
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
        .with_system("你是 TrueMate（cmx 企业桌面智能体）。安全第一，能用工具就用工具。");

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
