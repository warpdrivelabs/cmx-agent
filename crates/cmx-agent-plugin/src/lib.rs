//! cmx-agent 插件面（U15）：**`cmx-plugin.json` 清单驱动**，把外部能力包装成智能体工具。
//!
//! 「一核三面万物皆插件」的落地起点：一个插件 = 一份清单 + 一种载体。载体（都复用已有依赖，离线可测）：
//! - **`http`**：包装任意 REST 端点为工具（base_url + method + path 模板，`{arg}` 占位取自入参）。
//! - **`command`**：包装一条 shell 命令为工具（command + args 模板；写/执行类默认需审批）。
//! - **`wasm`**：经 wasm 运行时（默认 wasmer）子进程跑一个 WebAssembly 模块（invoke 导出函数 / WASI 命令）。
//! - **`mcp`**：把一个 MCP server 声明进清单，启动时连上、代理其工具（统一 U3 生态入口；异步连接见 [`connect_mcp_plugins`]）。
//!
//! 加载：扫 `<plugins_dir>/*/cmx-plugin.json`，坏清单跳过（不炸）。工具面：`plugin_list`（列已装）、
//! `plugin_install`（装一份内嵌清单**或**从市场按名装，审批门；重启后生效）、`plugin_marketplace`（浏览远程市场）。

mod command;
mod http;
mod market;
mod mcp;
mod wasm;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

pub use command::CommandPluginTool;
pub use http::HttpPluginTool;
pub use market::{PluginMarketplaceTool, fetch_market_catalog};
pub use mcp::{connect_mcp_manifest, connect_mcp_plugins};
pub use wasm::WasmPluginTool;

/// 一份插件清单（cmx-plugin.json）。`kind` 决定载体。
#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// 载体类型：`http` | `command` | `wasm` | `mcp`。
    pub kind: String,
    /// 能力/权限声明（映射到守卫 requires_auth；如 ["net:fetch"] / ["exec"]）。
    #[serde(default)]
    pub permissions: Vec<String>,
    /// 调用是否需人工审批（写/执行类置 true）。
    #[serde(default)]
    pub requires_approval: bool,
    /// 入参 JSON schema（可选；缺省用宽松 object）。
    #[serde(default)]
    pub schema: Option<Value>,

    // ── 详情展示（VSCode 式插件信息界面用）──
    /// 插件信息页（**HTML 链接**）：详情视图 iframe 内嵌 + 「在浏览器打开」外开兜底。
    #[serde(default)]
    pub homepage: Option<String>,
    /// 图标（emoji / 单字符；详情头部与列表展示；缺省按 kind 兜底）。
    #[serde(default)]
    pub icon: Option<String>,
    /// 作者 / 发布方（详情头部展示）。
    #[serde(default)]
    pub author: Option<String>,

    // ── http 载体 ──
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub path: Option<String>,

    // ── command / mcp / wasm 载体共用 ──
    /// command：要跑的程序；mcp：server 可执行；（wasm 用 module，不用它）。
    #[serde(default)]
    pub command: Option<String>,
    /// 命令行参数模板（`{arg}` 取自入参）；wasm 下作为 invoke 函数/命令的入参。
    #[serde(default)]
    pub args: Vec<String>,
    /// 环境变量（mcp server 用；map 形式 `{"KEY":"VAL"}`）。
    #[serde(default)]
    pub env: BTreeMap<String, String>,

    // ── wasm 载体 ──
    /// wasm 模块路径（相对插件目录或绝对；`.wasm`/`.wat` 皆可）。
    #[serde(default)]
    pub module: Option<String>,
    /// 要调用的导出函数名（有则 invoke 模式；无则按 WASI 命令跑）。
    #[serde(default)]
    pub invoke: Option<String>,
    /// wasm 运行时二进制（缺省 wasmer；亦可环境变量 CMX_AGENT_WASM_RUNTIME）。
    #[serde(default)]
    pub runtime: Option<String>,
}

impl PluginManifest {
    fn requires_auth(&self) -> Option<String> {
        self.permissions.first().cloned()
    }
    fn guard(&self) -> GuardHints {
        GuardHints {
            requires_auth: self.requires_auth(),
            requires_approval: if self.requires_approval {
                cmx_agent_core::tool::Approval::Always
            } else {
                cmx_agent_core::tool::Approval::Never
            },
            idempotent: !self.requires_approval,
            high_risk: self.requires_approval,
        }
    }
    fn tool_spec(&self) -> ToolSpec {
        let desc = if self.description.is_empty() {
            format!("插件 {}（{}）", self.name, self.kind)
        } else {
            self.description.clone()
        };
        let schema = self.schema.clone().unwrap_or_else(|| json!({ "type": "object" }));
        ToolSpec::new(self.name.clone(), desc).schema(schema).guard(self.guard())
    }

    /// 详情/列表用摘要（名/类型/版本/描述/权限/审批/信息页 + 载体特有字段）。前端 master-detail 视图直接消费。
    pub fn summary_json(&self) -> Value {
        json!({
            "name": self.name,
            "kind": self.kind,
            "version": self.version,
            "description": self.description,
            "permissions": self.permissions,
            "requiresApproval": self.requires_approval,
            "homepage": self.homepage,
            "icon": self.icon,
            "author": self.author,
            // 载体特有（详情「能力」区展示；无则 null）
            "baseUrl": self.base_url,
            "method": self.method,
            "path": self.path,
            "command": self.command,
            "args": self.args,
            "module": self.module,
            "invoke": self.invoke,
            "runtime": self.runtime,
        })
    }
}

/// 一组清单 → 摘要数组（供 `AgentApp::list_plugins` / `plugin_list` 复用）。
pub fn plugin_summaries(manifests: &[PluginManifest]) -> Vec<Value> {
    manifests.iter().map(PluginManifest::summary_json).collect()
}

/// 名称净化（防路径穿越）：仅留字母/数字/-/_。空 → None。
pub fn sanitize_plugin_name(name: &str) -> Option<String> {
    let safe: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_').collect();
    (!safe.is_empty()).then_some(safe)
}

/// 安装一份清单到 `<dir>/<name>/cmx-plugin.json`（校验可解析 + kind 已知 + 名称净化）。
/// 成功返回 `{installed,name,kind,path,note}`；失败返回 Err(消息)。工具与前门命令共用此逻辑。
pub fn install_manifest(dir: &Path, mf: &Value) -> Result<Value, String> {
    let m: PluginManifest = serde_json::from_value(mf.clone()).map_err(|e| format!("清单非法 {e}"))?;
    let safe = sanitize_plugin_name(&m.name).ok_or("name 非法")?;
    if !is_known_kind(&m.kind) {
        return Err(format!("未知载体 kind '{}'", m.kind));
    }
    let pdir = dir.join(&safe);
    std::fs::create_dir_all(&pdir).map_err(|e| format!("建目录失败 {e}"))?;
    let path = pdir.join("cmx-plugin.json");
    std::fs::write(&path, serde_json::to_string_pretty(mf).unwrap_or_default())
        .map_err(|e| format!("写入失败 {e}"))?;
    Ok(json!({
        "service": "cmx-plugin", "installed": true, "name": m.name, "kind": m.kind,
        "path": path.display().to_string(), "note": "重启后该插件工具生效",
    }))
}

/// 卸载：删 `<dir>/<name>/`（名称净化，限本 plugins 目录内）。目录不存在也算成功（幂等）。
pub fn uninstall_plugin(dir: &Path, name: &str) -> Result<Value, String> {
    let safe = sanitize_plugin_name(name).ok_or("name 非法")?;
    let pdir = dir.join(&safe);
    if pdir.exists() {
        std::fs::remove_dir_all(&pdir).map_err(|e| format!("删除失败 {e}"))?;
    }
    Ok(json!({
        "service": "cmx-plugin", "uninstalled": true, "name": name,
        "note": "重启后该插件工具从注册表移除",
    }))
}

/// 实时扫 plugins 目录 → 清单摘要（前门 list_plugins 用；反映安装/卸载/启停后的当前状态，无需重启）。
/// 每条摘要带 `enabled`（无 `.disabled` 标记文件即启用）。
pub fn scan_plugin_summaries(dir: &Path) -> Vec<Value> {
    read_manifests(dir)
        .iter()
        .map(|(pdir, m)| {
            let mut s = m.summary_json();
            if let Some(o) = s.as_object_mut() {
                o.insert("enabled".into(), json!(!is_plugin_disabled(pdir)));
            }
            s
        })
        .collect()
}

/// 插件是否被禁用（目录内有 `.disabled` 标记文件）。
pub fn is_plugin_disabled(plugin_dir: &Path) -> bool {
    plugin_dir.join(".disabled").exists()
}

/// 设置插件启停：`disabled=true` 写 `.disabled` 标记、`false` 删之（名称净化，限本目录）。
pub fn set_plugin_disabled(plugins_dir: &Path, name: &str, disabled: bool) -> Result<(), String> {
    let safe = sanitize_plugin_name(name).ok_or("name 非法")?;
    let pdir = plugins_dir.join(&safe);
    if !pdir.exists() {
        return Err(format!("插件 {name} 未安装"));
    }
    let marker = pdir.join(".disabled");
    if disabled {
        std::fs::write(&marker, b"disabled").map_err(|e| format!("写禁用标记失败 {e}"))?;
    } else if marker.exists() {
        std::fs::remove_file(&marker).map_err(|e| format!("删禁用标记失败 {e}"))?;
    }
    Ok(())
}

/// 读一个已装插件的清单原文（供启用时重建工具热注册）。名称净化。
pub fn read_plugin_manifest(plugins_dir: &Path, name: &str) -> Option<Value> {
    let safe = sanitize_plugin_name(name)?;
    let txt = std::fs::read_to_string(plugins_dir.join(&safe).join("cmx-plugin.json")).ok()?;
    serde_json::from_str(&txt).ok()
}

/// 记录 mcp 插件热连出的代理工具名（`<dir>/<name>/.mcp-tools.json`），供卸载/禁用时逐个热卸载。
pub fn record_mcp_tools(plugins_dir: &Path, name: &str, tool_names: &[String]) {
    if let Some(safe) = sanitize_plugin_name(name) {
        let path = plugins_dir.join(&safe).join(".mcp-tools.json");
        let _ = std::fs::write(&path, serde_json::to_string(tool_names).unwrap_or_default());
    }
}

/// 读回 mcp 插件的代理工具名（无则空）。
pub fn read_mcp_tools(plugins_dir: &Path, name: &str) -> Vec<String> {
    let Some(safe) = sanitize_plugin_name(name) else { return Vec::new() };
    std::fs::read_to_string(plugins_dir.join(&safe).join(".mcp-tools.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 从一份（刚安装的）清单构造其**同步载体工具**（http/command/wasm），供热注册。
/// mcp（需异步连接）/未知 kind → None（这类仍需重启由壳异步装配）。`plugins_dir` 为 plugins 根，
/// 内部按净化名拼插件目录以解析 wasm 相对 module。
pub fn tool_for_installed(plugins_dir: &Path, manifest: &Value) -> Option<Arc<dyn Tool>> {
    let m: PluginManifest = serde_json::from_value(manifest.clone()).ok()?;
    let safe = sanitize_plugin_name(&m.name)?;
    let pdir = plugins_dir.join(safe);
    tool_from_manifest(&m, Some(&pdir))
}

/// `{arg}` 模板替换：把 `tmpl` 里的 `{k}` 换成 `input[k]`（字符串原样，其它 to_string）。
pub(crate) fn subst(tmpl: &str, input: &Value) -> String {
    let mut out = tmpl.to_string();
    if let Some(obj) = input.as_object() {
        for (k, v) in obj {
            let rep = match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            out = out.replace(&format!("{{{k}}}"), &rep);
        }
    }
    out
}

/// 载体是否已知（含需异步连接的 mcp）。用于 plugin_install 校验与清单收集。
pub fn is_known_kind(kind: &str) -> bool {
    matches!(kind, "http" | "command" | "wasm" | "mcp")
}

/// 从一份清单构造**同步**载体工具（http/command/wasm）。
/// mcp 需异步连接（见 [`connect_mcp_plugins`]）→ 此处返回 None；未知 kind 亦 None。
/// `plugin_dir` 用于解析 wasm 模块相对路径（缺省用当前目录）。
pub fn tool_from_manifest(m: &PluginManifest, plugin_dir: Option<&Path>) -> Option<Arc<dyn Tool>> {
    match m.kind.as_str() {
        "http" => Some(Arc::new(HttpPluginTool::new(m.clone()))),
        "command" => Some(Arc::new(CommandPluginTool::new(m.clone()))),
        "wasm" => Some(Arc::new(WasmPluginTool::new(
            m.clone(),
            plugin_dir.map(Path::to_path_buf).unwrap_or_default(),
        ))),
        _ => None, // mcp：异步连接；其它：未知
    }
}

/// 扫 `<dir>/*/cmx-plugin.json`，返回 (插件目录, 已解析清单)。坏清单/未知 kind 跳过（graceful，不炸）。
pub(crate) fn read_manifests(dir: &Path) -> Vec<(PathBuf, PluginManifest)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let pdir = e.path();
        let mf = pdir.join("cmx-plugin.json");
        if !mf.exists() {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&mf) else { continue };
        match serde_json::from_str::<PluginManifest>(&txt) {
            Ok(m) if is_known_kind(&m.kind) => out.push((pdir, m)),
            Ok(m) => tracing::warn!("插件 {} 载体未知：{}", m.name, m.kind),
            Err(err) => tracing::warn!("插件清单解析失败 {}: {err}", mf.display()),
        }
    }
    out
}

/// 扫 plugins 目录，构造**同步**载体工具（http/command/wasm）+ 全部已知清单（含 mcp，供 plugin_list 展示）。
/// mcp 类无同步工具（由壳异步 [`connect_mcp_plugins`] 连接），但仍登记进清单列表。
pub fn load_plugins(dir: &Path) -> (Vec<Arc<dyn Tool>>, Vec<PluginManifest>) {
    let mut tools = Vec::new();
    let mut manifests = Vec::new();
    for (pdir, m) in read_manifests(dir) {
        // 禁用的插件：登记进清单（列表可见）但不构建工具（启动不加载）。
        if !is_plugin_disabled(&pdir) {
            if let Some(t) = tool_from_manifest(&m, Some(&pdir)) {
                tools.push(t);
            }
        }
        manifests.push(m);
    }
    (tools, manifests)
}

/// `plugin_list`：列出已装插件（名/类型/描述/权限）。持加载时的清单快照。
pub struct PluginListTool {
    plugins: Vec<Value>,
    dir: String,
}
impl PluginListTool {
    pub fn new(manifests: &[PluginManifest], dir: &Path) -> Self {
        Self {
            plugins: plugin_summaries(manifests),
            dir: dir.display().to_string(),
        }
    }
}
#[async_trait]
impl Tool for PluginListTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new("plugin_list", "列出当前已安装的插件（名称/类型/描述/权限）。")
            .guard(GuardHints { idempotent: true, ..Default::default() })
    }
    async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({
            "service": "cmx-plugin",
            "dir": self.dir,
            "count": self.plugins.len(),
            "plugins": self.plugins,
        })))
    }
}

/// `plugin_install`：把一份清单装进 plugins 目录（`<dir>/<name>/cmx-plugin.json`），审批门；重启后生效。
/// 两种来源：内嵌 `manifest` 对象，或从远程市场按 `name` 拉（配 `url`/`CMX_AGENT_PLUGIN_MARKET`）。
pub struct PluginInstallTool {
    dir: std::path::PathBuf,
    client: reqwest::Client,
}
impl PluginInstallTool {
    pub fn new(dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(20))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// 从市场目录按名取一份内嵌清单。
    async fn manifest_from_market(&self, input: &Value, name: &str) -> Result<Value, String> {
        let url = market::resolve_market_url(input)
            .ok_or("未提供市场 URL（入参 url 或设 CMX_AGENT_PLUGIN_MARKET）")?;
        let cat = market::fetch_catalog(&self.client, &url).await?;
        let entry = cat
            .plugins
            .into_iter()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("市场中无插件 '{name}'"))?;
        entry.manifest.ok_or_else(|| format!("市场条目 '{name}' 未内嵌 manifest，无法安装"))
    }
}
#[async_trait]
impl Tool for PluginInstallTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "plugin_install",
            "安装一个插件（写入插件目录，重启后可用）。来源二选一：① 内嵌 manifest 对象；\
             ② 从远程市场按 name 安装（配 url 或环境变量 CMX_AGENT_PLUGIN_MARKET，先用 plugin_marketplace 浏览）。\
             **改变智能体能力，需审批。**",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "manifest": { "type": "object", "description": "cmx-plugin.json 清单（须含 name/kind）；与 name 二选一" },
                "name": { "type": "string", "description": "从市场安装的插件名（与 manifest 二选一）" },
                "url": { "type": "string", "description": "市场目录 URL（从市场安装时；缺省用 CMX_AGENT_PLUGIN_MARKET）" }
            }
        }))
        .guard(GuardHints {
            requires_auth: Some("plugin:install".into()),
            requires_approval: cmx_agent_core::tool::Approval::Always,
            idempotent: false,
            high_risk: true,
        })
    }
    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        // 来源：内嵌 manifest 优先；否则按 name 从市场拉。
        let mf: Value = if let Some(mf) = input.get("manifest") {
            mf.clone()
        } else if let Some(name) = input.get("name").and_then(|v| v.as_str()) {
            match self.manifest_from_market(&input, name).await {
                Ok(mf) => mf,
                Err(e) => return Ok(ToolResult::err(format!("plugin_install: {e}"))),
            }
        } else {
            return Ok(ToolResult::err("plugin_install: 需 'manifest'（内嵌）或 'name'（从市场）"));
        };
        match install_manifest(&self.dir, &mf) {
            Ok(v) => Ok(ToolResult::ok(v)),
            Err(e) => Ok(ToolResult::err(format!("plugin_install: {e}"))),
        }
    }
}

/// 一站式装配：加载 plugins 目录，返回 (同步插件工具 + plugin_list + plugin_install + plugin_marketplace)。
/// mcp 类载体由壳异步 [`connect_mcp_plugins`] 连接后并入 `.mcp_tools(...)`，不在此列。
pub fn build_plugin_tools(plugins_dir: &Path) -> Vec<Arc<dyn Tool>> {
    let (mut tools, manifests) = load_plugins(plugins_dir);
    tools.push(Arc::new(PluginListTool::new(&manifests, plugins_dir)));
    tools.push(Arc::new(PluginInstallTool::new(plugins_dir.to_path_buf())));
    tools.push(Arc::new(PluginMarketplaceTool::default()));
    tools
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parse_and_spec() {
        let m: PluginManifest = serde_json::from_str(
            r#"{"name":"weather","kind":"http","description":"查天气","permissions":["net:fetch"],
                "base_url":"https://api.x","method":"GET","path":"/w/{city}"}"#,
        )
        .unwrap();
        assert_eq!(m.name, "weather");
        let spec = m.tool_spec();
        assert_eq!(spec.name, "weather");
        assert_eq!(spec.guard.requires_auth.as_deref(), Some("net:fetch"));
        assert!(tool_from_manifest(&m, None).is_some());
    }

    #[test]
    fn summary_carries_homepage_and_carrier_fields() {
        let m: PluginManifest = serde_json::from_str(
            r#"{"name":"wasm_add","kind":"wasm","version":"1.0","description":"两数相加",
                "homepage":"https://wasmer.io/","icon":"🧱","author":"me",
                "module":"add.wat","invoke":"add","runtime":"wasmer"}"#,
        )
        .unwrap();
        let s = m.summary_json();
        assert_eq!(s["name"], "wasm_add");
        assert_eq!(s["kind"], "wasm");
        assert_eq!(s["homepage"], "https://wasmer.io/");
        assert_eq!(s["icon"], "🧱");
        assert_eq!(s["module"], "add.wat");
        assert_eq!(s["invoke"], "add");
        // plugin_summaries 批量
        let arr = plugin_summaries(std::slice::from_ref(&m));
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["homepage"], "https://wasmer.io/");
    }

    #[test]
    fn subst_replaces_placeholders() {
        let v = json!({ "city": "北京", "n": 3 });
        assert_eq!(subst("/w/{city}?n={n}", &v), "/w/北京?n=3");
    }

    #[test]
    fn kind_recognition() {
        // wasm/mcp 现为已知载体；wasm 有同步工具，mcp 无（异步连接）；未知 kind 两者皆否。
        for k in ["http", "command", "wasm", "mcp"] {
            assert!(is_known_kind(k), "{k} 应为已知载体");
        }
        assert!(!is_known_kind("quux"));

        let wasm: PluginManifest =
            serde_json::from_str(r#"{"name":"w","kind":"wasm","module":"m.wasm","invoke":"f"}"#).unwrap();
        assert!(tool_from_manifest(&wasm, None).is_some());

        let mcp: PluginManifest =
            serde_json::from_str(r#"{"name":"m","kind":"mcp","command":"srv"}"#).unwrap();
        assert!(tool_from_manifest(&mcp, None).is_none(), "mcp 走异步连接，无同步工具");

        let unknown: PluginManifest = serde_json::from_str(r#"{"name":"x","kind":"quux"}"#).unwrap();
        assert!(tool_from_manifest(&unknown, None).is_none());
    }
}
