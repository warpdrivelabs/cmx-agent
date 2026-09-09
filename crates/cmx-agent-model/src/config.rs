//! 模型 provider 配置（多 provider：DeepSeek / OpenAI / Qwen / 本地 Ollama/vLLM）。
//!
//! 沿用门户 `CMX_AI_*` 约定并新增 `CMX_AGENT_MODEL_*` 优先键，便于桌面壳单独配置。
//! `from_env()` 返回 `None` 表示未配置（此时壳回退到离线 `DemoModel`）。

/// 一个 OpenAI 兼容 provider 的连接配置。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelProviderConfig {
    /// 形如 `https://api.deepseek.com`（无尾斜杠，不含 `/chat/completions`）。
    pub base_url: String,
    /// Bearer API Key；本地 keyless 端点（Ollama/vLLM）可为空。
    pub api_key: String,
    /// 模型名，如 `deepseek-chat` / `gpt-4o` / `qwen-max`。
    pub model: String,
    /// 采样温度（agent 场景偏低以稳定工具调用）。
    #[serde(default = "default_temperature")]
    pub temperature: f32,
    /// 请求超时（毫秒）。
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_temperature() -> f32 {
    0.2
}

fn default_timeout_ms() -> u64 {
    60_000
}

impl ModelProviderConfig {
    fn env(keys: &[&str]) -> Option<String> {
        for k in keys {
            if let Ok(v) = std::env::var(k) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        None
    }

    /// 从环境变量装配。`None` = 未配置（无 API Key 且未显式指定 base_url）。
    ///
    /// 键优先级：`CMX_AGENT_MODEL_*`（桌面壳专用）> `CMX_AI_*`（门户约定）> provider 兜底。
    pub fn from_env() -> Option<Self> {
        let api_key = Self::env(&["CMX_AGENT_MODEL_API_KEY", "CMX_AI_API_KEY", "DEEPSEEK_API_KEY"])
            .unwrap_or_default();
        let base_override = Self::env(&["CMX_AGENT_MODEL_BASE_URL", "CMX_AI_BASE_URL"]);
        // 未配置：既无 key 又无显式 base_url（本地 keyless 端点必须显式给 base_url 才算配置）。
        if api_key.is_empty() && base_override.is_none() {
            return None;
        }
        let base_url = base_override
            .unwrap_or_else(|| "https://api.deepseek.com".to_string())
            .trim_end_matches('/')
            .to_string();
        let model = Self::env(&["CMX_AGENT_MODEL", "CMX_AI_MODEL"])
            .unwrap_or_else(|| "deepseek-chat".to_string());
        let temperature = Self::env(&["CMX_AGENT_MODEL_TEMPERATURE"])
            .and_then(|v| v.parse().ok())
            .unwrap_or(0.2);
        let timeout_ms = Self::env(&["CMX_AGENT_MODEL_TIMEOUT_MS"])
            .and_then(|v| v.parse().ok())
            .unwrap_or(60_000);
        Some(Self {
            base_url,
            api_key,
            model,
            temperature,
            timeout_ms,
        })
    }

    /// 便于测试/程序化构造。
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>, model: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key: api_key.into(),
            model: model.into(),
            temperature: 0.2,
            timeout_ms: 60_000,
        }
    }

    /// 从一个 JSON 配置文件解析（桌面壳持久化配置：`<data_dir>/model.json`）。
    /// 字段：`base_url`/`api_key`/`model`（必需其一有意义）+ 可选 `temperature`/`timeout_ms`。
    /// 文件不存在或无有效字段返回 `None`（此时回退 env 或 DemoModel）。
    pub fn from_json_str(s: &str) -> Option<Self> {
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let get = |k: &str| v.get(k).and_then(|x| x.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let api_key = get("api_key").unwrap_or_default();
        let base_url = get("base_url");
        if api_key.is_empty() && base_url.is_none() {
            return None;
        }
        Some(Self {
            base_url: base_url
                .unwrap_or_else(|| "https://api.deepseek.com".to_string())
                .trim_end_matches('/')
                .to_string(),
            api_key,
            model: get("model").unwrap_or_else(|| "deepseek-v4-flash".to_string()),
            temperature: v.get("temperature").and_then(|x| x.as_f64()).map(|f| f as f32).unwrap_or(0.2),
            timeout_ms: v.get("timeout_ms").and_then(|x| x.as_u64()).unwrap_or(60_000),
        })
    }

    /// 解析优先级：环境变量 > `<data_dir>/model.json` 文件 > None。
    /// 桌面壳（点击启动、无 shell env）靠文件；开发/CI 用 env 覆盖。
    pub fn resolve(data_dir: Option<&std::path::Path>) -> Option<Self> {
        if let Some(cfg) = Self::from_env() {
            return Some(cfg);
        }
        let dir = data_dir?;
        let path = dir.join("model.json");
        let content = std::fs::read_to_string(&path).ok()?;
        Self::from_json_str(&content)
    }

    /// 序列化为 model.json 内容（含敏感 api_key；调用方负责文件 mode 600、勿入 git）。
    pub fn to_json_string(&self) -> String {
        serde_json::json!({
            "base_url": self.base_url,
            "api_key": self.api_key,
            "model": self.model,
            "temperature": self.temperature,
            "timeout_ms": self.timeout_ms,
        })
        .to_string()
    }

    /// 写入 `<dir>/model.json`（覆盖）。模型选择器切换模型后持久化用。
    pub fn save(&self, dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::write(dir.join("model.json"), self.to_json_string())
    }

    /// 按 base_url 推断 provider 的候选模型名（模型选择器下拉用；未知 provider 返回空）。
    pub fn candidate_models(&self) -> Vec<&'static str> {
        let b = self.base_url.to_ascii_lowercase();
        if b.contains("mlamp") {
            vec![
                "mlamp/deepseek-v4-flash",
                "mlamp/qwen3-coder-next-fp8",
                "mlamp/deepseek-v4-pro",
                "mlamp/glm-5.2",
                "mlamp/kimi-k3",
                "mlamp/qwen3.8-27b",
                "mlamp/minimax-h3",
            ]
        } else if b.contains("deepseek") {
            vec!["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-r1", "deepseek-chat"]
        } else if b.contains("openai") {
            vec!["gpt-4o", "gpt-4o-mini", "o1", "o1-mini"]
        } else if b.contains("dashscope") || b.contains("qwen") || b.contains("aliyun") {
            vec!["qwen-max", "qwen-plus", "qwen-turbo"]
        } else {
            vec![]
        }
    }

    /// API Key 脱敏展示（前端配置面板用）：末 4 位明文，其余 `sk-...`；短于 8 字符全 `*`。
    pub fn masked_api_key(&self) -> String {
        let k = &self.api_key;
        if k.len() <= 8 {
            "*".repeat(k.len().max(1))
        } else {
            let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
            format!("sk-...{tail}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_trims_trailing_slash() {
        let c = ModelProviderConfig::new("http://127.0.0.1:9999/", "k", "m");
        assert_eq!(c.base_url, "http://127.0.0.1:9999");
        assert_eq!(c.temperature, 0.2);
    }

    #[test]
    fn from_json_str_reads_deepseek() {
        let c = ModelProviderConfig::from_json_str(
            r#"{"base_url":"https://api.deepseek.com","api_key":"sk-x","model":"deepseek-v4-flash"}"#,
        )
        .unwrap();
        assert_eq!(c.base_url, "https://api.deepseek.com");
        assert_eq!(c.api_key, "sk-x");
        assert_eq!(c.model, "deepseek-v4-flash");
    }

    #[test]
    fn from_json_str_empty_is_none() {
        assert!(ModelProviderConfig::from_json_str(r#"{"model":"x"}"#).is_none());
        assert!(ModelProviderConfig::from_json_str("not json").is_none());
    }

    #[test]
    fn resolve_reads_file_when_env_absent() {
        let dir = std::env::temp_dir().join(format!("cmx-mcfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("model.json"), r#"{"api_key":"sk-file","model":"deepseek-v4-pro"}"#).unwrap();
        // 注意：本测试假定运行环境未设 CMX_AGENT_MODEL_* / DEEPSEEK_API_KEY（CI 干净环境成立）
        if ModelProviderConfig::from_env().is_none() {
            let c = ModelProviderConfig::resolve(Some(&dir)).unwrap();
            assert_eq!(c.api_key, "sk-file");
            assert_eq!(c.model, "deepseek-v4-pro");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn masked_api_key_long() {
        let c = ModelProviderConfig::new("http://x", "sk-VIZBzFoeHvpS12kZhK6abcd", "m");
        let m = c.masked_api_key();
        assert!(m.starts_with("sk-..."), "should be prefixed: {m}");
        assert!(m.ends_with("abcd"), "should end with last 4: {m}");
        assert!(!m.contains("VIZBz"), "should not expose middle: {m}");
    }

    #[test]
    fn masked_api_key_short() {
        let c = ModelProviderConfig::new("http://x", "abc", "m");
        let m = c.masked_api_key();
        assert_eq!(m, "***");
    }

    #[test]
    fn masked_api_key_empty() {
        let c = ModelProviderConfig::new("http://x", "", "m");
        let m = c.masked_api_key();
        assert_eq!(m, "*");
    }
}
