# cmx-agent 飞书 IM Provider（Stream 模式）落地纪要

> 日期：2026-09-07 · 关键路径：`crates/cmx-agent-im/`（`feishu.rs` / `config.rs` / `lib.rs`）·
> `crates/cmx-agent-cli/src/main.rs::im_mode`
>
> 起点：CSDN《手机一句话，遥控桌面智能体：cmx-agent IM 遥控详细实现方案》——其描述的就是本工程已有的
> `cmx-agent-im` 桥（`ImProvider` trait + `ImBridge` + Telegram 参考实现）。本纪要记录在此之上**新增飞书
> provider**、走通真机端到端、并把配置收口的全过程。

## 一、背景与目标

`cmx-agent-im` 已有 transport 无关的桥：

- `ImProvider` trait（`poll` 拉消息 + `send` 发消息）——换 IM 只写 provider，桥/内核零改动。
- `ImBridge`：拉取 → 白名单鉴权 → `im-<chat>` 会话映射 → `AgentApp::send` 跑一个完整回合 → `chunk_text`
  分段回复。同核多壳：IM 只是一个入站 transport，回合循环/五层守卫/会话库全部复用。
- `TelegramProvider`：长轮询 `getUpdates` 参考实现。

目标：**为企业 IM 加第一个 provider——飞书**，让手机/群里发消息就能遥控桌面上会跑代码、会办公的 agent。

## 二、关键选型：飞书 Stream（长连接）而非 Webhook

企业 IM 接入有两条范式：

| 范式 | 方向 | 公网回调 URL | crypto 依赖 | 部署 |
|---|---|---|---|---|
| Webhook 回调 | IM 服务器 → agent | **需要**（公网可达/内网穿透） | AES 解密 + SHA 验签 | 重 |
| Stream 长连接 | agent → IM（出站） | **不需要** | 无 | 轻 |

**选 Stream**：agent 主动连飞书 websocket 拉消息——无需公网回调 URL、无需内网穿透、**零 AES/SHA crypto
依赖**，`unsafe_code = "forbid"` 保持，范式与现有 Telegram（出站连接拉取）同构。企业微信/钉钉后续若支持
Stream 模式可照抄；若只有 webhook，再按各自验签解密追加。

## 三、飞书 Stream 协议（对照 lark oapi-sdk-go `ws/` 包还原）

连接是常驻 websocket；消息帧是 **protobuf `Frame`（`pbbp2.proto`）**，不是 JSON。

1. **拿连接地址**：`POST {base}/callback/ws/endpoint`，body `{"AppID","AppSecret"}` → 响应
   `{code:0, data:{URL, ClientConfig:{PingInterval,ReconnectCount,...}}}`，`URL` 即 wss 地址。
2. **建连**：wss 连 `URL`（tokio-tungstenite + rustls）。
3. **帧（pbbp2）**：
   - `Frame`: `SeqID` | `LogID` | `service` | `method` | `headers[]` | `payload_encoding` |
     `payload_type` | `payload` | `LogIDNew`
   - `Header`: `key` | `value`
   - `method=0` 控制帧（ping/pong）；`method=1` 数据帧（event）。headers 含 `type=event|ping|pong`、
     `message_id`、`seq` 等。
4. **心跳**：客户端发 ping 控制帧，服务端回 pong（飞书 `ClientConfig.PingInterval`，本实现用固定 30s 保活）。
5. **收消息**：`type=event` 的数据帧，payload 为 v2 事件 JSON（`im.message.receive_v1`）。取
   `event.message.{chat_id, content}`；`content` 是 JSON 串 `{"text":"…"}`，解出 `text`。
   回 ack 数据帧（payload `{"code":200}`，透传原 headers）。
6. **发消息**：`POST {base}/open-apis/im/v1/messages?receive_id_type=chat_id`，Bearer
   `tenant_access_token`（由 `auth/v3/tenant_access_token/internal` 用 app_id+app_secret 换取，缓存到过期前 60s）。
   body `{"receive_id","msg_type":"text","content":"{\"text\":\"…\"}"}`。

> pbbp2 Frame 的线格式是从 lark Go SDK 的 `pbbp2.pb.go` Marshal/Unmarshal 实现逐字段还原的
>（tag = field<<3|wire；varint=0, length-delimited=2）。**手写编解码，免引 prost/prost-build**——
> `unsafe_code = "forbid"` 下不引任何 native 代码生成。

## 四、实现

### 4.1 依赖（`crates/cmx-agent-im/Cargo.toml`）

```toml
reqwest = { version = "0.12", default-features = false, features = ["json", "rustls-tls"] }
tokio-tungstenite = { version = "0.24", default-features = false, features = ["connect", "rustls-tls-webpki-roots"] }
futures-util = { version = "0.3", default-features = false, features = ["std", "sink"] }
```

> `tokio-tungstenite` 不在离线缓存，经 aliyun 镜像联网拉取（一次）。`connect` feature 必须显式开
>（默认 `default-features=false` 时 `connect_async` 被关掉）。

### 4.2 `feishu.rs`：FeishuProvider

```rust
pub struct FeishuProvider { inner: Arc<FeishuInner> }

struct FeishuInner {
    app_id, app_secret, base: String,
    client: reqwest::Client,
    inbox: Mutex<VecDeque<InboundMsg>>,   // Stream task → poll 的入站队列
    seq: AtomicI64,                       // 自增 update_id（飞书事件无稳定 seq）
    token: Mutex<Option<(String, Instant)>>,  // tenant_access_token 缓存
    started: AtomicBool,                  // start() 幂等守门
}
```

- `from_env()`：读 `CMX_AGENT_IM_FEISHU_APP_ID` + `_APP_SECRET`（必需）+ `_BASE`（可选，默认
  `https://open.feishu.cn`；海外 `https://open.larksuite.com`）。
- `tenant_token()`：缓存 + 过期前 60s 刷新。
- `fetch_endpoint()`：调 `get_endpoint` 拿 wss URL。
- `run_stream_once()`：建连 → `select!` 收帧（Binary→解码→ping/pong/event 入队+ack；Ping→Pong）+
  定时 ping；出错返回由外层重连。
- `run_stream_loop()`：失败/断开 → 3s 退避重连，永不放弃。
- `impl ImProvider`：
  - `poll(offset)`：只取 `update_id > offset` 的；`<= offset` 的丢弃（防崩溃重启后历史残留重复驱动）。
  - `send(chat_id, text)`：调发消息 API；非 2xx 读 body 入错误信息。
  - `start()`：CAS 守门，仅首次 spawn Stream 后台 task；`Arc::clone(&inner)` 入 task（不重建内层，状态共享）。

协议解析/帧构造提成**纯函数**单测：`parse_receive_event` / `build_send_body` / `encode_frame` /
`decode_frame` / `make_ping_frame` / `make_pong_frame` / `make_ack_frame`。

### 4.3 trait 唯一（向后兼容）扩展

`ImProvider` 加默认 no-op `start()`：

```rust
async fn start(&self) -> Result<(), String> { Ok(()) }
```

飞书 override 启动 Stream task；Telegram 沿用默认。桥/CLI 统一 `provider.start()` + `bridge.run()`，不为
飞书加特例入口。`ImBridge` 编排、`chunk_text`、内核**零改动**。

### 4.4 配置收口（`config.rs`）

把 provider 选择、白名单、凭证从 env 收口到一处（范式对齐 `cmx_agent_model::ModelProviderConfig`）：

```rust
pub enum ImKind { Telegram, Feishu }

pub struct ImConfig { kind: ImKind, allow: Option<HashSet<String>> }

impl ImConfig {
    pub fn from_env() -> Result<Self, String>;       // kind + allow
    pub fn build_provider(&self) -> Result<Arc<dyn ImProvider>, String>;  // 凭证
}
```

环境变量（IM 前缀 `CMX_AGENT_IM_*`）：

| 变量 | 必需 | 说明 |
|---|---|---|
| `CMX_AGENT_IM_KIND` | — | provider 类型，默认 `telegram`；可选 `feishu` |
| `CMX_AGENT_IM_ALLOW` | ✓ | 逗号分隔的 chat_id 白名单（安全必需） |
| `CMX_AGENT_IM_NO_ALLOW` | — | `=1` 显式放开白名单（仅测试/纯内网，生产勿用） |
| `CMX_AGENT_IM_TOKEN` | TG | Telegram bot token（+ 可选 `_BASE`） |
| `CMX_AGENT_IM_FEISHU_APP_ID` | 飞书 | 飞书 app_id |
| `CMX_AGENT_IM_FEISHU_APP_SECRET` | 飞书 | 飞书 app_secret（+ 可选 `_BASE`） |

白名单语义：`ALLOW` 非空 → `Some(set)`；`NO_ALLOW=1` → `None`（开放）；两者都未设 → `Err`（**拒裸奔**）。
解析逻辑提成纯函数 `parse_kind` / `parse_allow_str`，避开 edition 2024 下 `env::set_var` 的 unsafe 限制
（`unsafe_code=forbid` 下测试不能写 env）。

### 4.5 CLI（`main.rs::im_mode`）

配置全交 `ImConfig`，CLI 只剩装配：

```rust
let im_cfg = cmx_agent_im::ImConfig::from_env()?;
let provider = im_cfg.build_provider()?;
let allow = im_cfg.allow;
tracing::info!("cmx-agent IM provider = {:?}", im_cfg.kind);
provider.start().await?;                      // 飞书起 stream task；telegram no-op
let bridge = cmx_agent_im::ImBridge::new(app, provider, allow);
bridge.run().await;
```

### 4.6 `ImBridge::run` 节流

原 `run()` 无消息时无 sleep 空转（长轮询 provider 自带阻塞；Stream provider 靠此节流）。改为：

```rust
match self.tick().await {
    Ok(0) => sleep(500ms),   // 无消息节流
    Ok(_) => { /* 立即下一轮 */ }
    Err(e) => { warn; sleep(3s); }
}
```

未授权分支加日志 `IM 未授权 chat_id=…`（联调抓 chat_id 用）。

## 五、真机联调全过程

### 5.1 前置探活（curl）

- 飞书 endpoint：`POST https://open.feishu.cn/callback/ws/endpoint`，空凭证返 `code 1000040346`（凭证无效类）→ 链路通。
- 真实凭证换 endpoint：返 `code:0` + wss URL + `ClientConfig` → 凭证有效。
- 模型端点：`POST https://llmgw-bz.mlamp.cn/v1/chat/completions` 用 GLM-5.2 返正常 completion → 模型可用。

### 5.2 启动 + 抓 chat_id

第一版用占位白名单 `CMX_AGENT_IM_ALLOW=__probe__` 启动（绕过「无白名单 bail」）。连上 Stream 后
**最初完全收不到帧**——排查发现是飞书应用侧配置（事件订阅模式、应用发布状态、权限）。修好权限并重新发布后：

- 飞书发消息 → agent 回「⛔ 未授权：你的会话不在允许列表内。」
  → 证明 Stream 收帧、帧解码、事件解析、发消息 API 全通。
- 加日志后从日志抓到真实 chat_id：`oc_3fb7cb8eb50b86a14c94205bd31b8f8e`。

### 5.3 端到端跑通

用真实 chat_id 作白名单重启，飞书发「帮我算一下2+3等于多少」：

```
user:      帮我算一下2+3等于多少
model:     [调 add(2,3)]
tool:      add → {sum: 5.0}
model:     2 + 3 = **5** 🎉
turn ended (2 steps, reason: completed)
```

会话落库 `im-oc_3fb7cb8eb50b86a14c94205bd31b8f8e`（7 事件，重启可续上下文）。飞书收到回复
`2 + 3 = **5** 🎉`。

### 5.4 验证项

- ✅ 飞书 Stream（websocket + 手写 pbbp2 protobuf 编解码）
- ✅ 事件解析 + 白名单鉴权
- ✅ 同核多壳：`AgentApp::send` 跑桌面壳/CLI 同一个回合循环
- ✅ 五层守卫 + 工具执行（`add` 被放行）
- ✅ 模型真实回答（GLM-5.2 经 llmgw 网关）
- ✅ 回复分段发回飞书（`/im/v1/messages`）
- ✅ 会话落库（`im-<chat>` 会话，重启可续）

## 六、测试

`cargo test --offline -p cmx-agent-im` —— 17 测全绿：

- `feishu.rs` 单测（11）：消息解析（文本/非文本/空/其他事件）、send body 形状、Frame 编解码 roundtrip、
  ping/pong/ack 帧、`poll` drain 游标推进、`poll` 跳过 `<=offset` 的历史残留。
- `bridge_tests.rs`（3）：MockProvider 驱动桥的鉴权/会话映射/分段。
- `feishu_tests.rs`（3）：FeishuProvider + 手动 inbox 驱动桥（鉴权/会话复用）。
- `config.rs`（3 纯函数）：`parse_kind` / `parse_allow_str`。
- `lib.rs`（1）：`chunk_text` 无损分段。

整仓 `cargo test --offline` 全绿；新增代码 `cargo clippy --offline --all-targets` 零告警。

## 七、跑法

```bash
export CMX_AGENT_IM_KIND=feishu
export CMX_AGENT_IM_FEISHU_APP_ID=cli_xxx
export CMX_AGENT_IM_FEISHU_APP_SECRET=xxx
export CMX_AGENT_IM_ALLOW=oc_xxx          # 飞书 chat_id；联调抓法见下
export CMX_AGENT_MODEL_BASE_URL=https://llmgw-bz.mlamp.cn/v1
export CMX_AGENT_MODEL_API_KEY=sk-xxx
export CMX_AGENT_MODEL=mlamp/glm-5.2

RUST_LOG=info cargo run --offline -p cmx-agent-cli -- im /tmp/cmx-feishu
```

**联调抓 chat_id**：设 `CMX_AGENT_IM_ALLOW=__probe__`（占位绕过 bail），在飞书发条消息，日志打
`IM 未授权 chat_id=oc_…`，把该 id 填进 `CMX_AGENT_IM_ALLOW` 重启即可。

**飞书应用侧前置**（任一不满足收不到事件）：
1. 事件订阅 = **长连接（Stream）** 模式（非 webhook）。
2. 已订阅 `im.message.receive_v1`，权限 `im:message` 已授予。
3. 应用已发布且 **可用**（改权限/事件后必须重新发布版本）。
4. 已添加「机器人」能力，能搜到并打开会话。

## 八、扩展路线（企业微信 / 钉钉）

照抄此范式，桥/内核永不改动：

1. 新建 `wecom.rs` / `dingtalk.rs`，`impl ImProvider`。
2. `config.rs::ImKind` 加变体 + `build_provider` 加分支。
3. 若该 IM 支持 Stream/websocket：零 crypto，直接套 `FeishuProvider` 的后台 task + inbox 模式。
4. 若只有 webhook：provider 内部起轻量 HTTP 收器（axum）+ 验签解密 + 内部队列，`poll` 从队列取——
   桥依旧只调 `poll`/`send`，不感知范式差异。需加 `aes`/`sha2`/`hmac` 依赖（先确认 aliyun 缓存命中）。

## 九、文件清单

| 文件 | 改动 |
|---|---|
| `crates/cmx-agent-im/src/feishu.rs` | **新增**：FeishuProvider + 手写 pbbp2 Frame 编解码 + 纯函数 + 单测 |
| `crates/cmx-agent-im/src/config.rs` | **新增**：`ImConfig` / `ImKind` / `parse_allow` 配置收口 + 纯函数测试 |
| `crates/cmx-agent-im/src/lib.rs` | `mod feishu/config` + 导出；`ImProvider` 加默认 `start()`；`ImBridge::run` 节流 + 未授权日志 |
| `crates/cmx-agent-im/Cargo.toml` | 加 `tokio-tungstenite` / `futures-util` |
| `crates/cmx-agent-im/tests/feishu_tests.rs` | **新增**：FeishuProvider × ImBridge 集成测试 |
| `crates/cmx-agent-cli/src/main.rs` | `im_mode` 改用 `ImConfig` 装配 |

**不动**：`cmx-agent-core`（回合/守卫/会话库）、`cmx-agent-app`（`AgentApp::send`/`dispatch_json`）、
`ImBridge` 编排、`chunk_text`、`TelegramProvider`——这正是 `ImProvider` 抽象的目的。
