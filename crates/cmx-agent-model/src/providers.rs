//! 命名 provider 多配置（providers.json）。
//!
//! 「配置模型」面板支持多个命名 provider（内置预设不可删 + 用户自定义可增删），
//! 每条 [`NamedProvider`] 在 [`ModelProviderConfig`] 之上加 `id`/`name`/`builtin` 身份字段，
//! 整体持久化为 `<data_dir>/providers.json`（含 `active` 指针）。
//!
//! 兼容：providers.json 不存在时从 model.json / env 播种（懒播种，读路径无副作用）；
//! 解析链 env > providers.json(active) > model.json（env 优先维持既有约定，
//! 设了 `CMX_AGENT_MODEL_*` 的开发环境里面板改动重启后会被 env 盖过）。

use std::collections::HashSet;
use std::path::Path;

use crate::config::ModelProviderConfig;

/// 一条候选模型（`NamedProvider::models` 的元素，方案 20260917 §5.1 + ZCode 式模型编辑弹窗）。
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModelEntry {
    /// 模型 ID（发往网关的名字），如 `mlamp/glm-5.2`。
    pub id: String,
    /// 启用开关（ZCode 同款）：关 = 保留配置但不进聊天选择器。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 思考标记：该模型会吐 reasoning_content（◎ 选择器显示 🧠，预期管理——
    /// 现状只有 glm-5.2 出思考内容，选了不吐思考的模型时有标记说明，不再神秘）。
    /// 由编辑弹窗「推理等级」是否非空自动推导（见 `set_model_config`），旧字段保留兼容。
    /// （2026-09-17 勘误：清单行 🧠 徽标撤除，但弹窗推理功能与推导语义不变。）
    #[serde(default)]
    pub reasoning: bool,
    /// 上下文窗口（token 数；空 = 未设置）。编辑弹窗「上下文窗口」。
    #[serde(default)]
    pub context_window: Option<u64>,
    /// 最大输出 Token（空 = 未设置）。编辑弹窗「最大输出 Token」。
    #[serde(default)]
    pub max_output_tokens: Option<u64>,
    /// 输入类型（编辑弹窗多选）：text / image / video / pdf。空 = 未标注（默认按 text）。
    #[serde(default)]
    pub input_types: Vec<String>,
    /// 模型能力（编辑弹窗多选）：structured_output / web_search / system_message。
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// 推理等级（从低到高），如 ["low","high","max"]。非空 ⇔ reasoning=true。
    #[serde(default)]
    pub reasoning_levels: Vec<String>,
    /// 推理参数映射（JSON 字符串，原样存储；P2 接请求透传）。
    #[serde(default)]
    pub reasoning_params: String,
}

/// 输入类型/模型能力 chips 的合法值域（编辑弹窗白名单，越界保存直接拒）。
pub const MODEL_INPUT_TYPES: &[&str] = &["text", "image", "video", "pdf"];
pub const MODEL_CAPABILITIES: &[&str] = &["structured_output", "web_search", "system_message"];
/// 推理等级的「从低到高」展示序（sort_by_key 用；自定义等级排在其后）。
pub const MODEL_REASONING_ORDER: &[&str] = &["low", "high", "max"];

fn default_true() -> bool {
    true
}

fn default_kind() -> String {
    "openai".to_string()
}

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
    /// 协议类型。P1 仅 `openai`（chat/completions），P2 预留 `anthropic`；
    /// 缺省 openai——旧文件零迁移可用。
    #[serde(default = "default_kind")]
    pub kind: String,
    /// 来源模板 id（[`PROVIDER_PRESETS`] 的 id）；空 = 自定义端点。
    #[serde(default)]
    pub preset: String,
    /// 候选模型清单（聊天选择器 + 配置页清单管理的数据源，替代 base_url 关键词硬编码）。
    #[serde(default)]
    pub models: Vec<ModelEntry>,
    /// 「获取 API Key」指引外链（模板携带）；空 = 不显示。
    #[serde(default)]
    pub api_key_url: String,
    /// 连接配置（base_url / api_key / model / temperature / timeout_ms）。
    #[serde(flatten)]
    pub config: ModelProviderConfig,
}

/// `<dir>/providers.json` 的完整内容。
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderFile {
    /// 当前激活的 provider id；None = 无激活（回退离线 DemoModel）。
    #[serde(default)]
    pub active: Option<String>,
    #[serde(default)]
    pub providers: Vec<NamedProvider>,
    /// 会话超长自动压缩开关（压缩方案 §4.5）：缺省 true；false = 仅手动 /compact 与
    /// 溢出救援生效。本轮不加设置 UI，配置文件即文档。
    #[serde(default = "default_true")]
    pub auto_compact: bool,
}

impl Default for ProviderFile {
    fn default() -> Self {
        Self {
            active: None,
            providers: Vec::new(),
            auto_compact: true,
        }
    }
}

/// 供应商模板内单条模型元数据（ZCode modelConfigRules 的静态子集）。
pub struct PresetModel {
    pub id: &'static str,
    pub reasoning: bool,
    /// 输入类型（MODEL_INPUT_TYPES 子集）：text / image / video / pdf；空 = 未标注。
    pub input_types: &'static [&'static str],
    /// 推理等级（从低到高；种子侧约定：非空 ⇔ reasoning=true）。
    pub reasoning_levels: &'static [&'static str],
}

impl PresetModel {
    /// 只标思考与否的简单条目（多数模板模型够用）。
    pub const fn simple(id: &'static str, reasoning: bool) -> Self {
        Self { id, reasoning, input_types: &[], reasoning_levels: &[] }
    }
}

/// 供应商模板（内置目录，「＋ 新增 Provider」目录卡与内置播种的唯一真源，方案 20260917 §5.2）。
/// P1 随应用版本分发（ZCode 同款做法）；播种条目 id 形态 `builtin-<preset id>`（现行仅 mlamp）。
pub struct ProviderPreset {
    /// 模板 id（写入 `NamedProvider::preset`；`custom` 只作目录卡不产生 preset 值）。
    pub id: &'static str,
    pub name: &'static str,
    /// 目录卡上的一句话说明。
    pub description: &'static str,
    pub base_url: &'static str,
    /// Key 获取指引外链；空 = 无公开页（如内部网关）。
    pub api_key_url: &'static str,
    pub models: &'static [PresetModel],
    /// true = 播种为不可删内置条目（`builtin-<id>`）。现行仅 mlamp。
    pub builtin_seed: bool,
}

/// 模板真源（后端化：前端 `MCFG_PRESETS`/`MCFG_CANDIDATES` 两份硬编码已下线，经
/// `list_provider_presets` 下发）。🧠 = reasoning。
pub const PROVIDER_PRESETS: &[ProviderPreset] = &[
    ProviderPreset {
        id: "mlamp",
        name: "MLamp 网关",
        description: "公司内部 LLM 网关（OpenAI 兼容）——公司同事默认选这个",
        base_url: "https://llmgw-bz.mlamp.cn/v1",
        api_key_url: "",
        models: &[
            PresetModel::simple("mlamp/glm-5.2", true),
            // 2026-09-17 用户提供：这两个模型全模态输入，推理等级默认高
            PresetModel {
                id: "mlamp/glm-5.3-flash",
                reasoning: true,
                input_types: &["text", "image", "video", "pdf"],
                reasoning_levels: &["high"],
            },
            PresetModel::simple("mlamp/deepseek-v4-flash", false),
            PresetModel::simple("mlamp/deepseek-v4-pro", false),
            PresetModel {
                id: "mlamp/kimi-k3",
                reasoning: true,
                input_types: &["text", "image", "video", "pdf"],
                reasoning_levels: &["high"],
            },
            PresetModel::simple("mlamp/qwen3-coder-next-fp8", false),
            PresetModel::simple("mlamp/qwen3.8-27b", false),
            PresetModel::simple("mlamp/minimax-h3", false),
        ],
        builtin_seed: true,
    },
    ProviderPreset {
        id: "bigmodel",
        name: "智谱 BigModel",
        description: "智谱开放平台官方 API",
        base_url: "https://open.bigmodel.cn/api/coding/paas/v4",
        api_key_url: "https://bigmodel.cn/usercenter/proj-mgmt/apikeys",
        models: &[
            PresetModel::simple("glm-5.3", true),
            PresetModel::simple("glm-5.3-flash", true),
            PresetModel::simple("glm-5.2", true),
        ],
        builtin_seed: false,
    },
    ProviderPreset {
        id: "deepseek",
        name: "DeepSeek",
        description: "DeepSeek 开放平台官方 API",
        base_url: "https://api.deepseek.com/v1",
        api_key_url: "https://platform.deepseek.com/api_keys",
        models: &[
            PresetModel::simple("deepseek-v4-pro", true),
            PresetModel::simple("deepseek-v4-flash", false),
            PresetModel::simple("deepseek-r1", true),
            PresetModel::simple("deepseek-chat", false),
        ],
        builtin_seed: false,
    },
    ProviderPreset {
        id: "dashscope",
        name: "阿里云百炼",
        description: "DashScope OpenAI 兼容模式",
        base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        api_key_url: "https://bailian.console.aliyun.com/",
        models: &[
            PresetModel::simple("qwen-max", false),
            PresetModel::simple("qwen-plus", false),
            PresetModel::simple("qwen-turbo", false),
            PresetModel::simple("qwen3-coder-plus", false),
        ],
        builtin_seed: false,
    },
    ProviderPreset {
        id: "moonshot",
        name: "月之暗面 Kimi",
        description: "Kimi 开放平台官方 API",
        base_url: "https://api.moonshot.cn/v1",
        api_key_url: "https://platform.kimi.com/console/apikeys",
        models: &[PresetModel::simple("kimi-k3", true), PresetModel::simple("kimi-latest", false)],
        builtin_seed: false,
    },
    ProviderPreset {
        id: "openai",
        name: "OpenAI",
        description: "OpenAI 官方 API",
        base_url: "https://api.openai.com/v1",
        api_key_url: "https://platform.openai.com/api-keys",
        models: &[
            PresetModel::simple("gpt-4o", false),
            PresetModel::simple("gpt-4o-mini", false),
            PresetModel::simple("o1", true),
            PresetModel::simple("o1-mini", true),
        ],
        builtin_seed: false,
    },
    ProviderPreset {
        id: "custom",
        name: "自定义端点",
        description: "任意 OpenAI 兼容端点（vLLM / Ollama / 公司网关）",
        base_url: "",
        api_key_url: "",
        models: &[],
        builtin_seed: false,
    },
];

/// 按模板 id 查模板（含 `custom`）。
pub fn find_preset(id: &str) -> Option<&'static ProviderPreset> {
    PROVIDER_PRESETS.iter().find(|p| p.id == id)
}

/// 模板目录的前门 JSON（`list_provider_presets` 命令下发；前端目录卡数据源）。
pub fn provider_presets_json() -> serde_json::Value {
    serde_json::Value::Array(
        PROVIDER_PRESETS
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id,
                    "name": p.name,
                    "description": p.description,
                    "base_url": p.base_url,
                    "api_key_url": p.api_key_url,
                    "builtin_seed": p.builtin_seed,
                    "models": p.models.iter().map(|m| serde_json::json!({
                        "id": m.id, "reasoning": m.reasoning,
                        "input_types": m.input_types, "reasoning_levels": m.reasoning_levels,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

impl NamedProvider {
    /// 读入/写入前的归一化（旧文件零迁移可用的关键）：kind 缺省 openai；
    /// models 空且当前模型非空 → 以当前模型播种单条启用项（旧数据首次打开设置看到的
    /// 就是 models=[当前模型]，不算回归，方案 §8）；按 id 去重保序、剪空 id。
    pub fn normalize(&mut self) {
        if self.kind.is_empty() {
            self.kind = "openai".to_string();
        }
        if self.models.is_empty() && !self.config.model.is_empty() {
            self.models.push(ModelEntry { id: self.config.model.clone(), enabled: true, ..Default::default() });
        }
        let mut seen: HashSet<String> = HashSet::new();
        self.models.retain(|m| !m.id.is_empty() && seen.insert(m.id.clone()));
    }

    /// 启用中的候选模型 id（聊天选择器数据源）。
    pub fn enabled_models(&self) -> Vec<String> {
        self.models.iter().filter(|m| m.enabled).map(|m| m.id.clone()).collect()
    }
}

/// 自定义条目 id（纳秒时间戳，无 uuid 依赖）。
pub fn new_id() -> String {
    format!("p-{}", std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0))
}

/// base_url 归一化：去 scheme/尾斜杠、小写。匹配规则：互为前缀即视为同一 provider
/// （旧 model.json 常写 `https://api.deepseek.com`，预设是 `…/v1`）。
fn normalize_base(u: &str) -> String {
    u.trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_ascii_lowercase()
}

/// 网关同源判定：normalize_base 后再去掉 `/v1` 尾段做**整段相等**。
/// 旧实现前缀互含（starts_with 双向）——相似域名条目（llmgw-bz.mlamp.cn.evil.io）
/// 会命中真网关并把 api_key 复制过去；同主机带不带 /v1 的写法仍视为同一网关。
fn same_gateway(a: &str, b: &str) -> bool {
    fn canon(s: &str) -> &str {
        s.trim_end_matches('/').trim_end_matches("/v1")
    }
    !a.is_empty() && !b.is_empty() && canon(a) == canon(b)
}

/// 旧内置条目元数据回填（ensure_builtins 专用）：preset 空 = 升级前的旧条目，
/// 按网关匹配补 preset/api_key_url，并把单模型清单扩成模板完整清单（当前模型保证在列）。
fn backfill_builtin_meta(p: &mut NamedProvider, preset: &ProviderPreset) {
    if !p.preset.is_empty() || !same_gateway(&p.config.base_url, preset.base_url) {
        return;
    }
    p.preset = preset.id.to_string();
    if p.api_key_url.is_empty() {
        p.api_key_url = preset.api_key_url.to_string();
    }
    if p.models.len() <= 1 && !preset.models.is_empty() {
        let mut models: Vec<ModelEntry> = preset
            .models
            .iter()
            .map(|m| ModelEntry {
                id: m.id.to_string(),
                enabled: true,
                reasoning: m.reasoning,
                input_types: m.input_types.iter().map(|s| s.to_string()).collect(),
                reasoning_levels: m.reasoning_levels.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            })
            .collect();
        let cur = &p.config.model;
        if !cur.is_empty() && !models.iter().any(|m| &m.id == cur) {
            models.insert(0, ModelEntry { id: cur.clone(), enabled: true, ..Default::default() });
        }
        p.models = models;
    }
}

impl ProviderFile {
    /// 读 `<dir>/providers.json`；不存在/解析失败则**播种**（不落盘——写发生在首次 save）：
    /// 内置预设常驻；model.json 的 key/model/base_url 合入命中 base_url 的内置条目
    /// （未命中则作为「默认」内置条目追加）；model.json 未配置则尝试 env 同样合并；
    /// 都没有 → 空表（demo 兜底）。
    ///
    /// **已存在的文件同样合并 legacy（model.json/env）凭据**：内置条目 key 恒空（不入仓库），
    /// 若只在播种时合并，面板热切模型会拿无 key 配置调网关（网关报「未提供令牌」）。
    /// 读已有文件时 `force=false`：只补空缺，不覆盖用户在面板里显式填过的 key/model。
    pub fn load(dir: &Path) -> Self {
        Self::load_with_inherit(dir, None)
    }

    /// [`Self::load`] 的继承变体：`inherit_from`（如登录态 per-user 目录对应的全局配置目录）
    /// 里存在 providers.json 时，把**同网关**（base_url 归一化匹配）条目的 key 补给本表空 key 条目。
    /// 场景：用户在共享域填过 key，登录后 effective 目录切到 per-user、其条目 key 为空，
    /// 不继承则面板热切模型会拿空凭据调网关（网关报「未提供令牌」）。只补空缺，不覆盖显式配置。
    pub fn load_with_inherit(dir: &Path, inherit_from: Option<&Path>) -> Self {
        if let Ok(s) = std::fs::read_to_string(dir.join("providers.json"))
            && let Ok(mut pf) = serde_json::from_str::<ProviderFile>(&s)
        {
            pf.prune_removed_builtins();
            if let Some(legacy) = Self::read_legacy(dir) {
                pf.merge_legacy(legacy, false);
            }
            if let Some(base) = inherit_from {
                pf.inherit_keys_from(base);
            }
            return Self::ensure_builtins(pf);
        }
        Self::seed(dir)
    }

    /// 从基准目录的 providers.json 同网关条目**只补空 key**（model 等其余配置不动、不落盘）。
    /// `inherit_from`（如登录态 per-user 目录对应的全局配置目录）里存在 providers.json 时，
    /// 把**同网关**（base_url 归一化匹配）条目的 key 补给本表空 key 条目。
    /// 场景：用户在共享域填过 key，登录后 effective 目录切到 per-user、其条目 key 为空，
    /// 不继承则面板热切模型会拿空凭据调网关（网关报「未提供令牌」）。只补空缺，不覆盖显式配置。
    fn inherit_keys_from(&mut self, base_dir: &Path) {
        let Ok(s) = std::fs::read_to_string(base_dir.join("providers.json")) else {
            return;
        };
        let Ok(base_pf) = serde_json::from_str::<ProviderFile>(&s) else {
            return;
        };
        for p in self.providers.iter_mut().filter(|p| p.config.api_key.is_empty()) {
            let b = normalize_base(&p.config.base_url);
            if b.is_empty() {
                continue;
            }
            if let Some(src) = base_pf.providers.iter().find(|x| {
                normalize_base(&x.config.base_url) == b && !x.config.api_key.is_empty()
            }) {
                p.config.api_key = src.config.api_key.clone();
            }
        }
    }

    /// 旧配置读取：model.json 优先，其次 env（哪个有值用哪个）。
    fn read_legacy(dir: &Path) -> Option<ModelProviderConfig> {
        std::fs::read_to_string(dir.join("model.json"))
            .ok()
            .and_then(|s| ModelProviderConfig::from_json_str(&s))
            .or_else(ModelProviderConfig::from_env)
    }

    /// 把 legacy 配置合并进命中 base_url 的条目。返回命中/新增条目 id（未命中且不追加时为 None）。
    /// `force=true`（播种）：key/model 直接覆盖，未命中追加「默认」条目；
    /// `force=false`（读已有文件）：只补空缺，不覆盖用户显式配置，未命中不追加。
    fn merge_legacy(&mut self, cfg: ModelProviderConfig, force: bool) -> Option<String> {
        let key = normalize_base(&cfg.base_url);
        let hit = self.providers.iter_mut().find(|p| {
            let b = normalize_base(&p.config.base_url);
            same_gateway(&key, &b)
        });
        match hit {
            Some(p) => {
                if force || p.config.api_key.is_empty() {
                    p.config.api_key = cfg.api_key.clone();
                }
                if force || p.config.model.is_empty() {
                    p.config.model = cfg.model.clone();
                }
                Some(p.id.clone())
            }
            None if force => {
                let id = "builtin-default".to_string();
                self.providers.push(NamedProvider {
                    id: id.clone(),
                    name: "默认".into(),
                    builtin: true,
                    kind: "openai".into(),
                    preset: String::new(),
                    models: Vec::new(),
                    api_key_url: String::new(),
                    config: cfg,
                });
                Some(id)
            }
            None => None,
        }
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

    /// 播种：内置预设打底 + model.json / env 合并（合并命中条目后设为激活）。
    fn seed(dir: &Path) -> Self {
        let mut pf = Self::ensure_builtins(Self::default());
        if let Some(cfg) = Self::read_legacy(dir)
            && let Some(id) = pf.merge_legacy(cfg, true)
        {
            pf.active = Some(id);
        }
        pf
    }

    /// 补齐内置预设（已存在则不动；手工删过 providers.json 也能恢复）。
    /// 顺带回填旧内置条目的模板元数据（preset/api_key_url/完整模型清单）——升级后老用户的
    /// 单模型内置条目自动获得模板自带清单（当前模型保持原位不动）。
    fn ensure_builtins(mut self) -> Self {
        for preset in PROVIDER_PRESETS.iter().filter(|p| p.builtin_seed) {
            let id = format!("builtin-{}", preset.id);
            match self.providers.iter_mut().find(|p| p.id == id) {
                Some(p) => backfill_builtin_meta(p, preset),
                None => {
                    let models: Vec<ModelEntry> = preset
                        .models
                        .iter()
                        .map(|m| ModelEntry {
                id: m.id.to_string(),
                enabled: true,
                reasoning: m.reasoning,
                input_types: m.input_types.iter().map(|s| s.to_string()).collect(),
                reasoning_levels: m.reasoning_levels.iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            })
                        .collect();
                    let model = models.first().map(|m| m.id.clone()).unwrap_or_default();
                    self.providers.push(NamedProvider {
                        id,
                        name: preset.name.to_string(),
                        builtin: true,
                        kind: "openai".to_string(),
                        preset: preset.id.to_string(),
                        models,
                        api_key_url: preset.api_key_url.to_string(),
                        config: ModelProviderConfig {
                            base_url: preset.base_url.to_string(),
                            api_key: String::new(),
                            model,
                            temperature: 0.2,
                            timeout_ms: 60_000,
                        },
                    });
                }
            }
        }
        for p in self.providers.iter_mut() {
            p.normalize();
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

    /// 按 id 插入或替换；返回是否为新插入。写入前归一化（kind 缺省 / models 播种 / 去重）。
    pub fn upsert(&mut self, mut p: NamedProvider) -> bool {
        p.normalize();
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
        for preset in PROVIDER_PRESETS.iter().filter(|p| p.builtin_seed) {
            let id = format!("builtin-{}", preset.id);
            let p = pf.get(&id).unwrap_or_else(|| panic!("missing {id}"));
            assert!(p.builtin);
            assert_eq!(p.preset, preset.id, "播种条目应带模板 id");
            assert_eq!(p.models.len(), preset.models.len(), "播种条目应带模板完整清单");
        }
        assert_eq!(pf.active, None, "无 model.json 时不应有激活条目");
        std::fs::remove_dir_all(&d).ok();
    }

    /// 旧版文件（无 kind/preset/models 字段）读入：normalize 补默认——kind=openai、
    /// models=[当前模型]；内置条目再由 ensure_builtins 回填模板元数据与完整清单。
    #[test]
    fn legacy_file_without_models_is_normalized_and_builtin_backfilled() {
        let d = tmpdir("legacy");
        std::fs::write(
            d.join("providers.json"),
            r#"{"active":"builtin-mlamp","providers":[
                {"id":"builtin-mlamp","name":"MLamp","builtin":true,"base_url":"https://llmgw-bz.mlamp.cn/v1","api_key":"sk-x","model":"mlamp/glm-5.2"},
                {"id":"p-1","name":"自定义","base_url":"https://api.moonshot.cn/v1","api_key":"","model":"kimi-k3"}]}"#,
        )
        .unwrap();
        let pf = ProviderFile::load(&d);
        let m = pf.get("builtin-mlamp").unwrap();
        assert_eq!(m.kind, "openai");
        assert_eq!(m.preset, "mlamp", "内置条目应回填模板 id");
        assert!(m.models.iter().any(|e| e.id == "mlamp/glm-5.2"), "当前模型应在清单中");
        assert!(m.models.len() > 1, "应扩成模板完整清单");
        assert!(m.models.iter().filter(|e| e.id == "mlamp/glm-5.2").all(|e| e.enabled));
        let c = pf.get("p-1").unwrap();
        assert_eq!(c.kind, "openai");
        assert_eq!(c.models, vec![ModelEntry { id: "kimi-k3".into(), enabled: true, ..Default::default() }]);
        std::fs::remove_dir_all(&d).ok();
    }

    /// models 去重保序；模型 enabled 缺省 true。
    #[test]
    fn models_dedupe_and_defaults() {
        let d = tmpdir("dedupe");
        std::fs::write(
            d.join("providers.json"),
            r#"{"active":null,"providers":[
                {"id":"p-1","name":"x","base_url":"https://a/v1","api_key":"","model":"m1",
                 "models":[{"id":"m1"},{"id":"m1","enabled":false},{"id":"m2","enabled":false,"reasoning":true}]}]}"#,
        )
        .unwrap();
        let pf = ProviderFile::load(&d);
        let p = pf.get("p-1").unwrap();
        assert_eq!(p.models.len(), 2, "重复 id 应去重");
        assert_eq!(p.models[0].id, "m1");
        assert!(p.models[0].enabled, "enabled 缺省 true");
        assert!(p.models[1].reasoning);
        assert_eq!(p.enabled_models(), vec!["m1".to_string()]);
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

    // 注意：本测试假定运行环境未设 CMX_AGENT_MODEL_* / DEEPSEEK_API_KEY。
    #[test]
    fn load_merges_legacy_key_into_existing_providers_file() {
        // 已有 providers.json（内置条目 key 空）+ model.json 同网关凭据：
        // 读路径也必须合并（base_url 归一化后互为前缀即命中），否则面板热切模型无 key 可用。
        let d = tmpdir("merge-existing");
        std::fs::write(
            d.join("providers.json"),
            r#"{"active":"builtin-mlamp","providers":[
                {"id":"builtin-mlamp","name":"MLamp","builtin":true,"base_url":"https://llmgw-bz.mlamp.cn/v1","api_key":"","model":"mlamp/glm-5.2"}]}"#,
        )
        .unwrap();
        // model.json 不带 /v1（覆盖归一化匹配），model 名故意不同 → force=false 不应覆盖条目模型。
        std::fs::write(
            d.join("model.json"),
            r#"{"base_url":"https://llmgw-bz.mlamp.cn","api_key":"sk-legacy","model":"mlamp/deepseek-v4-flash"}"#,
        )
        .unwrap();
        let pf = ProviderFile::load(&d);
        let m = pf.get("builtin-mlamp").unwrap();
        assert_eq!(m.config.api_key, "sk-legacy", "空 key 应由 legacy 补上");
        assert_eq!(
            m.config.model, "mlamp/glm-5.2",
            "非播种路径不得覆盖用户已选模型"
        );
        std::fs::remove_dir_all(&d).ok();
    }

    // 注意：同上，假定无 env 配置。
    #[test]
    fn load_never_overwrites_explicit_user_key() {
        let d = tmpdir("merge-keep");
        std::fs::write(
            d.join("providers.json"),
            r#"{"active":"builtin-mlamp","providers":[
                {"id":"builtin-mlamp","name":"MLamp","builtin":true,"base_url":"https://llmgw-bz.mlamp.cn/v1","api_key":"sk-user","model":"mlamp/glm-5.2"}]}"#,
        )
        .unwrap();
        std::fs::write(
            d.join("model.json"),
            r#"{"base_url":"https://llmgw-bz.mlamp.cn/v1","api_key":"sk-legacy","model":"mlamp/deepseek-v4-flash"}"#,
        )
        .unwrap();
        let pf = ProviderFile::load(&d);
        assert_eq!(
            pf.get("builtin-mlamp").unwrap().config.api_key,
            "sk-user",
            "用户显式配置的 key 优先于 legacy"
        );
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
            kind: "openai".into(),
            preset: "moonshot".into(),
            models: vec![
                ModelEntry { id: "kimi-k3".into(), enabled: true, reasoning: true, ..Default::default() },
                ModelEntry { id: "kimi-latest".into(), enabled: false, ..Default::default() },
            ],
            api_key_url: "https://platform.kimi.com/console/apikeys".into(),
            config: ModelProviderConfig::new("https://api.moonshot.cn/v1", "sk-k", "kimi-v1"),
        });
        pf.active = Some(id.clone());
        pf.save(&d).unwrap();
        let pf2 = ProviderFile::load(&d);
        let p = pf2.get(&id).unwrap();
        assert_eq!(p.name, "我的Kimi");
        assert_eq!(p.config.temperature, 0.2);
        assert_eq!(p.preset, "moonshot");
        assert_eq!(p.api_key_url, "https://platform.kimi.com/console/apikeys");
        assert_eq!(p.models.len(), 2, "用户清单应原样保留（不被播种/回填）");
        assert!(!p.models[1].enabled);
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
            kind: "openai".into(),
            preset: String::new(),
            models: Vec::new(),
            api_key_url: String::new(),
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
