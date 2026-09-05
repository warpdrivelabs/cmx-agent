//! `lsp` 工具：按文件扩展名连对应语言服务器，提供 hover/definition/references/document_symbol/diagnostics。
//!
//! 配置来自 `<data_dir>/lsp.json`：`{ ".rs": {"command":"rust-analyzer","language_id":"rust"}, ... }`。
//! **有则用无则降级**：无配置或无对应 server → 返回清晰提示而非报错。语言服务器较重，故按 ext **缓存复用**连接。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};
use tokio::sync::Mutex;

use crate::client::LspClient;

/// 单个语言服务器配置。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LspServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// LSP languageId（如 rust / python / typescript）。缺省用扩展名去点。
    #[serde(default)]
    pub language_id: Option<String>,
}

pub struct LspTool {
    /// 扩展名（含点，如 ".rs"）→ 语言服务器配置。
    servers: HashMap<String, LspServerConfig>,
    /// 按扩展名缓存的连接（懒启动、复用）。
    clients: Mutex<HashMap<String, Arc<Mutex<LspClient>>>>,
}

impl LspTool {
    pub fn new(servers: HashMap<String, LspServerConfig>) -> Self {
        Self { servers, clients: Mutex::new(HashMap::new()) }
    }

    /// 从 `lsp.json` 读取配置；文件不存在/解析失败 → 空配置（工具降级）。
    pub fn from_config_file(path: &Path) -> Self {
        let servers = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, LspServerConfig>>(&s).ok())
            .unwrap_or_default();
        Self::new(servers)
    }

    pub fn is_configured(&self) -> bool {
        !self.servers.is_empty()
    }

    /// 取或启动某扩展名的语言服务器连接。
    async fn client_for(&self, ext: &str, root_uri: &str) -> Result<Arc<Mutex<LspClient>>, String> {
        if let Some(c) = self.clients.lock().await.get(ext) {
            return Ok(c.clone());
        }
        let cfg = self.servers.get(ext).ok_or_else(|| {
            format!("未为扩展名 '{ext}' 配置语言服务器（在 lsp.json 配置）")
        })?;
        let client = LspClient::connect(&cfg.command, &cfg.args, root_uri)
            .await
            .map_err(|e| format!("启动语言服务器失败：{e}"))?;
        let arc = Arc::new(Mutex::new(client));
        self.clients.lock().await.insert(ext.to_string(), arc.clone());
        Ok(arc)
    }
}

fn resolve(path: &str, ctx: &ToolCtx) -> Result<PathBuf, String> {
    if ctx.allowed_roots.is_empty() {
        return Err("no allowed_roots".into());
    }
    let raw = PathBuf::from(path);
    let abs = if raw.is_absolute() { raw } else { ctx.allowed_roots[0].join(raw) };
    let canon = std::fs::canonicalize(&abs).unwrap_or(abs);
    let ok = ctx.allowed_roots.iter().any(|r| {
        let r = std::fs::canonicalize(r).unwrap_or_else(|_| r.clone());
        canon.starts_with(&r)
    });
    if ok { Ok(canon) } else { Err(format!("path '{path}' escapes sandbox")) }
}

fn file_uri(p: &Path) -> String {
    format!("file://{}", p.display())
}

#[async_trait]
impl Tool for LspTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "lsp",
            "代码智能（LSP）：hover 悬停 / definition 定义 / references 引用 / document_symbol 符号 / diagnostics 诊断。position 用 0 基 line/character。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "operation": {"type":"string","enum":["hover","definition","references","document_symbol","diagnostics"]},
                "path": {"type":"string","description":"工作区内源文件路径"},
                "line": {"type":"integer","description":"0 基行号（hover/definition/references 需要）"},
                "character": {"type":"integer","description":"0 基列号"}
            },
            "required": ["operation","path"]
        }))
        .guard(GuardHints { requires_auth: Some("fs:read".into()), idempotent: true, ..Default::default() })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !self.is_configured() {
            return Ok(ToolResult::err(
                "lsp: 未配置语言服务器。请在 <数据目录>/lsp.json 配置，如 {\".rs\":{\"command\":\"rust-analyzer\",\"language_id\":\"rust\"}}",
            ));
        }
        let op = input.get("operation").and_then(|v| v.as_str()).unwrap_or("");
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("lsp: 'path' is required"));
        };
        let abs = match resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("lsp: {e}"))),
        };
        let ext = abs.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        let text = std::fs::read_to_string(&abs).unwrap_or_default();
        let lang = self
            .servers
            .get(&ext)
            .and_then(|c| c.language_id.clone())
            .unwrap_or_else(|| ext.trim_start_matches('.').to_string());
        let root_uri = file_uri(&ctx.allowed_roots[0]);
        let uri = file_uri(&abs);

        let client = match self.client_for(&ext, &root_uri).await {
            Ok(c) => c,
            Err(e) => return Ok(ToolResult::err(format!("lsp: {e}"))),
        };
        let mut c = client.lock().await;
        if let Err(e) = c.ensure_open(&uri, &lang, &text).await {
            return Ok(ToolResult::err(format!("lsp: didOpen 失败：{e}")));
        }
        let line = input.get("line").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
        let ch = input.get("character").and_then(|v| v.as_u64()).unwrap_or(0) as u32;

        let out = match op {
            "hover" => c.hover(&uri, line, ch).await.map(|r| json!({"hover": summarize_hover(&r)})),
            "definition" => c.definition(&uri, line, ch).await.map(|r| json!({"locations": summarize_locations(&r)})),
            "references" => c.references(&uri, line, ch).await.map(|r| json!({"locations": summarize_locations(&r)})),
            "document_symbol" => c.document_symbol(&uri).await.map(|r| json!({"symbols": summarize_symbols(&r)})),
            "diagnostics" => {
                let d = c.collect_diagnostics(&uri, Duration::from_secs(8)).await;
                Ok::<Value, crate::client::LspError>(json!({"diagnostics": summarize_diagnostics(&d)}))
            }
            _ => return Ok(ToolResult::err(format!("lsp: 未知 operation '{op}'"))),
        };
        match out {
            Ok(v) => Ok(ToolResult::ok(v)),
            Err(e) => Ok(ToolResult::err(format!("lsp {op} 失败：{e}"))),
        }
    }
}

fn summarize_hover(r: &Value) -> String {
    let Some(c) = r.get("contents") else { return String::new() };
    match c {
        Value::String(s) => s.clone(),
        Value::Object(o) => o.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
        Value::Array(a) => a
            .iter()
            .map(|x| match x {
                Value::String(s) => s.clone(),
                Value::Object(o) => o.get("value").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn loc_str(loc: &Value) -> Option<String> {
    let uri = loc.get("uri").or_else(|| loc.get("targetUri")).and_then(|u| u.as_str())?;
    let range = loc.get("range").or_else(|| loc.get("targetRange"))?;
    let l = range.get("start").and_then(|s| s.get("line")).and_then(|x| x.as_u64()).unwrap_or(0);
    let ch = range.get("start").and_then(|s| s.get("character")).and_then(|x| x.as_u64()).unwrap_or(0);
    let short = uri.strip_prefix("file://").unwrap_or(uri);
    Some(format!("{short}:{}:{}", l + 1, ch + 1))
}
fn summarize_locations(r: &Value) -> Vec<String> {
    match r {
        Value::Array(a) => a.iter().filter_map(loc_str).collect(),
        Value::Object(_) => loc_str(r).into_iter().collect(),
        _ => vec![],
    }
}
fn summarize_symbols(r: &Value) -> Vec<String> {
    let Some(a) = r.as_array() else { return vec![] };
    a.iter()
        .filter_map(|s| {
            let name = s.get("name").and_then(|n| n.as_str())?;
            let kind = s.get("kind").and_then(|k| k.as_u64()).unwrap_or(0);
            Some(format!("{name} ({})", symbol_kind(kind)))
        })
        .collect()
}
fn summarize_diagnostics(d: &[Value]) -> Vec<String> {
    d.iter()
        .filter_map(|x| {
            let msg = x.get("message").and_then(|m| m.as_str())?;
            let sev = x.get("severity").and_then(|s| s.as_u64()).unwrap_or(0);
            let l = x.get("range").and_then(|r| r.get("start")).and_then(|s| s.get("line")).and_then(|v| v.as_u64()).unwrap_or(0);
            let s = match sev { 1 => "error", 2 => "warn", 3 => "info", 4 => "hint", _ => "?" };
            Some(format!("[{s}] L{}: {msg}", l + 1))
        })
        .collect()
}
fn symbol_kind(k: u64) -> &'static str {
    match k {
        5 => "class", 6 => "method", 8 => "field", 9 => "constructor", 11 => "interface",
        12 => "function", 13 => "variable", 14 => "constant", 23 => "struct", 10 => "enum", _ => "symbol",
    }
}
