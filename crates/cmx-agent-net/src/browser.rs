//! `browser_read`（headless Chrome 渲染 JS 后取正文）+ `browser_screenshot`（截图存工作区）。
//!
//! 走**子进程调本机 Chrome/Chromium**（`--headless --dump-dom` / `--screenshot`），不引 CDP/WebSocket 依赖。
//! URL 来自模型 → 过 [`crate::ensure_public_url`] SSRF 基线；截图输出路径过沙箱围栏。
//! 相比 [`crate::WebFetchTool`]（静态 HTML），本工具能读 **JS 渲染后**的动态页面（SPA 等）。

use std::path::{Path, PathBuf};
use std::time::Duration;

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{ensure_public_url, html};

/// 找本机 Chrome/Chromium。env `CMX_AGENT_CHROME` 优先，其次常见安装路径。
pub(crate) fn find_chrome() -> Option<String> {
    if let Ok(p) = std::env::var("CMX_AGENT_CHROME")
        && !p.is_empty() && Path::new(&p).exists() {
            return Some(p);
        }
    [
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
    ]
    .into_iter()
    .find(|p| Path::new(p).exists())
    .map(|s| s.to_string())
}

/// 每次调用独立的临时 --user-data-dir，避免与用户 Chrome 冲突、支持并发。
pub(crate) fn tmp_profile() -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!(
        "cmx-agent-hchrome-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ))
}

/// 最小可靠 flag 集。**不加 `--virtual-time-budget`**（它会让 `--dump-dom` 挂起不退出）；
/// `--dump-dom` 在 load 事件后转储，同步/onload 的 JS 已生效（对多数页面够用）。
pub(crate) fn base_args(profile: &Path, extra: &[String]) -> Vec<String> {
    let mut a = vec![
        "--headless".into(),
        "--disable-gpu".into(),
        "--no-sandbox".into(),
        "--no-first-run".into(),
        "--no-default-browser-check".into(),
        "--disable-extensions".into(),
        "--hide-scrollbars".into(),
        format!("--user-data-dir={}", profile.display()),
    ];
    a.extend_from_slice(extra);
    a
}

/// 起 Chrome 子进程。`kill_on_drop`：future 被弃时杀子进程，杜绝残留 Chrome。
fn spawn_chrome(chrome: &str, args: &[String], stdout: std::process::Stdio) -> Result<tokio::process::Child, String> {
    tokio::process::Command::new(chrome)
        .args(args)
        .stdout(stdout)
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Chrome 启动失败：{e}"))
}

/// 轮询 `path` 直到写稳（相邻两次大小 >0 且一致）或子进程自退或超时，随后杀进程。返回是否拿到输出。
/// Chrome `--dump-dom`/`--screenshot` 常写完输出后**不自退**，故不等 exit、只等输出文件稳定即收工。
async fn poll_output_then_kill(child: &mut tokio::process::Child, path: &Path, timeout_ms: u64) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        let self_exited = matches!(child.try_wait(), Ok(Some(_)));
        let s1 = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        if s1 > 0 {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let s2 = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if s1 == s2 {
                ok = true;
                break;
            }
        }
        if self_exited {
            ok = s1 > 0;
            break;
        }
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    let _ = child.start_kill();
    let _ = child.wait().await;
    ok
}

/// 无头渲染并返回 JS 执行后的 DOM（`--dump-dom` → 临时文件）。测试可用 file:// URL 直接验管线。
pub(crate) async fn render_dom(url: &str, wait_ms: u64) -> Result<String, String> {
    let chrome = find_chrome().ok_or("未找到本机 Chrome/Chromium（可设 env CMX_AGENT_CHROME 指定）")?;
    let profile = tmp_profile();
    let dom_file = profile.with_extension("dom.html");
    let f = std::fs::File::create(&dom_file).map_err(|e| format!("临时文件失败：{e}"))?;
    let args = base_args(&profile, &["--dump-dom".into(), url.to_string()]);
    let mut child = match spawn_chrome(&chrome, &args, std::process::Stdio::from(f)) {
        Ok(c) => c,
        Err(e) => {
            let _ = std::fs::remove_file(&dom_file);
            return Err(e);
        }
    };
    let ok = poll_output_then_kill(&mut child, &dom_file, wait_ms + 15000).await;
    let dom = std::fs::read_to_string(&dom_file).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&profile);
    let _ = std::fs::remove_file(&dom_file);
    if !ok || dom.trim().is_empty() {
        return Err("空 DOM（页面加载失败或超时）".into());
    }
    Ok(dom)
}

/// 无头截图到 `out_path`（PNG）。同样轮询文件出现即收工（Chrome 写完 PNG 常不自退）。
pub(crate) async fn render_screenshot(url: &str, out_path: &Path, w: u32, h: u32, wait_ms: u64) -> Result<(), String> {
    let chrome = find_chrome().ok_or("未找到本机 Chrome/Chromium（可设 env CMX_AGENT_CHROME 指定）")?;
    let profile = tmp_profile();
    let _ = std::fs::remove_file(out_path);
    let args = base_args(
        &profile,
        &[
            format!("--screenshot={}", out_path.display()),
            format!("--window-size={w},{h}"),
            url.to_string(),
        ],
    );
    let mut child = spawn_chrome(&chrome, &args, std::process::Stdio::null())?;
    let ok = poll_output_then_kill(&mut child, out_path, wait_ms + 15000).await;
    let _ = std::fs::remove_dir_all(&profile);
    if ok && out_path.exists() {
        Ok(())
    } else {
        Err("截图未生成或超时".into())
    }
}

// ── 沙箱输出路径解析（截图落工作区）：镜像 cmx-agent-office 的 canonicalize_partial ──
fn resolve_out(path: &str, ctx: &ToolCtx) -> Result<PathBuf, String> {
    if ctx.allowed_roots.is_empty() {
        return Err("no allowed_roots".into());
    }
    let raw = PathBuf::from(path);
    let abs = if raw.is_absolute() {
        raw
    } else {
        ctx.allowed_roots[0].join(raw)
    };
    let canon = canonicalize_partial(&abs);
    let ok = ctx.allowed_roots.iter().any(|r| {
        let r = std::fs::canonicalize(r).unwrap_or_else(|_| normalize(r));
        canon.starts_with(&r)
    });
    if ok {
        Ok(abs)
    } else {
        Err(format!("path '{path}' escapes sandbox"))
    }
}
fn canonicalize_partial(p: &Path) -> PathBuf {
    for anc in p.ancestors() {
        if let Ok(c) = std::fs::canonicalize(anc) {
            if let Ok(rest) = p.strip_prefix(anc) {
                return c.join(rest);
            }
            return c;
        }
    }
    normalize(p)
}
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            o => out.push(o.as_os_str()),
        }
    }
    out
}

/// `browser_read`：Chrome 无头渲染(执行 JS)后取正文。`allow_private`：放开私网(builder 从 env 读)。
#[derive(Default)]
pub struct BrowserReadTool {
    pub allow_private: bool,
}
impl BrowserReadTool {
    pub fn from_env() -> Self {
        Self {
            allow_private: std::env::var("CMX_AGENT_NET_ALLOW_PRIVATE").is_ok(),
        }
    }
}

#[async_trait]
impl Tool for BrowserReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "browser_read",
            "用本机 Chrome 无头渲染网页(执行 JS)后返回可读正文——用于 web_fetch 抓不到的动态/SPA/JS 页面。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "网页 URL（http/https）" },
                "wait_ms": { "type": "integer", "default": 4000, "description": "等 JS 渲染的虚拟时间预算(ms)" },
                "max_chars": { "type": "integer", "default": 8000 }
            },
            "required": ["url"]
        }))
        .guard(GuardHints {
            requires_auth: Some("net:browser".into()),
            idempotent: true,
            network: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(url) = input.get("url").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("browser_read: 'url' 必填"));
        };
        let wait_ms = input.get("wait_ms").and_then(|v| v.as_u64()).unwrap_or(4000).clamp(500, 30000);
        let max_chars = input.get("max_chars").and_then(|v| v.as_u64()).unwrap_or(8000) as usize;
        let u = match ensure_public_url(url, self.allow_private) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::err(format!("browser_read: {e}"))),
        };
        let dom = match render_dom(u.as_str(), wait_ms).await {
            Ok(d) => d,
            Err(e) => return Ok(ToolResult::err(format!("browser_read: {e}"))),
        };
        let title = html::extract_title(&dom);
        let text = html::html_to_text(&dom);
        let chars = text.chars().count();
        Ok(ToolResult::ok(json!({
            "url": u.as_str(),
            "title": title,
            "text": html::truncate(&text, max_chars),
            "chars": chars,
            "rendered": true,
        })))
    }
}

/// `browser_screenshot`：Chrome 无头把网页截图存工作区(PNG)。需 workspace-write。
#[derive(Default)]
pub struct BrowserScreenshotTool {
    pub allow_private: bool,
}
impl BrowserScreenshotTool {
    pub fn from_env() -> Self {
        Self {
            allow_private: std::env::var("CMX_AGENT_NET_ALLOW_PRIVATE").is_ok(),
        }
    }
}

#[async_trait]
impl Tool for BrowserScreenshotTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "browser_screenshot",
            "用本机 Chrome 无头把网页截图为 PNG 存到工作区。给 path(工作区内.png) + 可选 width/height。需 workspace-write。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "网页 URL（http/https）" },
                "path": { "type": "string", "description": "工作区内输出 PNG 路径" },
                "width": { "type": "integer", "default": 1280 },
                "height": { "type": "integer", "default": 900 },
                "wait_ms": { "type": "integer", "default": 4000 }
            },
            "required": ["url", "path"]
        }))
        .guard(GuardHints {
            requires_auth: Some("net:browser".into()),
            idempotent: false,
            network: true,
            writes: true, // 截图落盘工作区
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        if !ctx.sandbox.allows_write() {
            return Ok(ToolResult::err("browser_screenshot: 需 workspace-write 沙箱"));
        }
        let Some(url) = input.get("url").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("browser_screenshot: 'url' 必填"));
        };
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("browser_screenshot: 'path' 必填"));
        };
        let w = input.get("width").and_then(|v| v.as_u64()).unwrap_or(1280).clamp(200, 3840) as u32;
        let h = input.get("height").and_then(|v| v.as_u64()).unwrap_or(900).clamp(200, 3840) as u32;
        let wait_ms = input.get("wait_ms").and_then(|v| v.as_u64()).unwrap_or(4000).clamp(500, 30000);
        let u = match ensure_public_url(url, self.allow_private) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::err(format!("browser_screenshot: {e}"))),
        };
        let abs = match resolve_out(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("browser_screenshot: {e}"))),
        };
        match render_screenshot(u.as_str(), &abs, w, h, wait_ms).await {
            Ok(()) => {
                let bytes = std::fs::metadata(&abs).map(|m| m.len()).unwrap_or(0);
                Ok(ToolResult::ok(json!({
                    "url": u.as_str(), "path": abs.display().to_string(),
                    "bytes": bytes, "width": w, "height": h
                })))
            }
            Err(e) => Ok(ToolResult::err(format!("browser_screenshot: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp_html() -> PathBuf {
        let p = std::env::temp_dir().join(format!("cmx-u8-{}.html", std::process::id()));
        std::fs::write(
            &p,
            "<!DOCTYPE html><html><head><title>U8 页</title></head><body>\
             <h1 id='h'>加载中</h1><p>静态段落</p>\
             <script>document.getElementById('h').textContent='JS已执行';\
             var d=document.createElement('div');d.textContent='动态插入内容';document.body.appendChild(d);</script>\
             </body></html>",
        )
        .unwrap();
        p
    }

    #[test]
    fn escapes_sandbox_rejected() {
        let roots = vec![PathBuf::from("/tmp/cmx-u8-root")];
        std::fs::create_dir_all(&roots[0]).ok();
        let ctx = ToolCtx { sandbox: cmx_agent_core::guard::SandboxMode::WorkspaceWrite, allowed_roots: &roots, session_id: "test" };
        assert!(resolve_out("shot.png", &ctx).is_ok());
        assert!(resolve_out("../escape.png", &ctx).is_err());
    }

    // 真机 Chrome 管线：用 file:// 验 JS 渲染 + 截图（本环境 http 被墙，file 可用）。无 Chrome 则跳过。
    #[tokio::test]
    async fn chrome_renders_js_and_screenshots_via_file_url() {
        if find_chrome().is_none() {
            eprintln!("未找到 Chrome，跳过 U8 真机测试");
            return;
        }
        let html_path = write_tmp_html();
        let file_url = format!("file://{}", html_path.display());

        // dump-dom：应含 JS 执行后的内容
        let dom = render_dom(&file_url, 3000).await.expect("render_dom");
        assert!(dom.contains("JS已执行"), "JS 未执行：{}", &dom[..dom.len().min(200)]);
        assert!(dom.contains("动态插入内容"));
        let text = html::html_to_text(&dom);
        assert!(text.contains("静态段落"));
        assert_eq!(html::extract_title(&dom), "U8 页");

        // 截图：应生成 PNG
        let shot = std::env::temp_dir().join(format!("cmx-u8-{}.png", std::process::id()));
        render_screenshot(&file_url, &shot, 640, 480, 3000).await.expect("screenshot");
        let bytes = std::fs::metadata(&shot).map(|m| m.len()).unwrap_or(0);
        assert!(bytes > 500, "PNG 太小：{bytes}");
        assert_eq!(&std::fs::read(&shot).unwrap()[..4], b"\x89PNG");

        std::fs::remove_file(&html_path).ok();
        std::fs::remove_file(&shot).ok();
    }
}
