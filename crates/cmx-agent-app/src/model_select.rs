//! 模型选择（E0）：按环境变量 / 配置文件决定用**真实大模型**还是**离线演示模型**。
//!
//! 解析优先级：环境变量（`CMX_AGENT_MODEL_*` / `CMX_AI_*` / `DEEPSEEK_API_KEY`）> `<data_dir>/model.json`
//! 持久化配置 > 回退 [`DemoModel`]（关键词路由，无网络）。壳与前端零改动即可切换。

use std::path::Path;
use std::sync::Arc;

use cmx_agent_core::ModelSeam;

/// 装配当前应使用的模型缝。`config_dir` = 桌面壳数据目录（含 `providers.json` / `model.json`）；`None` 则仅看 env。
///
/// 解析链：env > providers.json(active) > model.json 兜底（见 `cmx_agent_model::resolve_active`）。
pub fn select_model(config_dir: Option<&Path>) -> Arc<dyn ModelSeam> {
    match cmx_agent_model::resolve_active(config_dir) {
        Some(cfg) => {
            tracing::info!(
                "cmx-agent 模型：OpenAI 兼容 provider · model={} · base={}",
                cfg.model,
                cfg.base_url
            );
            Arc::new(cmx_agent_model::OpenAiCompatModel::new(cfg))
        }
        None => {
            tracing::info!("cmx-agent 模型：DemoModel（未配置真实模型，回退离线演示）");
            Arc::new(crate::DemoModel)
        }
    }
}
