//! `computer_use`：**视觉回环** computer-use。给一个 URL + 自然语言目标，工具自跑循环：
//! 截图 → 视觉模型决策(下一步动作) → 经 CDP 坐标点击/输入/滚动执行 → 再截图 …… 直到 `done` 或步数上限。
//!
//! 视觉在**工具内部**（[`crate::vision`] 客户端），不动 agent 的文本 ModelSeam：文本 agent 只需
//! `computer_use(url, goal)` 交办，拿回轨迹 + 结论。视觉模型配置由 builder 注入（[`VisionCfg`]）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::cdp::Cdp;
use crate::interact::{attach_page, launch_cdp_chrome};
use crate::vision::{Action, VisionCfg, VisionClient};

const VW: u32 = 1280;
const VH: u32 = 900;

/// 截图 → base64 PNG。
pub(crate) async fn screenshot(cdp: &mut Cdp, sid: &str) -> Result<String, String> {
    let r = cdp
        .call("Page.captureScreenshot", json!({ "format": "png" }), Some(sid))
        .await?;
    r.get("data")
        .and_then(|d| d.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "截图无 data".into())
}

/// 坐标点击（按下 + 抬起）。
pub(crate) async fn mouse_click(cdp: &mut Cdp, sid: &str, x: f64, y: f64) -> Result<(), String> {
    cdp.call(
        "Input.dispatchMouseEvent",
        json!({ "type":"mousePressed","x":x,"y":y,"button":"left","buttons":1,"clickCount":1 }),
        Some(sid),
    )
    .await?;
    cdp.call(
        "Input.dispatchMouseEvent",
        json!({ "type":"mouseReleased","x":x,"y":y,"button":"left","buttons":1,"clickCount":1 }),
        Some(sid),
    )
    .await?;
    Ok(())
}

/// 在当前焦点插入文本。
pub(crate) async fn type_text(cdp: &mut Cdp, sid: &str, text: &str) -> Result<(), String> {
    cdp.call("Input.insertText", json!({ "text": text }), Some(sid)).await?;
    Ok(())
}

/// 按一个键（Enter/Tab 带虚拟键码，其余按 key 名）。
pub(crate) async fn press_key(cdp: &mut Cdp, sid: &str, key: &str) -> Result<(), String> {
    let (code, vk, text) = match key {
        "Enter" => ("Enter", 13, "\r"),
        "Tab" => ("Tab", 9, "\t"),
        "Backspace" => ("Backspace", 8, ""),
        _ => (key, 0, ""),
    };
    let down = json!({ "type":"keyDown","key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":vk,"text":text });
    let up = json!({ "type":"keyUp","key":key,"code":code,"windowsVirtualKeyCode":vk,"nativeVirtualKeyCode":vk });
    cdp.call("Input.dispatchKeyEvent", down, Some(sid)).await?;
    cdp.call("Input.dispatchKeyEvent", up, Some(sid)).await?;
    Ok(())
}

/// 滚轮滚动。
pub(crate) async fn scroll(cdp: &mut Cdp, sid: &str, dy: f64) -> Result<(), String> {
    cdp.call(
        "Input.dispatchMouseEvent",
        json!({ "type":"mouseWheel","x":(VW/2) as f64,"y":(VH/2) as f64,"deltaX":0,"deltaY":dy }),
        Some(sid),
    )
    .await?;
    Ok(())
}

/// 固定视口尺寸（截图坐标与模型看到的一致）。
pub(crate) async fn set_viewport(cdp: &mut Cdp, sid: &str) -> Result<(), String> {
    cdp.call(
        "Emulation.setDeviceMetricsOverride",
        json!({ "width":VW,"height":VH,"deviceScaleFactor":1,"mobile":false }),
        Some(sid),
    )
    .await
    .map(|_| ())
}

pub struct ComputerUseTool {
    pub allow_private: bool,
    pub vision: Option<VisionCfg>,
}

impl ComputerUseTool {
    pub fn new(allow_private: bool, vision: Option<VisionCfg>) -> Self {
        Self { allow_private, vision }
    }
}

#[async_trait]
impl Tool for ComputerUseTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "computer_use",
            "视觉 computer-use：给 url + 自然语言 goal，工具自跑「截图→视觉模型决策→坐标点击/输入」回环直到达成。\
             适合无稳定选择器、必须看画面操作的网站。需已配视觉模型。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "url": { "type": "string", "description": "起始网页 URL（http/https）" },
                "goal": { "type": "string", "description": "自然语言目标，如「搜索 rust 并打开第一条结果」" },
                "max_steps": { "type": "integer", "default": 8, "description": "最多回环步数（防失控）" }
            },
            "required": ["url", "goal"]
        }))
        .guard(GuardHints {
            requires_auth: Some("net:browser".into()),
            idempotent: false, // 会点击/提交，有副作用
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(vision) = self.vision.clone() else {
            return Ok(ToolResult::err(
                "computer_use: 未配置视觉模型（设 CMX_AGENT_VISION_MODEL 且模型/密钥已就绪）",
            ));
        };
        let Some(url) = input.get("url").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("computer_use: 'url' 必填"));
        };
        let Some(goal) = input.get("goal").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("computer_use: 'goal' 必填"));
        };
        let max_steps = input.get("max_steps").and_then(|v| v.as_u64()).unwrap_or(8).clamp(1, 20) as usize;
        let u = match crate::ensure_public_url(url, self.allow_private) {
            Ok(u) => u,
            Err(e) => return Ok(ToolResult::err(format!("computer_use: {e}"))),
        };

        match run_loop(u.as_str(), goal, max_steps, &VisionClient::new(vision)).await {
            Ok((actions, answer, done)) => Ok(ToolResult::ok(json!({
                "url": u.as_str(),
                "goal": goal,
                "steps": actions.len(),
                "actions": actions,
                "answer": answer,
                "done": done,
                "computer_use": true,
            }))),
            Err(e) => Ok(ToolResult::err(format!("computer_use: {e}"))),
        }
    }
}

async fn run_loop(
    url: &str,
    goal: &str,
    max_steps: usize,
    vision: &VisionClient,
) -> Result<(Vec<String>, String, bool), String> {
    let (mut child, ws, profile) = launch_cdp_chrome(url).await?;
    let out = loop_inner(&ws, goal, max_steps, vision).await;
    let _ = child.start_kill();
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&profile);
    out
}

/// 收集视口内可见可交互元素（`#i tag 'label' @(cx,cy)`），供视觉模型精确定位（不必纯靠估坐标）。
pub(crate) async fn collect_elements(cdp: &mut Cdp, sid: &str) -> String {
    let js = r#"(()=>{const sel='a,button,input,select,textarea,[role=button],[onclick],summary';const out=[];let i=0;
      for(const e of document.querySelectorAll(sel)){const r=e.getBoundingClientRect();
        if(r.width<2||r.height<2||r.bottom<0||r.right<0||r.top>innerHeight||r.left>innerWidth)continue;
        const st=getComputedStyle(e); if(st.visibility==='hidden'||st.display==='none'||st.opacity==='0')continue;
        let l=(e.getAttribute('aria-label')||e.value||e.placeholder||e.innerText||e.getAttribute('title')||e.name||e.tagName||'').trim().replace(/\s+/g,' ').slice(0,40);
        out.push('#'+(i++)+' '+e.tagName.toLowerCase()+" '"+l+"' @("+Math.round(r.left+r.width/2)+','+Math.round(r.top+r.height/2)+')');
        if(i>=25)break;}
      return out.join('\n');})()"#;
    eval_str(cdp, sid, js).await
}

/// 页面签名（URL|标题|正文长度）——检测动作是否引起变化。
pub(crate) async fn page_sig(cdp: &mut Cdp, sid: &str) -> String {
    eval_str(cdp, sid, "location.href+'|'+document.title+'|'+(document.body?document.body.innerText.length:0)").await
}

/// 坐标点击并**吸附到最近可点击元素中心**（纠正视觉小偏差）；返回实际点击坐标。
pub(crate) async fn snap_and_click(cdp: &mut Cdp, sid: &str, x: f64, y: f64) -> Result<(f64, f64), String> {
    let js = format!(
        "(()=>{{let e=document.elementFromPoint({x},{y});if(!e)return null;let t=e;\
         for(let i=0;i<4&&t;i++){{if(t.matches('a,button,input,select,textarea,[role=button],[onclick],summary')||t.onclick){{const r=t.getBoundingClientRect();return [r.left+r.width/2,r.top+r.height/2];}}t=t.parentElement;}}\
         const r=e.getBoundingClientRect();return [r.left+r.width/2,r.top+r.height/2];}})()"
    );
    let snapped = cdp
        .call("Runtime.evaluate", json!({"expression":js,"returnByValue":true}), Some(sid))
        .await
        .ok()
        .and_then(|v| {
            v.pointer("/result/value").and_then(|a| a.as_array()).map(|a| {
                (
                    a.first().and_then(|n| n.as_f64()).unwrap_or(x),
                    a.get(1).and_then(|n| n.as_f64()).unwrap_or(y),
                )
            })
        });
    let (cx, cy) = snapped.unwrap_or((x, y));
    mouse_click(cdp, sid, cx, cy).await?;
    Ok((cx, cy))
}

async fn eval_str(cdp: &mut Cdp, sid: &str, expr: &str) -> String {
    cdp.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true }), Some(sid))
        .await
        .ok()
        .and_then(|v| v.pointer("/result/value").and_then(|x| x.as_str()).map(|s| s.to_string()))
        .unwrap_or_default()
}

async fn wait_ready(cdp: &mut Cdp, sid: &str) {
    for _ in 0..25 {
        let s = eval_str(cdp, sid, "document.readyState").await;
        if s == "complete" || s == "interactive" {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
    }
}

async fn loop_inner(
    ws: &str,
    goal: &str,
    max_steps: usize,
    vision: &VisionClient,
) -> Result<(Vec<String>, String, bool), String> {
    let mut cdp = Cdp::connect(ws).await?;
    let sid = attach_page(&mut cdp).await?;
    let _ = set_viewport(&mut cdp, &sid).await;
    let _ = cdp.call("Page.enable", json!({}), Some(&sid)).await;
    wait_ready(&mut cdp, &sid).await;

    let mut history: Vec<String> = Vec::new();
    let mut last_sig = page_sig(&mut cdp, &sid).await;
    let mut no_change = 0u32;
    let mut change_note = String::new();

    for _ in 0..max_steps {
        let shot = screenshot(&mut cdp, &sid).await?;
        let elems = collect_elements(&mut cdp, &sid).await;
        let info = eval_str(&mut cdp, &sid, "location.href+' | '+document.title").await;
        let page_ctx = format!(
            "当前页面：{info}\n可交互元素(编号 标签 @中心坐标)：\n{}\n{change_note}",
            if elems.is_empty() { "（未探测到）".to_string() } else { elems }
        );

        let action = vision.decide(&shot, goal, &history, &page_ctx, VW, VH).await?;

        // 执行：动作失败反馈回历史让模型自愈，不中止整个工具。
        let exec: Result<String, String> = match &action {
            Action::Done { answer } => {
                history.push("done".into());
                return Ok((history, answer.clone(), true));
            }
            Action::Click { x, y } => snap_and_click(&mut cdp, &sid, *x, *y)
                .await
                .map(|(cx, cy)| format!("click({cx:.0},{cy:.0})")),
            Action::Type { text } => type_text(&mut cdp, &sid, text)
                .await
                .map(|_| format!("type({})", text.chars().take(20).collect::<String>())),
            Action::Key { key } => press_key(&mut cdp, &sid, key).await.map(|_| format!("key({key})")),
            Action::Scroll { dy } => scroll(&mut cdp, &sid, *dy).await.map(|_| format!("scroll({dy:.0})")),
            Action::Wait { ms } => {
                tokio::time::sleep(std::time::Duration::from_millis((*ms).min(30000))).await;
                Ok(format!("wait({ms})"))
            }
        };
        match exec {
            Ok(desc) => history.push(desc),
            Err(e) => {
                history.push(format!("失败:{e}"));
                change_note = format!("上一步失败：{e}");
                continue;
            }
        }

        // 等页面反应；若导航则等加载
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        wait_ready(&mut cdp, &sid).await;

        // 变化检测 + 无进展止损
        let sig = page_sig(&mut cdp, &sid).await;
        if sig == last_sig {
            no_change += 1;
            change_note = if no_change >= 2 {
                "上一步后页面连续无明显变化——请换个位置/元素，或若目标已达成就 done。".to_string()
            } else {
                "上一步后页面无明显变化。".to_string()
            };
            if no_change >= 3 {
                break; // 连续 3 步无进展，止损防死循环
            }
        } else {
            no_change = 0;
            change_note.clear();
            last_sig = sig;
        }
    }
    Ok((history, String::new(), false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::find_chrome;

    // 真机 CDP 视觉原语（无视觉模型）：file:// 页 → 截图非空 + 坐标点击命中按钮 + 输入生效。
    #[tokio::test]
    async fn cdp_visual_primitives_via_file_url() {
        if find_chrome().is_none() {
            eprintln!("未找到 Chrome，跳过 computer_use 原语测试");
            return;
        }
        // 按钮绝对定位在 (40,40)-(160,80)；点击中心 (100,60) 应触发 onclick。
        let p = std::env::temp_dir().join(format!("cmx-cu-{}.html", std::process::id()));
        std::fs::write(
            &p,
            "<!DOCTYPE html><html><head><title>CU</title></head><body style='margin:0'>\
             <button style='position:absolute;left:40px;top:40px;width:120px;height:40px' \
             onclick=\"document.title='CLICKED'\">B</button>\
             <input id='i' style='position:absolute;left:40px;top:120px;width:200px' value=''>\
             </body></html>",
        )
        .unwrap();
        let url = format!("file://{}", p.display());
        let (mut child, ws, profile) = launch_cdp_chrome(&url).await.expect("launch");
        let r = async {
            let mut cdp = Cdp::connect(&ws).await?;
            let sid = attach_page(&mut cdp).await?;
            set_viewport(&mut cdp, &sid).await?;
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;

            // 截图非空
            let shot = screenshot(&mut cdp, &sid).await?;
            assert!(shot.len() > 500, "截图 base64 过短：{}", shot.len());

            // 坐标点击按钮中心 → onclick 改标题
            mouse_click(&mut cdp, &sid, 100.0, 60.0).await?;
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            let title = cdp
                .call("Runtime.evaluate", json!({"expression":"document.title","returnByValue":true}), Some(&sid))
                .await?;
            assert_eq!(title.pointer("/result/value").and_then(|v| v.as_str()), Some("CLICKED"), "坐标点击未命中按钮");

            // 点击输入框聚焦 → 输入
            mouse_click(&mut cdp, &sid, 140.0, 132.0).await?;
            type_text(&mut cdp, &sid, "视觉输入").await?;
            let val = cdp
                .call("Runtime.evaluate", json!({"expression":"document.getElementById('i').value","returnByValue":true}), Some(&sid))
                .await?;
            assert_eq!(val.pointer("/result/value").and_then(|v| v.as_str()), Some("视觉输入"), "坐标输入未生效");
            Ok::<(), String>(())
        }
        .await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::remove_file(&p).ok();
        r.expect("cdp 视觉原语");
    }

    // 精度打磨：元素表收集 + 点击吸附到元素中心（真机 file://）。
    #[tokio::test]
    async fn element_hints_and_click_snapping() {
        if find_chrome().is_none() {
            eprintln!("未找到 Chrome，跳过吸附测试");
            return;
        }
        // 按钮 (40,40)-(160,80)，中心 (100,60)；onclick 记录命中坐标到标题。
        let p = std::env::temp_dir().join(format!("cmx-snap-{}.html", std::process::id()));
        std::fs::write(
            &p,
            "<!DOCTYPE html><html><head><title>init</title></head><body style='margin:0'>\
             <button id='b' style='position:absolute;left:40px;top:40px;width:120px;height:40px' \
             onclick=\"document.title=Math.round(event.clientX)+','+Math.round(event.clientY)\">登录</button>\
             </body></html>",
        )
        .unwrap();
        let url = format!("file://{}", p.display());
        let (mut child, ws, profile) = launch_cdp_chrome(&url).await.expect("launch");
        let r = async {
            let mut cdp = Cdp::connect(&ws).await?;
            let sid = attach_page(&mut cdp).await?;
            set_viewport(&mut cdp, &sid).await?;
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;

            // 元素表应含 button + 中心坐标
            let elems = collect_elements(&mut cdp, &sid).await;
            assert!(elems.contains("button") && elems.contains("@(100,60)"), "元素表缺 button@中心：{elems}");

            // 在按钮内偏左上 (55,48) 点击 → 吸附到中心 (100,60)
            let (cx, cy) = snap_and_click(&mut cdp, &sid, 55.0, 48.0).await?;
            assert_eq!((cx as i32, cy as i32), (100, 60), "未吸附到元素中心");
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let title = eval_str(&mut cdp, &sid, "document.title").await;
            assert_eq!(title, "100,60", "点击未落在吸附后的中心：{title}");
            Ok::<(), String>(())
        }
        .await;
        let _ = child.start_kill();
        let _ = child.wait().await;
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::remove_file(&p).ok();
        r.expect("吸附");
    }

    // 全回环 live E2E（需网络 + 视觉模型 + model.json）：手动 `cargo test -p cmx-agent-net --ignored` 跑。
    #[tokio::test]
    #[ignore]
    async fn live_computer_use_loop() {
        if find_chrome().is_none() {
            return;
        }
        let home = std::env::var("HOME").unwrap();
        let mj = std::path::Path::new(&home).join("Library/Application Support/com.pansoft.cmx-agent/model.json");
        let Ok(s) = std::fs::read_to_string(&mj) else {
            eprintln!("无 model.json，跳过 live");
            return;
        };
        let cfg: Value = serde_json::from_str(&s).unwrap();
        let vc = VisionCfg {
            base_url: cfg["base_url"].as_str().unwrap_or("https://api.deepseek.com").to_string(),
            api_key: cfg["api_key"].as_str().unwrap_or("").to_string(),
            model: "deepseek-v4-flash-vision-exp".to_string(),
        };
        let p = std::env::temp_dir().join("cmx-cu-live.html");
        std::fs::write(
            &p,
            "<!DOCTYPE html><html><head><meta charset=utf-8></head><body style='font-family:sans-serif;padding:30px'>\
             <h1 style='color:#1a73e8'>登录 cmx 系统</h1>\
             <input placeholder='用户名' style='display:block;width:260px;padding:10px;margin:12px 0;font-size:16px'>\
             <button style='background:#1a73e8;color:#fff;padding:12px 30px;font-size:16px;border:0;border-radius:6px'>登 录</button>\
             </body></html>",
        )
        .unwrap();
        let url = format!("file://{}", p.display());
        let (actions, answer, done) = run_loop(&url, "点击页面上的登录按钮", 2, &VisionClient::new(vc))
            .await
            .expect("run_loop");
        eprintln!("live computer_use → actions={actions:?} answer={answer:?} done={done}");
        assert!(actions.iter().any(|a| a.starts_with("click")), "视觉应至少点击一次：{actions:?}");
        std::fs::remove_file(&p).ok();
    }
}
