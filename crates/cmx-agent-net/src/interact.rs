//! `browser_do`：打开网页并执行**交互**（点击 / 填表 / 等待）后返回页面文本。
//!
//! 用 [`crate::cdp`]（手写 CDP-over-WebSocket）驱动一个带 `--remote-debugging-port` 的无头 Chrome：
//! 附着到页面 target → 等加载 → 逐个动作用 `Runtime.evaluate` 执行 → 读回 `document.title`/`innerText`。
//! 用于需登录、搜索、点按的动态站点（`browser_read` 只能读、不能交互）。

use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::browser::{base_args, find_chrome, tmp_profile};
use crate::cdp::Cdp;
use crate::ensure_public_url;

fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// Runtime.evaluate（返回值透传）。抛 JS 异常则 Err。
async fn eval(cdp: &mut Cdp, sid: &str, expr: &str) -> Result<Value, String> {
    let r = cdp
        .call(
            "Runtime.evaluate",
            json!({ "expression": expr, "returnByValue": true, "awaitPromise": true }),
            Some(sid),
        )
        .await?;
    if let Some(exc) = r.get("exceptionDetails") {
        let msg = exc
            .get("exception")
            .and_then(|e| e.get("description"))
            .and_then(|d| d.as_str())
            .or_else(|| exc.get("text").and_then(|t| t.as_str()))
            .unwrap_or("JS 异常");
        return Err(msg.to_string());
    }
    Ok(r.get("result").and_then(|x| x.get("value")).cloned().unwrap_or(Value::Null))
}

/// 启动带调试端口的无头 Chrome，从 stderr 解析出 CDP ws url。
pub(crate) async fn launch_cdp_chrome(url: &str) -> Result<(tokio::process::Child, String, PathBuf), String> {
    let chrome = find_chrome().ok_or("未找到本机 Chrome/Chromium（可设 env CMX_AGENT_CHROME 指定）")?;
    let profile = tmp_profile();
    let args = base_args(&profile, &["--remote-debugging-port=0".into(), url.to_string()]);
    let mut child = tokio::process::Command::new(&chrome)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("Chrome 启动失败：{e}"))?;
    let stderr = child.stderr.take().ok_or("无 stderr 管道")?;
    let mut lines = BufReader::new(stderr).lines();
    let ws = tokio::time::timeout(Duration::from_secs(15), async {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(i) = line.find("ws://127.0.0.1:") {
                let u = &line[i..];
                let end = u.find(char::is_whitespace).unwrap_or(u.len());
                return Some(u[..end].to_string());
            }
        }
        None
    })
    .await
    .map_err(|_| "等 CDP ws 超时".to_string())?
    .ok_or("未从 Chrome stderr 解析到 ws url")?;
    // 继续排空 stderr，防管道写满导致 Chrome 阻塞（Chrome 被 kill 时该任务自然结束）。
    tokio::spawn(async move { while let Ok(Some(_)) = lines.next_line().await {} });
    Ok((child, ws, profile))
}

/// 附着到页面 target，返回 sessionId。
pub(crate) async fn attach_page(cdp: &mut Cdp) -> Result<String, String> {
    let targets = cdp.call("Target.getTargets", json!({}), None).await?;
    let tid = targets
        .get("targetInfos")
        .and_then(|a| a.as_array())
        .and_then(|a| a.iter().find(|t| t.get("type").and_then(|x| x.as_str()) == Some("page")))
        .and_then(|t| t.get("targetId").and_then(|x| x.as_str()))
        .ok_or("无 page target")?
        .to_string();
    let att = cdp
        .call("Target.attachToTarget", json!({ "targetId": tid, "flatten": true }), None)
        .await?;
    att.get("sessionId")
        .and_then(|x| x.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "attachToTarget 无 sessionId".into())
}

pub struct BrowserDoTool {
    pub allow_private: bool,
}
impl Default for BrowserDoTool {
    fn default() -> Self {
        Self::from_env()
    }
}
impl BrowserDoTool {
    pub fn from_env() -> Self {
        Self {
            allow_private: std::env::var("CMX_AGENT_NET_ALLOW_PRIVATE").is_ok(),
        }
    }
}

#[async_trait]
impl Tool for BrowserDoTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "browser_do",
            "打开网页并执行交互(点击/填表/等待)后返回页面文本——用于需搜索/点按/登录的动态站点。\
             actions 例：[{\"fill\":{\"selector\":\"#q\",\"text\":\"rust\"}},{\"click\":\"#go\"},{\"wait_ms\":800}]",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "起始 URL（http/https）" },
                "actions": {
                    "type": "array",
                    "description": "动作序列：{click:css选择器} / {fill:{selector,text}} / {wait_ms:毫秒}",
                    "items": { "type": "object" }
                },
                "max_chars": { "type": "integer", "default": 8000 }
            },
            "required": ["url", "actions"]
        }))
        .guard(GuardHints {
            requires_auth: Some("net:browser".into()),
            idempotent: false, // 交互有副作用（可能提交表单）
            network: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(url) = input.get("url").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("browser_do: 'url' 必填"));
        };
        let actions = input.get("actions").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        let max_chars = input.get("max_chars").and_then(|v| v.as_u64()).unwrap_or(8000) as usize;
        let u = match ensure_public_url(url, self.allow_private) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::err(format!("browser_do: {e}"))),
        };

        match drive(u.as_str(), &actions).await {
            Ok((title, text, steps)) => Ok(ToolResult::ok(json!({
                "url": u.as_str(),
                "title": title,
                "text": crate::html::truncate(&text, max_chars),
                "chars": text.chars().count(),
                "steps": steps,
                "interacted": true,
            }))),
            Err(e) => Ok(ToolResult::err(format!("browser_do: {e}"))),
        }
    }
}

async fn drive(url: &str, actions: &[Value]) -> Result<(String, String, usize), String> {
    let (mut child, ws, profile) = launch_cdp_chrome(url).await?;
    let result = drive_inner(&ws, actions).await;
    let _ = child.start_kill();
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&profile);
    result
}

async fn drive_inner(ws: &str, actions: &[Value]) -> Result<(String, String, usize), String> {
    let mut cdp = Cdp::connect(ws).await?;
    let sid = attach_page(&mut cdp).await?;
    let _ = cdp.call("Runtime.enable", json!({}), Some(&sid)).await;

    // 等加载完成（readyState complete/interactive，最多 ~6s）
    for _ in 0..60 {
        if let Ok(Value::String(s)) = eval(&mut cdp, &sid, "document.readyState").await
            && (s == "complete" || s == "interactive") {
                break;
            }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let mut steps = 0usize;
    for a in actions {
        if let Some(sel) = a.get("click").and_then(|v| v.as_str()) {
            let expr = format!(
                "(()=>{{const e=document.querySelector({s}); if(!e) throw new Error('未找到元素: '+{s}); e.scrollIntoView(); e.click(); return true;}})()",
                s = js_str(sel)
            );
            eval(&mut cdp, &sid, &expr).await?;
            steps += 1;
        } else if let Some(f) = a.get("fill").and_then(|v| v.as_object()) {
            let sel = f.get("selector").and_then(|v| v.as_str()).unwrap_or("");
            let text = f.get("text").and_then(|v| v.as_str()).unwrap_or("");
            let expr = format!(
                "(()=>{{const e=document.querySelector({s}); if(!e) throw new Error('未找到元素: '+{s}); e.focus(); e.value={t}; e.dispatchEvent(new Event('input',{{bubbles:true}})); e.dispatchEvent(new Event('change',{{bubbles:true}})); return true;}})()",
                s = js_str(sel),
                t = js_str(text)
            );
            eval(&mut cdp, &sid, &expr).await?;
            steps += 1;
        } else if let Some(ms) = a.get("wait_ms").and_then(|v| v.as_u64()) {
            tokio::time::sleep(Duration::from_millis(ms.min(30000))).await;
            steps += 1;
        }
        // 未知动作忽略
    }

    let res = eval(
        &mut cdp,
        &sid,
        "({t:document.title, x:(document.body?document.body.innerText:'')})",
    )
    .await?;
    let title = res.get("t").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let text = res.get("x").and_then(|v| v.as_str()).unwrap_or("").to_string();
    Ok((title, text, steps))
}

#[cfg(test)]
mod tests {
    use super::*;

    // 真机 CDP 交互：file:// 本地表单页 → fill + click → 读回结果文本。无 Chrome 则跳过。
    #[tokio::test]
    async fn cdp_fill_click_read() {
        if find_chrome().is_none() {
            eprintln!("未找到 Chrome，跳过 browser_do 真机测试");
            return;
        }
        let p = std::env::temp_dir().join(format!("cmx-do-{}.html", std::process::id()));
        std::fs::write(
            &p,
            "<!DOCTYPE html><html><head><title>表单</title></head><body>\
             <input id='q' value=''>\
             <button id='go' onclick=\"document.getElementById('out').textContent='结果:'+document.getElementById('q').value\">Go</button>\
             <div id='out'>未点击</div></body></html>",
        )
        .unwrap();
        let url = format!("file://{}", p.display());
        let actions = vec![
            json!({"fill":{"selector":"#q","text":"你好世界"}}),
            json!({"click":"#go"}),
            json!({"wait_ms":200}),
        ];
        let (title, text, steps) = drive(&url, &actions).await.expect("drive");
        assert_eq!(title, "表单");
        assert_eq!(steps, 3);
        assert!(text.contains("结果:你好世界"), "交互结果缺失：{text}");
        std::fs::remove_file(&p).ok();
    }
}
