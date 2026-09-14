//! ToolRegistry 热注册/热卸载 + 共享句柄语义（U15 插件热加载底座）。
//!
//! 验证 `#[derive(Clone)]` 得到的是**同一张表**的句柄：一个句柄 `register_dyn`/`unregister`，
//! 另一个句柄 `get`/`specs` 立即可见。这是「装/卸插件不重启即生效」的核心。

use std::sync::Arc;

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolRegistry, ToolResult, ToolSpec};
use serde_json::{Value, json};

struct NamedTool(&'static str);
#[async_trait]
impl Tool for NamedTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(self.0, "测试工具").guard(GuardHints { idempotent: true, ..Default::default() })
    }
    async fn invoke(&self, _i: Value, _c: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        Ok(ToolResult::ok(json!({ "name": self.0 })))
    }
}

#[test]
fn clone_shares_same_table_hot_add_remove() {
    let mut base = ToolRegistry::new();
    base.register(Arc::new(NamedTool("builtin")));
    // 克隆得到同表句柄（模拟 Agent 持一份、App 经 agent.tools().clone() 持一份）
    let handle = base.clone();
    assert!(base.get("builtin").is_some());
    assert!(handle.get("builtin").is_some());

    // 经 handle 热注册（&self）→ base 立即可见（同一张表）
    handle.register_dyn(Arc::new(NamedTool("plugin_a"))).expect("首次热注册应成功");
    assert!(base.get("plugin_a").is_some(), "热注册应对另一句柄立即可见");
    assert!(base.specs().iter().any(|s| s.name == "plugin_a"));

    // 重名拒绝（N3：register_dyn 不得同名遮蔽）
    assert!(handle.register_dyn(Arc::new(NamedTool("plugin_a"))).is_err());

    // 经 base 热卸载 → handle 立即看不到
    assert!(base.unregister("plugin_a"));
    assert!(handle.get("plugin_a").is_none(), "热卸载应对另一句柄立即可见");
    // 卸载不存在的返回 false
    assert!(!base.unregister("nope"));
    // builtin 不受影响
    assert!(handle.get("builtin").is_some());
}
