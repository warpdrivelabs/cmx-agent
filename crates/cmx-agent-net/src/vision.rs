//! 视觉决策客户端（computer-use）：把「网页截图 + 目标」发给视觉模型，要一个 JSON **下一步动作**回来。
//! OpenAI 兼容 `chat/completions`（`content` 数组含 `image_url` data URL）。配置由 builder 从模型配置注入。

use serde_json::{Value, json};

/// 视觉模型配置（builder 从 `ModelProviderConfig` 注入；`CMX_AGENT_VISION_MODEL` 可覆盖模型名）。
#[derive(Clone, Debug)]
pub struct VisionCfg {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
}

/// 视觉模型给出的下一步动作。
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Click { x: f64, y: f64 },
    Type { text: String },
    Key { key: String },
    Scroll { dy: f64 },
    Wait { ms: u64 },
    Done { answer: String },
}

fn num(v: &Value) -> Option<f64> {
    v.as_f64()
}

/// 从模型回复里抽第一个**平衡的** JSON 对象（容忍 ```json 包裹/前后废话）。
fn extract_json(s: &str) -> Option<Value> {
    let start = s.find('{')?;
    let bytes = s.as_bytes();
    let mut depth = 0i32;
    let mut instr = false;
    let mut esc = false;
    for (i, &b) in bytes[start..].iter().enumerate() {
        let c = b as char;
        if instr {
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                instr = false;
            }
            continue;
        }
        match c {
            '"' => instr = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return serde_json::from_str(&s[start..start + i + 1]).ok();
                }
            }
            _ => {}
        }
    }
    None
}

/// 解析模型回复为 [`Action`]。
pub fn parse_action(reply: &str) -> Result<Action, String> {
    let v = extract_json(reply).ok_or("回复中未找到 JSON 动作")?;
    let act = v.get("action").and_then(|a| a.as_str()).unwrap_or("").to_lowercase();
    match act.as_str() {
        "click" => match (v.get("x").and_then(num), v.get("y").and_then(num)) {
            (Some(x), Some(y)) => Ok(Action::Click { x, y }),
            _ => Err("click 缺 x/y".into()),
        },
        "type" => Ok(Action::Type {
            text: v.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string(),
        }),
        "key" => Ok(Action::Key {
            key: v.get("key").and_then(|t| t.as_str()).unwrap_or("Enter").to_string(),
        }),
        "scroll" => Ok(Action::Scroll {
            dy: v.get("dy").and_then(num).unwrap_or(400.0),
        }),
        "wait" => Ok(Action::Wait {
            ms: v.get("ms").and_then(|m| m.as_u64()).unwrap_or(800),
        }),
        "done" => Ok(Action::Done {
            answer: v.get("answer").and_then(|t| t.as_str()).unwrap_or("").to_string(),
        }),
        other => Err(format!("未知动作：{other}")),
    }
}

pub struct VisionClient {
    cfg: VisionCfg,
    client: reqwest::Client,
}

impl VisionClient {
    pub fn new(cfg: VisionCfg) -> Self {
        Self {
            cfg,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(90))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        }
    }

    /// 构造一帧决策请求体（纯函数，可测）。`page_ctx`：URL/标题/可交互元素表/上一步变化提示。
    pub fn build_body(&self, png_b64: &str, goal: &str, history: &[String], page_ctx: &str, w: u32, h: u32) -> Value {
        let sys = "你是电脑操作助手。根据网页截图 + 可交互元素表，为达成用户目标决定**下一步单个动作**。\
                   坐标以截图左上角为原点、单位像素。**点击时优先用元素表里给的中心坐标**；元素表没有的才从图上估坐标。\
                   若上一步提示「无明显变化」，换个位置或判断是否已完成，别重复同一次点击。只输出一个 JSON，不要多余文字。可选动作：\
                   {\"action\":\"click\",\"x\":数,\"y\":数} 点击；\
                   {\"action\":\"type\",\"text\":\"...\"} 在当前焦点输入；\
                   {\"action\":\"key\",\"key\":\"Enter\"} 按键；\
                   {\"action\":\"scroll\",\"dy\":数} 滚动(正=向下)；\
                   {\"action\":\"wait\",\"ms\":数} 等待；\
                   {\"action\":\"done\",\"answer\":\"...\"} 目标达成并给结论。";
        let user_text = format!(
            "目标：{goal}\n视口：{w}x{h} 像素。\n{page_ctx}\n已执行：{}\n请给下一步 JSON 动作。",
            if history.is_empty() {
                "（无）".to_string()
            } else {
                history.join(" → ")
            }
        );
        json!({
            "model": self.cfg.model,
            "temperature": 0,
            "max_tokens": 1200,
            "messages": [
                { "role": "system", "content": sys },
                { "role": "user", "content": [
                    { "type": "text", "text": user_text },
                    { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{png_b64}") } }
                ]}
            ]
        })
    }

    /// 发一帧决策，返回下一步动作。
    pub async fn decide(&self, png_b64: &str, goal: &str, history: &[String], page_ctx: &str, w: u32, h: u32) -> Result<Action, String> {
        let body = self.build_body(png_b64, goal, history, page_ctx, w, h);
        let url = format!("{}/chat/completions", self.cfg.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .post(&url)
            .bearer_auth(&self.cfg.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("视觉请求失败：{e}"))?;
        let status = resp.status();
        let v: Value = resp.json().await.map_err(|e| format!("视觉响应解析失败：{e}"))?;
        if !status.is_success() {
            return Err(format!("视觉 HTTP {status}: {v}"));
        }
        // 推理型视觉模型（如 deepseek vision）动作在 content；若 content 空则退回 reasoning_content。
        let msg = v.pointer("/choices/0/message");
        let content = msg
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_str())
            .filter(|s| !s.trim().is_empty())
            .or_else(|| msg.and_then(|m| m.get("reasoning_content")).and_then(|c| c.as_str()))
            .unwrap_or("");
        parse_action(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_actions_from_messy_reply() {
        assert_eq!(
            parse_action("好的，下一步：\n```json\n{\"action\":\"click\",\"x\":120.5,\"y\":48}\n```").unwrap(),
            Action::Click { x: 120.5, y: 48.0 }
        );
        assert_eq!(
            parse_action("{\"action\":\"type\",\"text\":\"你好\"}").unwrap(),
            Action::Type { text: "你好".into() }
        );
        assert_eq!(
            parse_action("{\"action\":\"done\",\"answer\":\"完成了\"}").unwrap(),
            Action::Done { answer: "完成了".into() }
        );
        assert!(parse_action("没有 json").is_err());
        assert!(parse_action("{\"action\":\"click\"}").is_err()); // 缺坐标
    }

    #[test]
    fn build_body_has_image_and_model() {
        let c = VisionClient::new(VisionCfg {
            base_url: "https://x".into(),
            api_key: "k".into(),
            model: "vis".into(),
        });
        let b = c.build_body("QUJD", "找登录按钮", &["click(1,2)".into()], "当前URL: http://x\n可交互元素:\n#0 button '登录' @(150,292)", 1280, 900);
        assert_eq!(b["model"], "vis");
        let img = b.pointer("/messages/1/content/1/image_url/url").and_then(|u| u.as_str()).unwrap();
        assert!(img.starts_with("data:image/png;base64,QUJD"));
        let txt = b.pointer("/messages/1/content/0/text").and_then(|t| t.as_str()).unwrap();
        assert!(txt.contains("找登录按钮") && txt.contains("1280x900") && txt.contains("click(1,2)"));
        assert!(txt.contains("#0 button '登录'"), "page_ctx 应嵌入元素表");
    }
}
