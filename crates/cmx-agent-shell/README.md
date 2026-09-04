# cmx-agent-shell — 原生 Tauri 桌面壳（已就绪 ✅）

cmx 企业桌面智能体的**原生桌面 App**（系统 WKWebView，非浏览器窗口）。方案「同核多壳」的原生兑现：
壳里零业务逻辑，全部复用已测的 `cmx-agent-app`（façade / JSONL 落库 / 前门协议 / 五层守卫）。

```
WKWebView 前端  --invoke("agent","{cmd:...}")-->  src-tauri (本 crate)  --dispatch_json-->  cmx-agent-app
   （ui/index.html）                                 （~40 行胶水）                            （核，48 测试）
```

## 运行

```bash
cd crates/cmx-agent-shell/src-tauri
cargo run                 # 弹出原生窗口（首次已联网拉过 tauri 依赖，之后可 --offline）
```

数据落到 `~/Library/Application Support/com.pansoft.cmx-agent/`（sessions/ + workspace/）——与 Web 壳
同一 `FileSessionStore`，同核。

## 与 Web 壳的关系（同核多壳）

| | 原生 Tauri 壳（本 crate） | Web 壳（`cmx-agent-web`） |
|---|---|---|
| 窗口 | 系统 WKWebView（原生菜单栏/dock） | Chrome `--app` 窗口 |
| 边界 | Tauri `invoke("agent")` | HTTP `POST /api` |
| 核 | **同一个** `cmx-agent-app::dispatch_json` | 同左 |
| 前端 | **同一份** `ui/index.html`（`call()` 检测 `window.__TAURI__` 自动切换） | 同左 |
| 离线 workspace | **不属于**（独立 Cargo.lock，联网构建一次） | 属于（离线可测） |

前端 `ui/index.html` 从 `cmx-agent-web/ui/index.html` 复制而来，仅 `call()` 一处适配双壳。

## 关键文件

- `src-tauri/src/main.rs` — Tauri 入口：`DesktopAppBuilder` 装配 AgentApp + 一个 `#[tauri::command] agent` 把 payload 交给 `dispatch_json`。
- `src-tauri/tauri.conf.json` — 窗口/标题/图标/CSP/`frontendDist: ./ui`。
- `src-tauri/capabilities/default.json` — 授 `core:default` 给 `main` 窗口（Tauri 2 必需，否则自定义 invoke 被拒）。
- `src-tauri/icons/` — cmx 品牌图标集（含 .icns）。
- `src-tauri/build.rs` — `tauri_build::build()`。

## 为何独立于离线 workspace

`tauri` 依赖庞大且需联网首拉；若把本 crate 加进 `cmx-agent/Cargo.toml` 的 members，会让**整个 workspace**
的 `cargo build --offline` 失败，砸了 M0/M1 的离线可测性。故本 crate 用 `[workspace]` 自成单成员 workspace +
自带 Cargo.lock，经 path 直引 `cmx-agent-app`。已验证：改动后 `cd cmx-agent && cargo test --offline` 仍 48 绿。
