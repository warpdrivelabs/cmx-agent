//! 子智能体注册表（方案 20260914 §6.1，阶段一）+ 专属模型解析（§6.5）。
//!
//! - [`AgentRegistry`]：`<data_dir>/agents.json` 读写（自定义 + 内置覆盖项）。内置两条编译进
//!   core（`builtin_specs()`），文件只存 custom 与 builtin 的 model/enabled 覆盖；文件损坏降级
//!   为只用内置（对齐 `read_meta` 降级先例），不炸启动。合并后的生效清单放在
//!   `Arc<RwLock<Vec<AgentSpec>>>` 与 task 工具共享——内核每步 `specs()` 现调 `TaskTool::spec()`
//!   动态拼枚举，故保存/删除即热生效，零额外同步路径。
//! - [`AppModelResolver`]：core [`ModelResolver`] 缝的 app 实现。`None` → 父当前模型槽
//!   （`ModelSlot` 本体实现 `ModelSeam`，clone 共享内部指针，热切自动跟随）；`Some(id)` →
//!   在 `providers_lock` 下走 `load_providers()`（内含 per-user 目录回落与同网关 key 继承），
//!   按 provider id 查 [`cmx_agent_model::NamedProvider`] 构建专属模型。失败显式报错不静默降级。

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock, Weak};

use cmx_agent_core::agents::{builtin_specs, AgentSpec, ModelResolver};
use cmx_agent_core::model::ModelSeam;
use serde::{Deserialize, Serialize};

use crate::app::AgentApp;
use crate::error::{AppError, AppResult};
use crate::store::write_atomic;

/// agents.json 落盘形态：自定义清单 + 内置覆盖项（仅 model/enabled 两键）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct AgentsFile {
    #[serde(default)]
    custom: Vec<AgentSpec>,
    #[serde(default)]
    builtin_overrides: HashMap<String, BuiltinOverride>,
}

/// 内置类型的可改覆盖项（内置不可删，工具集/提示词编译进代码不可改）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct BuiltinOverride {
    #[serde(default)]
    model: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
}

fn default_true() -> bool {
    true
}

pub struct AgentRegistry {
    path: PathBuf,
    /// 合并后的生效清单（内置+覆盖+自定义），与 task 工具共享；变更即热生效。
    specs: Arc<RwLock<Vec<AgentSpec>>>,
    custom: RwLock<Vec<AgentSpec>>,
    overrides: RwLock<HashMap<String, BuiltinOverride>>,
}

impl AgentRegistry {
    /// 加载（或初始化）注册表。文件损坏 → warn + 只用内置两条，不炸启动。
    pub fn load_or_init(data_dir: &std::path::Path) -> Self {
        let path = data_dir.join("agents.json");
        let (custom, overrides) = match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<AgentsFile>(&raw) {
                Ok(f) => (f.custom, f.builtin_overrides),
                Err(e) => {
                    eprintln!("[agents] agents.json 解析失败，降级只用内置两条（{}）：{e}", path.display());
                    (Vec::new(), HashMap::new())
                }
            },
            Err(_) => (Vec::new(), HashMap::new()), // 不存在 = 首次，静默
        };
        let reg = Self {
            path,
            specs: Arc::new(RwLock::new(Vec::new())),
            custom: RwLock::new(custom),
            overrides: RwLock::new(overrides),
        };
        reg.rebuild();
        reg
    }

    /// 与 task 工具共享的生效清单句柄。
    pub fn shared(&self) -> Arc<RwLock<Vec<AgentSpec>>> {
        self.specs.clone()
    }

    /// 生效清单快照（内置在前）。
    pub fn list(&self) -> Vec<AgentSpec> {
        self.specs
            .read()
            .expect("agents specs lock")
            .clone()
    }

    /// 当前自定义清单快照。
    pub fn customs(&self) -> Vec<AgentSpec> {
        self.custom.read().expect("agents custom lock").clone()
    }

    fn rebuild(&self) {
        let overrides = self.overrides.read().expect("agents overrides lock").clone();
        let mut merged: Vec<AgentSpec> = builtin_specs()
            .into_iter()
            .map(|mut b| {
                if let Some(o) = overrides.get(&b.name) {
                    b.model = o.model.clone();
                    b.enabled = o.enabled;
                }
                b
            })
            .collect();
        merged.extend(self.custom.read().expect("agents custom lock").clone());
        *self.specs.write().expect("agents specs lock") = merged;
    }

    fn persist(&self, custom: &[AgentSpec], overrides: &HashMap<String, BuiltinOverride>) -> AppResult<()> {
        let file = AgentsFile {
            custom: custom.to_vec(),
            builtin_overrides: overrides.clone(),
        };
        let json = serde_json::to_string_pretty(&file)?;
        write_atomic(&self.path, json.as_bytes())?;
        Ok(())
    }

    /// upsert 一个自定义类型（name 冲突内置 → 拒绝；name 唯一）。保存后热生效。
    pub fn upsert_custom(&self, spec: AgentSpec) -> AppResult<()> {
        let name = spec.name.trim().to_string();
        if !valid_name(&name) {
            return Err(AppError::BadRequest(format!(
                "名称「{name}」无效：须 snake_case（小写字母开头，仅小写字母/数字/下划线）"
            )));
        }
        if builtin_specs().iter().any(|b| b.name == name) {
            return Err(AppError::BadRequest(format!(
                "名称「{name}」与内置子智能体冲突，换一个"
            )));
        }
        if spec.title.trim().is_empty() {
            return Err(AppError::BadRequest("显示名不能为空".into()));
        }
        let mut custom = self.custom.write().expect("agents custom lock");
        match custom.iter_mut().find(|a| a.name == name) {
            Some(existing) => *existing = spec,
            None => custom.push(spec),
        }
        let snapshot = custom.clone();
        drop(custom);
        let overrides = self.overrides.read().expect("agents overrides lock").clone();
        self.persist(&snapshot, &overrides)?;
        self.rebuild();
        Ok(())
    }

    /// 内置类型覆盖项（仅 model/enabled）。保存后热生效。
    pub fn set_builtin_override(&self, name: &str, model: Option<String>, enabled: bool) -> AppResult<()> {
        if !builtin_specs().iter().any(|b| b.name == name) {
            return Err(AppError::NotFound(format!("内置子智能体 '{name}'")));
        }
        let mut overrides = self.overrides.write().expect("agents overrides lock");
        overrides.insert(
            name.to_string(),
            BuiltinOverride { model, enabled },
        );
        let snapshot = overrides.clone();
        drop(overrides);
        let custom = self.custom.read().expect("agents custom lock").clone();
        self.persist(&custom, &snapshot)?;
        self.rebuild();
        Ok(())
    }

    /// 删除一个自定义类型（内置/未知 → 拒绝）。删除后热生效。
    pub fn delete_custom(&self, name: &str) -> AppResult<()> {
        if builtin_specs().iter().any(|b| b.name == name) {
            return Err(AppError::BadRequest("内置子智能体不可删除".into()));
        }
        let mut custom = self.custom.write().expect("agents custom lock");
        let before = custom.len();
        custom.retain(|a| a.name != name);
        if custom.len() == before {
            return Err(AppError::NotFound(format!("子智能体 '{name}'")));
        }
        let snapshot = custom.clone();
        drop(custom);
        let overrides = self.overrides.read().expect("agents overrides lock").clone();
        self.persist(&snapshot, &overrides)?;
        self.rebuild();
        Ok(())
    }
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().next().is_some_and(|c| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

/// core [`ModelResolver`] 的 app 实现（§6.5）。
///
/// 构造在 builder 期（持 [`crate::ModelSlot`] clone，`None` 解析不依赖 app 实例——CLI/e2e
/// 无头装配也能用 general-purpose/explore 的默认模型）；`Some(id)` 解析需要完整 app
/// （providers_lock / per-user 目录），由 `AgentApp::into_shared` 注入 Weak 后可用。
pub struct AppModelResolver {
    slot: crate::ModelSlot,
    app: OnceLock<Weak<AgentApp>>,
}

impl AppModelResolver {
    pub fn new(slot: crate::ModelSlot) -> Self {
        Self {
            slot,
            app: OnceLock::new(),
        }
    }

    /// AgentApp 构建完成后注入弱引用（`into_shared` 调用；重复 set 静默忽略）。
    pub fn attach(&self, app: &Arc<AgentApp>) {
        let _ = self.app.set(Arc::downgrade(app));
    }
}

impl ModelResolver for AppModelResolver {
    fn resolve(&self, provider: Option<&str>) -> Result<Arc<dyn ModelSeam>, String> {
        match provider {
            // None = 继承默认：返回模型槽本体（ModelSlot 实现 ModelSeam 且热切自动跟随）。
            None => Ok(Arc::new(self.slot.clone())),
            Some(id) => {
                let app = self
                    .app
                    .get()
                    .and_then(Weak::upgrade)
                    .ok_or_else(|| "专属模型解析需完整装配（当前环境不可用）".to_string())?;
                app.resolve_named_provider(id)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cmx-agents-test-{}-{}",
            tag,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|x| x.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn load_defaults_when_missing() {
        let dir = tmp_dir("missing");
        let reg = AgentRegistry::load_or_init(&dir);
        let list = reg.list();
        assert_eq!(list.len(), 2, "无文件时只用内置两条");
        assert!(list.iter().all(|a| a.builtin));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_falls_back_to_builtins() {
        let dir = tmp_dir("corrupt");
        std::fs::write(dir.join("agents.json"), "{ not json").unwrap();
        let reg = AgentRegistry::load_or_init(&dir);
        assert_eq!(reg.list().len(), 2, "损坏文件降级内置，不炸启动");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn custom_upsert_delete_and_hot_effect() {
        let dir = tmp_dir("crud");
        let reg = AgentRegistry::load_or_init(&dir);
        let spec = AgentSpec {
            name: "code_reviewer".into(),
            title: "代码评审员".into(),
            description: "只读审查".into(),
            tools: cmx_agent_core::agents::ToolSelection::Allow {
                names: vec!["fs_read".into(), "grep".into()],
            },
            model: None,
            system_prompt: "你是严格的代码评审员。".into(),
            enabled: true,
            builtin: false,
        };
        reg.upsert_custom(spec.clone()).expect("upsert");
        assert_eq!(reg.list().len(), 3, "保存即热生效（合并清单 +1）");
        // 重名 upsert = 更新
        let mut spec2 = spec.clone();
        spec2.title = "评审员 2".into();
        reg.upsert_custom(spec2).unwrap();
        assert_eq!(reg.list().len(), 3);
        assert_eq!(reg.customs()[0].title, "评审员 2");
        // 落盘回读
        let raw = std::fs::read_to_string(dir.join("agents.json")).unwrap();
        assert!(raw.contains("code_reviewer"));
        // 内置覆盖
        reg.set_builtin_override("explore", Some("builtin-mlamp".into()), false)
            .unwrap();
        let list = reg.list();
        let explore = list.iter().find(|a| a.name == "explore").unwrap();
        assert!(!explore.enabled && explore.model.as_deref() == Some("builtin-mlamp"));
        // 删除
        reg.delete_custom("code_reviewer").unwrap();
        assert_eq!(reg.list().len(), 2);
        assert!(reg.delete_custom("code_reviewer").is_err(), "未知删除报 NotFound");
        assert!(reg.delete_custom("explore").is_err(), "内置不可删");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn name_validation() {
        let dir = tmp_dir("validate");
        let reg = AgentRegistry::load_or_init(&dir);
        let mk = |name: &str| AgentSpec {
            name: name.into(),
            title: "t".into(),
            description: String::new(),
            tools: Default::default(),
            model: None,
            system_prompt: String::new(),
            enabled: true,
            builtin: false,
        };
        assert!(reg.upsert_custom(mk("Bad-Name")).is_err());
        assert!(reg.upsert_custom(mk("1abc")).is_err());
        assert!(reg.upsert_custom(mk("general-purpose")).is_err(), "撞内置名拒绝");
        assert!(reg.upsert_custom(mk("ok_name2")).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
