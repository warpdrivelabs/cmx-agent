# cmx-agent

**企业桌面智能体微服务** —— 复刻并超越腾讯云 WorkBuddy。

> 方案：[`../docs/20260902_WorkBuddy复刻方案_基于Codex与dsh的企业桌面智能体.md`](../docs/20260902_WorkBuddy复刻方案_基于Codex与dsh的企业桌面智能体.md)
> 「形 / 核 / 体」三层混合：**形**借 DeepSeek Harness（前门+编排+会话日志），**核**借 Codex-rs
> （回合循环+内核沙箱+工具路由），**体**用 cmx-*（把 ERP 引擎变成智能体的双手）。

## 当前状态：M0「核」已落地 ✅

Rust agent kernel，与平台/框架解耦，**30 个测试全绿，clippy 零告警**。

```
crates/
  cmx-agent-core    内核：回合循环 · 工具路由 · 会话事件日志 · 五层守卫管道 · 模型缝
  cmx-agent-tools   内置工具：echo · add · clock · fs_read（沙箱围栏）· danger_rm（高危靶子）
  cmx-agent-cli     无头前门 `cmx-agent`（会话事件 JSONL，M0 冒烟 / e2e 入口）
```

### 已实现的方案要点

| 方案图 | 落地 |
|---|---|
| 图 4 回合循环 | `agent.rs`：Turn=多 Step；模型→路由→pre 守卫→审批→执行→post 守卫→回灌 |
| 图 5 工具平面 | `tool.rs`：MCP 式契约 `ToolSpec`（name/desc/inputSchema + `x-guard` 守卫标注） |
| 图 7 五层护栏 | `guard.rs`：AuthGuard(权限)/ApprovalGuard(人在环)/HighRiskGuard(高危)；fail-closed 短路 |
| 图 9 会话日志 | `event.rs`：append-only `SessionLog`，**不变量 Model-visible means logged** |
| 两旋钮 | `SandboxMode`（能力）× `ApprovalPolicy`（许可）正交 |

## 快速开始

```bash
cargo build --offline
cargo test  --offline                    # 30 passed
cargo clippy --offline --all-targets     # 0 issues
cargo run   --offline -p cmx-agent-cli   # 打印一个回合的会话事件日志（JSONL）
```

演示输出（模型脚本化调用 `add(2,3)`，全程可审计）：

```json
{"seq":1,"kind":"turn_started","turn":1,"user_input":"帮我把 2 和 3 相加"}
{"seq":2,"kind":"user_message","text":"帮我把 2 和 3 相加"}
{"seq":3,"kind":"model_message","text":"我来算一下。","tool_calls":[{"id":"call-1","name":"add","input":{"a":2,"b":3}}]}
{"seq":4,"kind":"tool_invoked","call":{"id":"call-1","name":"add","input":{"a":2,"b":3}}}
{"seq":5,"kind":"tool_result","call_id":"call-1","ok":true,"output":{"sum":5.0}}
{"seq":6,"kind":"model_message","text":"2 + 3 = 5。已完成。","tool_calls":[]}
{"seq":7,"kind":"turn_ended","turn":1,"reason":"completed","steps":2}
```

## 测试覆盖（30）

- **loop_tests**（7）：单/多 step、结果回灌、未知工具、max_steps 截断、并行调用、多回合累积
- **guard_tests**（10）：权限允许/拒绝、审批批准/拒绝、高危拦截/放行、两旋钮、fail-closed 短路、post 相
- **invariant_tests**（5）：model-visible-means-logged 正反验证、append-only 单调、回合边界、JSON 回放
- **tool_tests**（8）：echo/add/clock、沙箱空根拒绝、根内读取、**`../` 逃逸拦截**、高危标注、注册表

## 下一步（M1→M6）

见 [`AGENTS.md`](AGENTS.md) 与方案图 10。M1：Tauri 桌面壳 + 本地文件；M2：接 cmx-flow/rules/ontology
等真实工具 + MCP 外接；M3：五层护栏接 cmx-data-auth + OS/WASM 沙箱；M4 多智能体；M5 IM 远程 + 记忆。
