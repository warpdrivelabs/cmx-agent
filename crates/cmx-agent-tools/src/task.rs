//! `task` —— 子智能体编排（U1）。把一个独立子任务 fan-out 给**隔离上下文**的子智能体：
//! 用一个全新 [`Session`] 跑一整个回合（可多步、可用全部工具），完成后把最终结果回灌父回合。
//!
//! 接缝：子智能体复用**同一个** [`Agent`]（同模型/工具/守卫/策略/工作区），只是换一个隔离的会话——
//! 这正是「子代理 = 独立上下文」。为避免父↔子的 `Arc` 循环，本工具持 [`Weak<Agent>`]（构建后由
//! `DesktopAppBuilder` 注入）。深度计数器防失控递归（子智能体仍可用 task，但受 max_depth 限制）。

use std::sync::{Arc, OnceLock, Weak};

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Agent, Session, Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

/// 子智能体默认系统提示：专注、独立、简洁收尾。
const SUBAGENT_SYSTEM: &str = "你是一个子智能体，专注完成被交办的**单一子任务**。\
 可用工具就用工具，独立把任务做完（不要反问父级）。完成后用简洁中文给出**最终结果**，结论在前。";

tokio::task_local! {
    /// 当前子智能体的**递归嵌套层级**（父回合视为 0，其直接子智能体为 1，依此类推）。
    ///
    /// 关键：深度随调用链**下传**（task-local，`.scope()` 只在轮询自身 future 期间置位），
    /// 而**不是**共享计数器——故一步里的多个 `task` 被 `join_all` 并发执行时，兄弟子智能体各自
    /// 读到相同的父层级、独立 +1，绝不会互相把闸门顶满。这正是「真并行 fan-out」不误伤深度限制的根因。
    static SUBAGENT_DEPTH: usize;
}

/// 父↔子共享句柄：构建后注入 `Weak<Agent>`。递归深度改由 [`SUBAGENT_DEPTH`] task-local 承载
/// （不再用共享计数器），故只保留上限 `max_depth`。
///
/// `log_sink`（可选）：子会话事件落库回调，由 app 装配层注入（接 `FileSessionStore`）——
/// 子会话此前只活在内存、工具返回即丢弃，事后无法审计子智能体用过哪些工具；落库后
/// `sessions/subtask-*/log.jsonl` 留痕（不写 meta，UI 列表不显示，仅审计可查）。
/// 子会话日志落库回调（builder 装配时注入，接 app 层 FileSessionStore）。
pub type SubagentLogSink = Arc<dyn Fn(&str, &[cmx_agent_core::SessionEvent]) + Send + Sync>;

pub struct SubagentHandle {
    agent: OnceLock<Weak<Agent>>,
    max_depth: usize,
    log_sink: OnceLock<SubagentLogSink>,
}

impl SubagentHandle {
    pub fn new(max_depth: usize) -> Self {
        Self {
            agent: OnceLock::new(),
            max_depth,
            log_sink: OnceLock::new(),
        }
    }
    /// 构建出 `Arc<Agent>` 后由 builder 调用，注入弱引用（不成环）。
    pub fn attach(&self, agent: &Arc<Agent>) {
        let _ = self.agent.set(Arc::downgrade(agent));
    }
    /// 注入子会话日志落库回调（builder 装配；重复 set 静默忽略首个之后者）。
    pub fn attach_log_sink(&self, f: SubagentLogSink) {
        let _ = self.log_sink.set(f);
    }
    fn upgrade(&self) -> Option<Arc<Agent>> {
        self.agent.get().and_then(Weak::upgrade)
    }
}

pub struct TaskTool {
    handle: Arc<SubagentHandle>,
}

impl TaskTool {
    pub fn new(handle: Arc<SubagentHandle>) -> Self {
        Self { handle }
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "task",
            "把一个独立子任务交给隔离上下文的子智能体完成（可多步、可用工具），返回其最终结果。用于并行/拆解复杂任务。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": "交给子智能体的完整子任务描述（自包含）" },
                "label": { "type": "string", "description": "可选：子任务简短标签（便于展示）" }
            },
            "required": ["prompt"]
        }))
        .guard(GuardHints {
            idempotent: false,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(prompt) = input.get("prompt").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("task: 'prompt' is required"));
        };
        let label = input.get("label").and_then(|v| v.as_str()).unwrap_or("子任务");

        // 递归深度闸门：防子智能体无限自我 fan-out。深度取自 task-local（随调用链下传），
        // **不是共享计数器**——故同一步里多个 task 并发时，兄弟各自读到相同父层级、独立判断，
        // 不会因并发而互相顶满闸门（真并行 fan-out 的关键）。
        let depth = SUBAGENT_DEPTH.try_with(|d| *d).unwrap_or(0);
        if depth >= self.handle.max_depth {
            return Ok(ToolResult::err(format!(
                "task: 子智能体递归深度已达上限 {}（拒绝继续 fan-out）",
                self.handle.max_depth
            )));
        }
        let Some(agent) = self.handle.upgrade() else {
            return Ok(ToolResult::err("task: 子智能体不可用（agent 句柄未注入或已释放）"));
        };

        // 隔离会话：全新 Session + 子智能体系统提示。子回合复用同一 agent 的模型/工具/守卫/工作区。
        let sub_id = format!(
            "subtask-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let mut sub = Session::new(sub_id.clone()).with_system(SUBAGENT_SYSTEM);

        // 主体透传：父回合以 IM 绑定用户身份跑时（TURN_SUBJECT task-local 由内核在回合外层置位），
        // 子回合**同一主体**过守卫——旧实现子回合回落 policy.subject（桌面主体），绑定用户的子任务
        // 会以他人身份判权，权限错位。None（桌面登录常态）维持 run_turn 回落语义不变。
        let turn_subject = cmx_agent_core::TURN_SUBJECT.try_with(|s| s.clone()).ok().flatten();
        // 子回合整体在 depth+1 的 task-local 作用域内运行：其内部若再 fan-out，
        // 会读到 depth+1 并据此判断/再嵌套，形成正确的递归层级传播。
        // （两个分支是不同 future 类型，不能同 match 存一个变量——分别在 scope 内 await。）
        let outcome = match &turn_subject {
            Some(subj) => SUBAGENT_DEPTH.scope(depth + 1, agent.run_turn_as(&mut sub, prompt, subj)).await,
            None => SUBAGENT_DEPTH.scope(depth + 1, agent.run_turn(&mut sub, prompt)).await,
        };

        // 子会话日志落库（审计）：子回合此前只存内存、返回即丢。落库失败不致命（warn 即可），
        // 不写 meta → UI 会话列表不显示，仅磁盘留痕供事后追查。
        if let Some(sink) = self.handle.log_sink.get() {
            sink(&sub_id, sub.log.events());
        }

        match outcome {
            Ok(o) => Ok(ToolResult::ok(json!({
                "label": label,
                "final": o.final_text.unwrap_or_default(),
                "steps": o.steps,
                "reason": format!("{:?}", o.reason),
                "events": sub.log.len(),
            }))),
            Err(e) => Ok(ToolResult::err(format!("task: 子智能体执行失败：{e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;
    use cmx_agent_core::model::{ModelResponse, MockModel};
    use cmx_agent_core::{GuardPipeline, Policy, ToolRegistry};
    use std::path::PathBuf;

    // 构造一个「父」agent：MockModel 第一步调 task 子任务，看到结果后收尾。
    fn build_agent_with_task(handle: Arc<SubagentHandle>, sub_reply: &str) -> Arc<Agent> {
        // 子智能体的模型脚本：直接回一句话收尾（无工具）。
        // 父模型脚本：先请求 task 调用，再收尾。这里父子共用同一 agent/model，
        // 故用一个能按「是否已有工具结果」二段式应答的 MockModel。
        let model = Arc::new(MockModel::new([
            // 第一次 complete（父，无历史工具结果）→ 调 task
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c1",
                "task",
                json!({ "prompt": "算个数", "label": "子活" }),
            )]),
            // 子智能体的 complete → 直接收尾
            ModelResponse::text(sub_reply),
            // 父看到 task 结果 → 收尾
            ModelResponse::text("父任务完成"),
        ]));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(handle.clone())));
        let agent = Agent::builder()
            .model(model)
            .tools(reg)
            .guards(GuardPipeline::new())
            .policy(Policy {
                sandbox: SandboxMode::WorkspaceWrite,
                allowed_roots: vec![PathBuf::from("/tmp")],
                ..Default::default()
            })
            .build()
            .unwrap();
        let agent = Arc::new(agent);
        handle.attach(&agent);
        agent
    }

    #[tokio::test]
    async fn subagent_runs_and_returns_final() {
        let handle = Arc::new(SubagentHandle::new(2));
        let agent = build_agent_with_task(handle.clone(), "子任务答案=42");
        let mut session = Session::new("parent");
        let outcome = agent.run_turn(&mut session, "帮我拆一个子任务").await.unwrap();
        // 父回合应完成，且日志里包含 task 的 tool_result（final=子任务答案）
        assert!(matches!(
            outcome.reason,
            cmx_agent_core::event::StopReason::Completed
        ));
        let has_sub_result = session.log.events().iter().any(|e| {
            matches!(&e.kind, cmx_agent_core::event::EventKind::ToolResult { output, .. }
                if output.get("final").and_then(|v| v.as_str()) == Some("子任务答案=42"))
        });
        assert!(has_sub_result, "父日志应含子智能体的最终结果");
    }

    #[tokio::test]
    async fn concurrent_siblings_not_blocked_by_depth_gate() {
        // 一步内两个 task 兄弟**并发**执行；即便 max_depth=1，也应二者**都**通过深度闸门。
        // （旧实现用共享 AtomicUsize 计数器：并发时计数被顶到 1，第 2 个兄弟 load 到 1>=1 被
        //  误判为"递归过深"而拒绝。改 task-local 后，兄弟各读父层级 0<1，互不干扰。）
        let handle = Arc::new(SubagentHandle::new(1));
        let model = Arc::new(MockModel::new([
            // 父第一步：一次发起两个子任务（c1/c2）
            ModelResponse::calls(vec![
                cmx_agent_core::ToolCall::with_id("c1", "task", json!({ "prompt": "子任务A" })),
                cmx_agent_core::ToolCall::with_id("c2", "task", json!({ "prompt": "子任务B" })),
            ]),
        ])); // 脚本耗尽后走默认 fallback=text("done")：两个子回合与父收尾都用它
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(handle.clone())));
        let agent = Agent::builder()
            .model(model)
            .tools(reg)
            .guards(GuardPipeline::new())
            .policy(Policy {
                sandbox: SandboxMode::WorkspaceWrite,
                allowed_roots: vec![PathBuf::from("/tmp")],
                ..Default::default()
            })
            .build()
            .unwrap();
        let agent = Arc::new(agent);
        handle.attach(&agent);

        let mut session = Session::new("parent");
        let outcome = agent
            .run_turn(&mut session, "并发拆两个子任务")
            .await
            .unwrap();
        assert!(matches!(
            outcome.reason,
            cmx_agent_core::event::StopReason::Completed
        ));

        // 两个 task 的结果都应成功（final="done"），无一被深度闸门拒绝。
        let ok_task_results = session
            .log
            .events()
            .iter()
            .filter(|e| {
                matches!(&e.kind,
                    cmx_agent_core::event::EventKind::ToolResult { ok, output, .. }
                        if *ok && output.get("final").and_then(|v| v.as_str()) == Some("done"))
            })
            .count();
        assert_eq!(ok_task_results, 2, "两个并发子智能体都应成功通过深度闸门");
    }

    #[tokio::test]
    async fn depth_guard_blocks_when_maxed() {
        // max_depth=0 → 任何 task 调用都应被拒绝
        let handle = Arc::new(SubagentHandle::new(0));
        let agent = build_agent_with_task(handle.clone(), "不该被调用");
        let roots = vec![PathBuf::from("/tmp")];
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let t = TaskTool::new(handle);
        // 直接调工具（绕过模型），验证深度闸门
        let _ = agent; // 保持 agent 存活以便 upgrade 成功
        let r = t.invoke(json!({"prompt":"x"}), &ctx).await.unwrap();
        assert!(!r.ok, "max_depth=0 应拒绝");
    }

    // 探针工具：记录 invoke 时刻的 TURN_SUBJECT（验证子回合主体透传用）。
    struct SubjectProbe(std::sync::Arc<std::sync::Mutex<Option<String>>>);
    #[async_trait::async_trait]
    impl Tool for SubjectProbe {
        fn spec(&self) -> ToolSpec {
            ToolSpec::new("probe", "记录当前回合主体")
        }
        async fn invoke(&self, _input: Value, _ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
            let s = cmx_agent_core::TURN_SUBJECT.try_with(|s| s.clone()).ok().flatten();
            *self.0.lock().expect("probe lock") = s.map(|x| x.user);
            Ok(ToolResult::ok(json!({ "ok": true })))
        }
    }

    #[tokio::test]
    async fn subagent_inherits_turn_subject() {
        // 父回合以 bob 身份跑（run_turn_as，IM 绑定场景）→ task 派生的子回合**同一主体**：
        // 子回合内 probe 记录的 TURN_SUBJECT 应为 bob（旧实现回落 policy.subject=desktop，权限错位）。
        let probe_seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let probe = Arc::new(SubjectProbe(probe_seen.clone()));
        let handle = Arc::new(SubagentHandle::new(2));
        let model = Arc::new(MockModel::new([
            // 父第 1 次 complete → 调 task
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c1", "task", json!({ "prompt": "查点东西" }),
            )]),
            // 子第 1 次 complete → 调 probe（在子回合 task-local 作用域内）
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id("c2", "probe", json!({}))]),
            // 子收尾 / 父收尾
            ModelResponse::text("子完成"),
            ModelResponse::text("父完成"),
        ]));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(handle.clone())));
        reg.register(probe);
        let agent = Agent::builder()
            .model(model)
            .tools(reg)
            .guards(GuardPipeline::new())
            .policy(Policy {
                sandbox: SandboxMode::WorkspaceWrite,
                allowed_roots: vec![PathBuf::from("/tmp")],
                ..Default::default()
            })
            .build()
            .unwrap();
        let agent = Arc::new(agent);
        handle.attach(&agent);

        let subject = cmx_agent_core::Subject::new("bob");
        let mut session = Session::new("parent-im");
        agent.run_turn_as(&mut session, "IM 来活", &subject).await.unwrap();
        assert_eq!(
            probe_seen.lock().expect("probe lock").as_deref(),
            Some("bob"),
            "子回合内读到的回合主体应是 IM 绑定用户 bob"
        );
    }

    #[tokio::test]
    async fn missing_prompt_errors() {
        let handle = Arc::new(SubagentHandle::new(2));
        let roots = vec![PathBuf::from("/tmp")];
        let ctx = ToolCtx { sandbox: SandboxMode::WorkspaceWrite, allowed_roots: &roots };
        let t = TaskTool::new(handle);
        let r = t.invoke(json!({}), &ctx).await.unwrap();
        assert!(!r.ok);
    }
}
