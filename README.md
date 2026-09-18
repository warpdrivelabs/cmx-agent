# cmx-agent

**企业桌面智能体微服务** 

> 方案：[`../docs/20260902_WorkBuddy复刻方案_基于Codex与dsh的企业桌面智能体.md`](../docs/20260902_WorkBuddy复刻方案_基于Codex与dsh的企业桌面智能体.md)
> 「形 / 核 / 体」三层混合：**形**借 DeepSeek Harness（前门+编排+会话日志），**核**借 Codex-rs
> （回合循环+审批守卫+工具路由），**体**用 cmx-*（把 ERP 引擎变成智能体的双手）。

## 当前状态：M2 推进中（13 crate，211 测试全绿，clippy 零告警）✅

Rust agent 内核 + 多前门（CLI / Web / Tauri 桌面壳）+ Windows 原生支持。**同核多壳**：三个壳都调
`cmx-agent-app::dispatch_json`，壳里零业务逻辑。

```
crates/
  cmx-agent-core        内核：回合循环 · 工具路由 · 会话事件日志 · 五层守卫管道 · 模型缝
  cmx-agent-tools       内置工具：fs_* / shell / grep / glob / git / run_tests / chart …（Windows 探测链 + Job Object）
  cmx-agent-connectors  企业连接器：对接 cmx-flow/rules/onto/report 等微服务 + 门户认证
  cmx-agent-model       真实模型缝（OpenAI 兼容：DeepSeek/MLamp/Qwen/本地），流式 + 断流自愈重试
  cmx-agent-mcp         外部 MCP server 接入（<data_dir>/mcp.json）
  cmx-agent-lsp         LSP 代码智能（懒连语言服务器）
  cmx-agent-office      办公面：读 Excel/PDF/Word/PPT + 写 Excel + 生成 PPT（pptx_write）
  cmx-agent-net         联网研究：web_fetch / web_search / browser_read / browser_do
  cmx-agent-im          IM 远程驱动（Telegram 等）
  cmx-agent-plugin      插件面（kind:"mcp" 清单等，<data_dir>/plugins）
  cmx-agent-app         桌面壳后端 façade：AgentApp · FileSessionStore(JSONL) · protocol(JSON 前门) · DesktopAppBuilder
  cmx-agent-cli         无头前门：demo 冒烟 + serve（stdin/stdout JSONL）
  cmx-agent-web         Web 桌面壳（离线可跑）：axum + 内嵌 UI，浏览器/Chrome --app 当桌面窗口
  cmx-agent-shell       原生 Tauri 壳（独立 workspace，需联网首拉 tauri 依赖）
```

### 已落地的方案要点

| 能力 | 落地 |
| --- | --- |
| 图 4 回合循环 | `agent.rs`：Turn=多 Step；模型→路由→pre 守卫→审批→执行→post 守卫→回灌；审批策略运行时可切（`set_policy`） |
| 图 5 工具平面 | `tool.rs`：MCP 式契约 `ToolSpec`（name/desc/inputSchema + `x-guard` 守卫标注） |
| 守卫管道 | `guard.rs`：权限、计划模式、操作审批与审计，拒绝即短路；工具按当前系统账号权限执行 |
| 图 9 会话日志 | `event.rs`：append-only `SessionLog`，**不变量 Model-visible means logged**；JSONL 落库可回放 |
| Windows 支持 | `proc.rs` shell 探测链（`CMX_AGENT_SHELL > pwsh > powershell > sh(git 反推) > cmd`）+ argv 模板 + UTF-8 前缀 + CREATE_NO_WINDOW + Job Object 进程树收尸；调研见 `../../documents/plans/20260908_cmx-agent_主流Agent的Windows支持调研与适配方案.md` |
| 断流自愈 | `openai.rs`：长流被网关中途掐断时，已收 finish_reason 则采纳、否则整轮重试（最多 3 次） |
| 办公交付 | `doc_read` 读五类文档 · `xlsx_write` 生成 Excel · `pptx_write` 生成 PPT（纯 Rust，无外部依赖） |

## 快速开始

```bash
cargo build
cargo test                               # 211 passed
cargo clippy --all-targets               # 0 issues
cargo run   -p cmx-agent-cli             # 无头 demo：打印一个回合的会话事件 JSONL

# Web 桌面壳（离线可跑；登录门需门户 :8080）
cargo run -p cmx-agent-web

# 原生 Tauri 壳（独立 workspace，首次联网拉依赖）
cd crates/cmx-agent-shell/src-tauri && cargo run
```

- **模型配置**：环境变量（`CMX_AGENT_MODEL_*` / `CMX_AI_*` / `DEEPSEEK_API_KEY`）或
  `<数据目录>/model.json`（OpenAI 兼容 base_url/api_key/model）；未配置回退离线 DemoModel。
  双壳统一数据目录 `%APPDATA%\pansoft\truemate\data`（macOS 见 shell README）。
- **Windows**：无需 Git Bash（默认 PowerShell；要 POSIX 语义设 `CMX_AGENT_SHELL=sh`）。
- **操作审批**：界面下拉切换变更前确认、计划模式或自动执行，选择记忆在浏览器 localStorage；`set_policy` 仅接收 `approval`（`on-request` / `unless-trusted` / `never` / `auto`）。自动执行只放行普通条件审批，强制审批或高风险操作直接拒绝。
- **本机执行**：不提供执行沙箱或工作目录路径围栏，文件与命令按当前系统账号权限执行；工作目录仅用于相对路径解析。不会再读写沙箱 `settings.json` / `sandbox_sid.json`，既有数据与系统 ACL 不自动清理。

## 下一步（M2→M6）

见 [`AGENTS.md`](AGENTS.md) 与方案图 10。M2：工具平面接 cmx-* 真实引擎；M3：权限与审批接 cmx-data-auth；M4 多智能体；M5 IM 远程 + 记忆。
