# AGENTS.md — cmx-agent

> 企业桌面智能体微服务（复刻并超越腾讯云 WorkBuddy）。方案见
> `../docs/20260902_WorkBuddy复刻方案_基于Codex与dsh的企业桌面智能体.md`。

## 这是什么

「形 / 核 / 体」三层混合智能体的 **核 + 多壳后端**。当前 **M2 推进中（13 crate，211 测试全绿、
clippy 零告警）**：M0 内核（回合循环/守卫/会话日志/模型缝）之上已落地——会话 JSONL 落库、本地文件
工具沙箱、JSON 前门协议（= Tauri invoke 边界）、真实模型缝（OpenAI 兼容 + 断流自愈）、企业连接器、
MCP/LSP/办公/联网/IM/插件面、Web 桌面壳与 Tauri 原生壳、Windows 原生支持（shell 探测链 + Job Object）。

## 构建 / 测试

```bash
cd cmx-agent
cargo build --offline            # 离线构建
cargo test  --offline            # 全量测试（当前 211 passed）
cargo clippy --offline --all-targets   # 必须零告警
./e2e-serve.sh                   # e2e：serve 前门跨进程持久化
cargo run --offline -p cmx-agent-cli                    # demo：打印一个回合的会话事件 JSONL
cargo run --offline -p cmx-agent-web                    # Web 桌面壳（登录门需门户 :8080）
echo '{"cmd":"send","session_id":"s1","text":"算 2+3"}' | \
  cargo run --offline -p cmx-agent-cli -- serve /tmp/d  # serve：JSON 前门（= Tauri invoke）
```

- **离线优先**：本机无公网 registry，一律加 `--offline`。外部 crate 版本照抄 cmx-container 根，
  复用其离线缓存；新增外部依赖前先确认缓存命中。
- 工具链锁定 `1.97.1`（`rust-toolchain.toml`），edition 2024，与 cmx-container/flow/rules 对齐。

## 架构约束（改动前必读）

1. **不变量 Model-visible means logged**：任何"模型能看见"的东西（用户输入、模型输出、工具调用、
   工具结果）必须先 `SessionLog::append`，模型上下文只经 `Session::model_context` 从日志派生。
   新增模型可见信息时，务必同时新增对应 `EventKind` 并落日志——`invariant_tests.rs` 会守住这条。
2. **fail-closed**：守卫管道遇第一个非 Allow 即短路拒绝；工具失败转 `ToolResult::err` 回灌（不 panic、
   不中止回合）。禁止"出错就放行"。
3. **两旋钮正交**：`SandboxMode`（能力）与 `ApprovalPolicy`（许可）独立；改一个别默认改另一个。
4. **工具即契约**：每个 `Tool` 必须给出 `GuardHints`（requires_auth / requires_approval /
   idempotent / high_risk）——护栏据此施闸，不靠猜。
5. **crate 分层**：`core`（无 IO 内核）← `tools`（内置工具）← `cli`（前门）。core 的集成测试用
   tools 走 dev-dep 环（Cargo 允许），不得让 core 的 **normal** 依赖反向指向 tools。

## crate 布局

| crate | 职责 |
|---|---|
| `cmx-agent-core` | 内核：agent/回合循环 · guard/守卫 · event/会话日志 · model/模型缝 · session · tool/注册表 |
| `cmx-agent-tools` | 内置工具：fs_* / shell（Windows 探测链+Job Object）· grep/glob/git/run_tests · chart 等 |
| `cmx-agent-connectors` | 企业连接器：cmx-flow/rules/onto/report 微服务对接 + 门户认证 |
| `cmx-agent-model` | 真实模型缝：OpenAI 兼容（流式 + 断流自愈重试）· 多 provider 配置 |
| `cmx-agent-mcp` / `cmx-agent-lsp` | 外部 MCP server 接入 / LSP 代码智能 |
| `cmx-agent-office` | 办公面：doc_read 读五类文档 · xlsx_write 写 Excel · pptx_write 生成 PPT |
| `cmx-agent-net` / `cmx-agent-im` | 联网研究（web_fetch/search/browser_*）· IM 远程驱动 |
| `cmx-agent-plugin` | 插件面（`<data_dir>/plugins` 清单，kind:"mcp" 等） |
| `cmx-agent-app` | 桌面壳后端 façade：AgentApp · FileSessionStore(JSONL) · protocol(JSON 前门) · DesktopAppBuilder |
| `cmx-agent-cli` | 无头前门 `cmx-agent`：demo(M0 冒烟) + `serve`(JSON 前门 e2e) |
| `cmx-agent-web` | Web 桌面壳（离线可跑）：axum + 内嵌 UI，同核多壳 |
| `cmx-agent-shell` | 原生 Tauri 壳（独立单成员 workspace + 自带 Cargo.lock；**不在离线 members**，构建需联网首拉 tauri） |

## M1 关键约束（在 M0 之上新增）

6. **同核多壳**：桌面壳 / CLI / Web 壳 / 后续 Headless HTTP 都调 `cmx-agent-app::dispatch_json`——壳里零业务逻辑。
   加新前门=加薄壳，不改核。
7. **增量落库**：每回合只 append 新事件（`SessionStore::append_events`），绝不重写历史行——与
   append-only 内核日志同构。回合号从日志派生（`Session::next_turn_no`），故重启可续。
8. **会话 id 即路径**：`FileSessionStore` 必须挡路径注入（`/`、`..`、`\0`）——`store_tests.rs` 守住。
9. **前门错误进信封**：`dispatch` 永不 panic/Err，错误转 `{ok:false,error:{code,...}}` 稳定错误码。
10. **两旋钮运行时可切**：`Agent::policy` 经 RwLock 持有，前门 `set_policy` 热切
   （`sandbox: read-only|workspace-write|danger-full-access` × `approval: never|on-request|unless-trusted`）；
   回合内按快照读取，改 `Policy` 相关代码须保持快照语义。
11. **Windows 适配（2026-09-08 方案，详见 `../../documents/plans/20260908_cmx-agent_*.md`）**：
   子进程一律走 `proc::run`/`run_cmd`，**禁止再硬编码 shell**——探测链 `CMX_AGENT_SHELL > pwsh >
   powershell > sh(git.exe 反推) > cmd` 缓存于进程内；PowerShell 命令须带 UTF-8 前缀；
   Windows 子进程默认 CREATE_NO_WINDOW + Job Object 整树收尸，别绕开。

## 路线（M0→M6，见方案图 10）

M0 核 ✅ → M1 桌面壳(Tauri)+本地文件 → M2 工具平面(接 cmx-*)+技能 → M3 五层护栏接地+OS/WASM 沙箱
→ M4 多智能体编排 → M5 IM 远程+记忆 → M6 Rust 核下沉+GA。**每个里程碑都要有可回放会话日志 +
护栏红队用例 + 真机 e2e 断言。**

## 测试口径

改内核后至少跑：`cargo test --offline` 全绿 + `cargo clippy --offline --all-targets` 零告警。
新增能力必须带测试；安全相关（守卫/沙箱/审批）必须含"该拒被拒"的负例。
