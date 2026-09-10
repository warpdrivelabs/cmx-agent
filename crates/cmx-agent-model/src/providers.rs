//! 命名 provider 多配置（providers.json）。
//!
//! 「配置模型」面板支持多个命名 provider（内置预设不可删 + 用户自定义可增删），
//! 每条 [`NamedProvider`] 在 [`ModelProviderConfig`] 之上加 `id`/`name`/`builtin` 身份字段，
//! 整体持久化为 `<data_dir>/providers.json`（含 `active` 指针）。
//!
//! 兼容：providers.json 不存在时从 model.json / env 播种（懒播种，读路径无副作用）；
//! 解析链 env > providers.json(active) > model.json（env 优先维持既有约定，
//! 设了 `CMX_AGENT_MODEL_*` 的开发环境里面板改动重启后会被 env 盖过）。

use std::path::Path;

use crate::config::ModelProviderConfig;

/// 一条命名 provider 配置（providers.json 的一个元素）。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NamedProvider {
    /// 稳定标识：内置用 `builtin-mlamp`（旧版遗留 `builtin-deepseek`/`builtin-openai` 读入时剪除，
    /// 兼容播种 `builtin-default`），自定义用 `p-<nanos>`。
    pub id: String,
    /// 用户可见名称，如「我的Kimi」。必填非空（面板校验 + app 层兜底）。
    pub name: String,
    /// 内置条目（预设）不可删除，可编辑（填 key / 换模型）。
    #[serde(default)]
    pub builtin: bool,
    /// 连接配置（base_url / api_key / model / temperature / timeout_ms）。
    #[serde(flatten)]
    pub config: ModelProviderConfig,
}

/// `<dir>/providers.json` 的完整内容。
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ProviderFile {
    /// 当前激活的 provider id；None = 无激活（回退离线 DemoModel）。
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub providers: Vec<NamedProvider>,
}

/// 内置预设（不可删）。id 固定；base_url/model 预填，key 留空等用户填（**key 不入仓库**，
/// 开源分发下内置条目只预置地址，凭据仍由用户 GUI 填入本地 providers.json）。
pub const BUILTIN_PRESETS: &[(&str, &str, &str)] = &[("builtin-mlamp", "MLamp", "https://llmgw-bz.mlamp.cn/v1")];

/// 自定义条目 id（纳秒时间戳，无 uuid 依赖）。
pub fn new_id() -> String {
    format!("p-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0))
}

impl ProviderFile {
    /// 读 `<dir>/providers.json`；不存在/解析失败则**播种**（不落盘——写发生在首次 save）：
    /// 内置预设常驻；model.json 的 key/model/base_url 合入命中 base_url 的内置条目
    /// （未命中则作为「默认」内置条目追加）；model.json 未配置则尝试 env 同样合并；
    /// 都没有 → 空表（demo 兜底）。
    pub fn load(dir: &Path) -> Self {
        if let Ok(s) = std::fs::read_to_string(dir.join("providers.json"))
            && let Ok(mut pf) = serde_json::from_str::<ProviderFile>(&s)
        {
            pf.prune_removed_builtins();
            return Self::ensure_builtins(pf);
        }
        Self::seed(dir)
    }

    /// 旧版本内置条目剪除（2026-09-10 收敛为仅 MLamp 一条）：已发布的旧版文件里留有
    /// `builtin-deepseek`/`builtin-openai`，读入时删除；active 指向被剪条目时回落剩余首条。
    /// `ensure_builtins` 会把误剪的**现行**内置补回，故此处只管历史遗留 id。
    fn prune_removed_builtins(&mut self) {
        const REMOVED: [&str; 2] = ["builtin-deepseek", "builtin-openai"];
        let before = self.providers.len();
        self.providers.retain(|p| !REMOVED.contains(&p.id.as_str()));
        if self.providers.len() == before {
            return;
        }
        if let Some(a) = &self.active
            && REMOVED.contains(&a.as_str())
        {
            self.active = None; // 先清：指向被剪条目的激活失效
        }
        // active 失效（或本就指向被剪条目）→ 回落剩余首条。
        if self.active.is_none() {
            self.active = self.providers.first().map(|p| p.id.clone());
        }
    }

    /// 播种：内置预设打底 + model.json / env 合并。
    fn seed(dir: &Path) -> Self {
        let mut pf = Self::ensure_builtins(Self::default());
        // 旧 model.json / env 配置（哪个有值用哪个）合并成默认激活条目。
        let legacy = std::fs::read_to_string(dir.join("model.json"))
            .ok()
            .and_then(|s| ModelProviderConfig::from_json_str(&s))
            .or_else(ModelProviderConfig::from_env);
        if let Some(cfg) = legacy {
            // base_url 命中内置预设 → 把 key/model 并进去；否则追加「默认」条目。
            // 归一化比较：去 scheme/尾斜杠后互为前缀即视为同一 provider
            //（旧 model.json 常写 `https://api.deepseek.com`，预设是 `…/v1`）。
            let norm = |u: &str| {
                u.trim()
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_end_matches('/')
                    .to_ascii_lowercase()
            };
            let key = norm(&cfg.base_url);
            let hit = pf.providers.iter_mut().find(|p| {
                let b = norm(&p.config.base_url);
                !key.is_empty() && !b.is_empty() && (b.starts_with(&key) || key.starts_with(&b))
            });
            let id = match hit {
                Some(p) => {
                    p.config.api_key = cfg.api_key.clone();
                    p.config.model = cfg.model.clone();
                    p.id.clone()
                }
                None => {
                    let id = "builtin-default".to_string();
                    pf.providers.push(NamedProvider {
                        id: id.clone(),
                        name: "默认".into(),
                        builtin: true,
                        config: cfg,
                    });
                    id
                }
            };
            pf.active = Some(id);
        }
        pf
    }

    /// 补齐内置预设（已存在则不动；手工删过 providers.json 也能恢复）。
    fn ensure_builtins(mut self) -> Self {
        for (id, name, base) in BUILTIN_PRESETS {
            if !self.providers.iter().any(|p| &p.id == id) {
                let model = ModelProviderConfig {
                    base_url: base.to_string(),
                    api_key: String::new(),
                    model: String::new(),
                    temperature: 0.2,
                    timeout_ms: 60_000,
                }
                .candidate_models()
                .first()
                .map(|s| s.to_string())
                .unwrap_or_default();
                self.providers.push(NamedProvider {
                    id: id.to_string(),
                    name: name.to_string(),
                    builtin: true,
                    config: ModelProviderConfig {
                        base_url: base.to_string(),
                        api_key: String::new(),
                        model,
                        temperature: 0.2,
                        timeout_ms: 60_000,
                    },
                });
            }
        }
        self
    }

    /// 序列化落盘 `<dir>/providers.json`（临时文件 + rename 原子写）。
    /// **含明文 api_key**：调用方负责目录权限，勿入 git（同 model.json 约定）。
    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        let path = dir.join("providers.json");
        let tmp = dir.join(".providers.json.tmp");
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &path)
    }

    pub fn get(&self, id: &str) -> Option<&NamedProvider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut NamedProvider> {
        self.providers.iter_mut().find(|p| p.id == id)
    }

    /// 按 id 插入或替换；返回是否为新插入。
    pub fn upsert(&mut self, p: NamedProvider) -> bool {
        match self.providers.iter_mut().find(|x| x.id == p.id) {
            Some(slot) => {
                *slot = p;
                false
            }
            None => {
                self.providers.push(p);
                true
            }
        }
    }

    /// 按 id 删除；内置条目由调用方守卫（此处不拦）。返回被删条目。
    pub fn remove(&mut self, id: &str) -> Option<NamedProvider> {
        let i = self.providers.iter().position(|p| p.id == id)?;
        Some(self.providers.remove(i))
    }

    /// 当前激活的 provider id。
    pub fn active_id(&self) -> Option<&str> {
        self.active.as_deref()
    }

    /// 当前激活条目（active 指向已删条目时返回 None，调用方自行兜底）。
    pub fn active(&self) -> Option<&NamedProvider> {
        self.active.as_deref().and_then(|id| self.get(id))
    }

    /// 名称是否已被**其他**条目占用（重名校验用）。
    pub fn name_taken(&self, name: &str, exclude_id: Option<&str>) -> bool {
        self.providers.iter().any(|p| {
            p.name == name && exclude_id.map(|x| p.id != x).unwrap_or(true)
        })
    }
}

/// 启动期解析：env > providers.json(active) > model.json 兜底。
/// 设了 env 的开发环境里 env 恒优先（面板改动重启后不生效——既定约定，注释于 config.rs）。
pub fn resolve_active(dir: Option<&Path>) -> Option<ModelProviderConfig> {
    if let Some(cfg) = ModelProviderConfig::from_env() {
        return Some(cfg);
    }
    let dir = dir?;
    let pf = ProviderFile::load(dir);
    if let Some(p) = pf.active() {
        return Some(p.config.clone());
    }
    // providers.json 缺失或 active 失效 → 回落旧 model.json（完全兼容旧数据）。
    std::fs::read_to_string(dir.join("model.json"))
        .ok()
        .and_then(|s| ModelProviderConfig::from_json_str(&s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cmx-prov-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn seed_ensures_builtins() {
        let d = tmpdir("seed");
        let pf = ProviderFile::load(&d);
        for (id, _, _) in BUILTIN_PRESETS {
            assert!(pf.get(id).map(|p| p.builtin).unwrap_or(false), "missing {id}");
        }
        assert_eq!(pf.active, None, "无 model.json 时不应有激活条目");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn load_prunes_removed_legacy_builtins() {
        // 旧版文件带着已下线的 builtin-deepseek/openai → 读入时剪除；active 指向被剪条目 → 回落首条。
        let d = tmpdir("prune");
        std::fs::write(
            d.join("providers.json"),
            r#"{"active":"builtin-deepseek","providers":[
                {"id":"builtin-mlamp","name":"MLamp","builtin":true,"base_url":"https://llmgw-bz.mlamp.cn/v1","api_key":"","model":"mlamp/glm-5.2"},
                {"id":"builtin-deepseek","name":"DeepSeek","builtin":true,"base_url":"https://api.deepseek.com/v1","api_key":"","model":"deepseek-chat"},
                {"id":"builtin-openai","name":"OpenAI","builtin":true,"base_url":"https://api.openai.com/v1","api_key":"","model":"gpt-4o"}]}"#,
        )
        .unwrap();
        let pf = ProviderFile::load(&d);
        assert!(pf.get("builtin-deepseek").is_none(), "历史遗留内置应被剪除");
        assert!(pf.get("builtin-openai").is_none(), "历史遗留内置应被剪除");
        assert!(pf.get("builtin-mlamp").is_some(), "现行内置保留");
        assert_eq!(pf.active.as_deref(), Some("builtin-mlamp"), "active 指向被剪条目应回落");
        std::fs::remove_dir_all(&d).ok();
    }

    // 注意：本测试假定运行环境未设 CMX_AGENT_MODEL_* / DEEPSEEK_API_KEY（CI 干净环境成立）。
    #[test]
    fn seed_merges_model_json_into_matching_builtin() {
        let d = tmpdir("merge");
        std::fs::write(
            d.join("model.json"),
            r#"{"base_url":"https://api.deepseek.com","api_key":"sk-abc","model":"deepseek-r1"}"#,
        )
        .unwrap();
        let pf = ProviderFile::load(&d);
        let ds = pf.get("builtin-default").unwrap();
        assert_eq!(ds.config.api_key, "sk-abc");
        assert_eq!(ds.config.model, "deepseek-r1");
        assert_eq!(pf.active.as_deref(), Some("builtin-default"));
        std::fs::remove_dir_all(&d).ok();
    }

    // 注意：同上，假定无 env 配置。
    #[test]
    fn seed_appends_default_for_unknown_base_url() {
        let d = tmpdir("append");
        std::fs::write(
            d.join("model.json"),
            r#"{"base_url":"https://api.moonshot.cn/v1","api_key":"sk-k","model":"kimi-v1"}"#,
        )
        .unwrap();
        let pf = ProviderFile::load(&d);
        assert_eq!(pf.active.as_deref(), Some("builtin-default"));
        let p = pf.active().unwrap();
        assert_eq!(p.name, "默认");
        assert_eq!(p.config.model, "kimi-v1");
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn save_load_roundtrip_preserves_fields() {
        let d = tmpdir("round");
        let mut pf = ProviderFile::load(&d);
        let id = new_id();
        pf.upsert(NamedProvider {
            id: id.clone(),
            name: "我的Kimi".into(),
            builtin: false,
            config: ModelProviderConfig::new("https://api.moonshot.cn/v1", "sk-k", "kimi-v1"),
        });
        pf.active = Some(id.clone());
        pf.save(&d).unwrap();
        let pf2 = ProviderFile::load(&d);
        let p = pf2.get(&id).unwrap();
        assert_eq!(p.name, "我的Kimi");
        assert_eq!(p.config.temperature, 0.2);
        assert_eq!(pf2.active.as_deref(), Some(id.as_str()));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn name_taken_excludes_self() {
        let mut pf = ProviderFile::default();
        pf.upsert(NamedProvider {
            id: "a".into(),
            name: "Kimi".into(),
            builtin: false,
            config: ModelProviderConfig::new("http://x", "", "m"),
        });
        assert!(pf.name_taken("Kimi", None));
        assert!(!pf.name_taken("Kimi", Some("a")));
        assert!(!pf.name_taken("Other", None));
    }

    #[test]
    fn resolve_active_falls_back_to_model_json_when_active_missing() {
        let d = tmpdir("fallback");
        std::fs::write(
            d.join("model.json"),
            r#"{"base_url":"https://api.deepseek.com","api_key":"sk-x","model":"deepseek-chat"}"#,
        )
        .unwrap();
        // 不写 providers.json → 走播种，active 指向合并后的条目；清 active 场景用空 providers.json 模拟。
        std::fs::write(d.join("providers.json"), r#"{"active":null,"providers":[]}"#).unwrap();
        let cfg = resolve_active(Some(&d)).unwrap();
        assert_eq!(cfg.model, "deepseek-chat");
        std::fs::remove_dir_all(&d).ok();
    }
}
