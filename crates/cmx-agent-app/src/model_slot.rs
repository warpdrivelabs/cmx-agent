//! 可热换的模型缝 [`ModelSlot`]（B2 模型选择器底座）。
//!
//! 与 [`cmx_agent_core::ToolRegistry`] 的热注册同构：`Agent` 持本 wrapper（身份不变），
//! `AgentApp` 经 `.slot` 换里面的实现——`run_turn` 每回合读当前实现，切换即下回合生效，无需重建 Agent。

use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use cmx_agent_core::model::{ModelContext, ModelError, ModelResponse, ModelSeam, TurnObserver};

/// 共享可换的模型槽。`Clone` 得到的是**同一实现指针的句柄**。
#[derive(Clone)]
pub struct ModelSlot {
    inner: Arc<RwLock<Arc<dyn ModelSeam>>>,
}

impl ModelSlot {
    pub fn new(model: Arc<dyn ModelSeam>) -> Self {
        Self { inner: Arc::new(RwLock::new(model)) }
    }

    /// 热换当前模型实现（下一回合生效）。
    pub fn swap(&self, model: Arc<dyn ModelSeam>) {
        if let Ok(mut g) = self.inner.write() {
            *g = model;
        }
    }

    fn current(&self) -> Arc<dyn ModelSeam> {
        self.inner.read().expect("model slot").clone()
    }
}

#[async_trait]
impl ModelSeam for ModelSlot {
    async fn complete(&self, ctx: &ModelContext) -> Result<ModelResponse, ModelError> {
        self.current().complete(ctx).await
    }

    // 委托流式版（保留真实模型的 token 流；不走 trait 默认的「整段当一个 delta」）。
    async fn complete_streaming(
        &self,
        ctx: &ModelContext,
        observer: &dyn TurnObserver,
    ) -> Result<ModelResponse, ModelError> {
        self.current().complete_streaming(ctx, observer).await
    }
}
