//! `task` —— 子智能体编排（U1 + 方案 20260914 阶段一/三）。把一个独立子任务 fan-out 给
//! **类型化**的子智能体：按 `subagent_type` 从共享注册表取 [`AgentSpec`]（工具白名单 /
//! 系统提示词 / 可选专属模型），派生一个一次性子 [`Agent`]（守卫/审批/策略与父全量共享，
//! 工具集做**额外收紧**），用全新隔离 [`Session`] 跑一整个回合，结果回灌父回合。
//!
//! 关键不变量：
//! - **控制面收走**（§6.4 v1 硬规则）：子代理一律再收走 `task` / `ask_user` / `exit_plan`——
//!   `task` 防递归炸弹；`ask_user`/`exit_plan` 因子会话无实时事件通道会挂死父回合。
//!   双重保险：内核 [`cmx_agent_core::SUBAGENT_TURN`] 门控（agent.rs 对交互工具直接 dismissed）。
//! - **派生时现建子 Agent**：结构全是 Arc 字段成本可忽略；`ToolRegistry` 必须 `new()`
//!   （`clone` 是共享同表句柄，过滤会直接改掉父进程共享表）。
//! - **深度/主体透传**：[`SUBAGENT_DEPTH`] task-local 深度闸门 + [`cmx_agent_core::TURN_SUBJECT`]
//!   主体透传（前台子回合随父 future 树自动继承 [`cmx_agent_core::TURN_PLAN_MODE`]，不可绕过）。
//! - **后台派生显式 re-scope**（§7.5）：tokio task-local 不跨 `tokio::spawn`——后台闭包内
//!   显式重挂 SUBAGENT_TURN / SUBAGENT_DEPTH / TURN_PLAN_MODE（计划模式继承不可断）。
//! - **并发上限 + 取消**：每父会话并发 4（前台+后台合计，RAII 释放）；子回合登记可取消
//!   旗标，`cancel_for_parent` 级联取消（修复旧实现对 subtask-* 取消恒 no-op）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use async_trait::async_trait;
use cmx_agent_core::agents::{AgentSpec, ModelResolver};
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Agent, Session, Tool, ToolCtx, ToolError, ToolResult, ToolSpec, TurnCancel};
use serde_json::{json, Value};

/// 子智能体默认系统提示：专注、独立、简洁收尾（spec 未写系统提示词时用）。
/// 收尾语约定（09-15）：最终回复会被原样作为结果交付父会话（<task_result> 卡片正文+转述引用），
/// 写「任务已完成」这类状态句会退化成回声——完成状态由系统另行呈现，正文只放结果本体。
const SUBAGENT_SYSTEM: &str = "你是一个子智能体，专注完成被交办的**单一子任务**。\
 可用工具就用工具，独立把任务做完（不要反问父级）。完成后用简洁中文给出**最终结果**，结论在前。\
 你的最终回复就是交付给父级的结果本体：结论、数据、产出直接给出，不要写「任务已完成」之类的状态语\
（完成状态由系统另行呈现）。";

/// 控制面工具（v1 硬规则，§6.4）：子代理一律收走，类型显式 Allow 也不生效。
const CONTROL_PLANE_TOOLS: &[&str] = &["task", "ask_user", cmx_agent_core::EXIT_PLAN_TOOL_NAME];

/// 回灌截断（§8）：父上下文无压缩机制，多条 20k+ 结果会撑爆上下文。
const RESULT_MAX_CHARS: usize = 20_000;

/// 每父会话并发子任务上限（前台 + 后台合计；§8）。
const MAX_CHILDREN_PER_PARENT: usize = 4;

fn truncate_result(s: &str) -> String {
    if s.chars().count() > RESULT_MAX_CHARS {
        let t: String = s.chars().take(RESULT_MAX_CHARS).collect();
        format!("{t}（已截断）")
    } else {
        s.to_string()
    }
}

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
///
/// `injector`（阶段三，可选）：后台子任务完成注入回调（parent_session_id, `<task_result>` 文本），
/// app 层经 `AgentApp::into_shared` 装配（内部 `Weak<AgentApp>` → `send` 唤醒父会话）；
/// 未装配时 `background=true` 直接报错拒绝（fail-closed，不静默丢结果）。
pub type SubagentLogSink = Arc<dyn Fn(&str, &[cmx_agent_core::SessionEvent]) + Send + Sync>;
pub type SubagentInjector =
    Arc<dyn Fn(String, String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> + Send + Sync>;

pub struct SubagentHandle {
    agent: OnceLock<Weak<Agent>>,
    max_depth: usize,
    log_sink: OnceLock<SubagentLogSink>,
    injector: OnceLock<SubagentInjector>,
    /// 活动子任务取消旗标：parent_session_id → [(sub_id, TurnCancel)]（级联取消用）。
    cancels: Mutex<HashMap<String, Vec<(String, TurnCancel)>>>,
    /// 每父会话并发子任务计数（前台+后台合计；key = `ToolCtx.session_id`）。
    active_children: Mutex<HashMap<String, Arc<AtomicUsize>>>,
}

impl SubagentHandle {
    pub fn new(max_depth: usize) -> Self {
        Self {
            agent: OnceLock::new(),
            max_depth,
            log_sink: OnceLock::new(),
            injector: OnceLock::new(),
            cancels: Mutex::new(HashMap::new()),
            active_children: Mutex::new(HashMap::new()),
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
    /// 注入后台完成注入器（app 层 `into_shared` 装配；重复 set 静默忽略）。
    pub fn attach_injector(&self, f: SubagentInjector) {
        let _ = self.injector.set(f);
    }
    /// 后台派生可用性（未装配注入器的无头环境 `background=true` 必须显式拒绝）。
    pub fn has_injector(&self) -> bool {
        self.injector.get().is_some()
    }
    fn upgrade(&self) -> Option<Arc<Agent>> {
        self.agent.get().and_then(Weak::upgrade)
    }

    /// 取一个每父会话并发槽（RAII：guard drop 即释放；超上限返回 None，调用方报错）。
    pub fn acquire_child_slot(&self, parent: &str) -> Option<ChildSlotGuard> {
        let counter = {
            let mut map = self.active_children.lock().expect("children lock");
            map.entry(parent.to_string())
                .or_insert_with(|| Arc::new(AtomicUsize::new(0)))
                .clone()
        };
        if counter.fetch_add(1, Ordering::SeqCst) >= MAX_CHILDREN_PER_PARENT {
            counter.fetch_sub(1, Ordering::SeqCst);
            return None;
        }
        Some(ChildSlotGuard { counter })
    }

    /// 登记一个活动子任务取消旗标（父会话级联取消用）。
    fn register_cancel(&self, parent: &str, sub_id: &str, cancel: TurnCancel) {
        self.cancels
            .lock()
            .expect("cancels lock")
            .entry(parent.to_string())
            .or_default()
            .push((sub_id.to_string(), cancel));
    }

    /// 子任务收尾摘除旗标。
    fn remove_cancel(&self, parent: &str, sub_id: &str) {
        if let Some(entries) = self.cancels.lock().expect("cancels lock").get_mut(parent) {
            entries.retain(|(id, _)| id != sub_id);
        }
    }

    /// 级联取消某父会话的全部活动子任务（前台与后台一并覆盖；`AgentApp::cancel_session_turn` /
    /// `delete_session` 挂调用）。返回被取消的 sub_id 列表（调用方据此连带清理以 subtask-*
    /// 会话名下挂起的审批/提问）。
    pub fn cancel_for_parent_with_ids(&self, parent: &str) -> Vec<String> {
        let entries = self.cancels.lock().expect("cancels lock").remove(parent);
        match entries {
            Some(list) => {
                let ids: Vec<String> = list.iter().map(|(id, _)| id.clone()).collect();
                for (_, c) in list {
                    c.cancel();
                }
                ids
            }
            None => Vec::new(),
        }
    }
}

/// 并发槽 RAII guard：drop 即释放计数（成功/失败/panic 全路径覆盖）。
pub struct ChildSlotGuard {
    counter: Arc<AtomicUsize>,
}

impl Drop for ChildSlotGuard {
    fn drop(&mut self) {
        self.counter.fetch_sub(1, Ordering::SeqCst);
    }
}

pub struct TaskTool {
    handle: Arc<SubagentHandle>,
    /// 共享生效清单（内置 + 覆盖项 + 自定义；app 层 agents.json 变更改此表即热生效——
    /// 内核每步 `tools.specs()` 现调 `TaskTool::spec()` 动态拼枚举）。
    specs: Arc<std::sync::RwLock<Vec<AgentSpec>>>,
    /// 模型解析缝（app 实现：None=父当前模型槽；Some(id)=按 provider id 构建专属模型）。
    resolver: Arc<dyn ModelResolver>,
}

impl TaskTool {
    pub fn new(
        handle: Arc<SubagentHandle>,
        specs: Arc<std::sync::RwLock<Vec<AgentSpec>>>,
        resolver: Arc<dyn ModelResolver>,
    ) -> Self {
        Self {
            handle,
            specs,
            resolver,
        }
    }

    /// 按名取一个启用的类型规格。
    fn lookup(&self, name: &str) -> Result<AgentSpec, String> {
        let list = self
            .specs
            .read()
            .map_err(|_| "子智能体注册表不可用".to_string())?;
        let available: Vec<String> = list.iter().filter(|a| a.enabled).map(|a| a.name.clone()).collect();
        match list.iter().find(|a| a.name == name) {
            Some(s) if s.enabled => Ok(s.clone()),
            Some(_) => Err(format!(
                "task: 子智能体类型 '{name}' 已停用。可用：{}",
                available.join("、")
            )),
            None => Err(format!(
                "task: 未知子智能体类型 '{name}'。可用：{}",
                available.join("、")
            )),
        }
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn spec(&self) -> ToolSpec {
        // 动态枚举：每步 model_context 现调本方法 → agents.json 增删改即时进模型工具清单。
        let (types, desc_list) = match self.specs.read() {
            Ok(list) => {
                let enabled: Vec<&AgentSpec> = list.iter().filter(|a| a.enabled).collect();
                let types: Vec<Value> = enabled
                    .iter()
                    .map(|a| Value::String(a.name.clone()))
                    .collect();
                let descs = enabled
                    .iter()
                    .map(|a| {
                        let d = if a.description.is_empty() { &a.title } else { &a.description };
                        format!("- {}: {}", a.name, d)
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                (types, descs)
            }
            Err(_) => (Vec::new(), String::new()),
        };
        let description = format!(
            "把一个独立子任务交给指定类型的子智能体完成（隔离上下文、可多步用工具），返回其最终结果。\
             用于并行拆解与只读调研（explore 类型只读，适合摸底/信息收集）。\
             可选类型：\n{desc_list}\n缺省 {default}。background=true 时后台执行：立即返回，\
             完成后结果以 <task_result> 消息自动送达本会话——不要轮询。\
             prompt 必须自包含（子智能体看不到本对话历史）。",
            default = cmx_agent_core::agents::GENERAL_PURPOSE,
        );
        ToolSpec::new("task", description)
            .schema(json!({
                "type": "object",
                "properties": {
                    "prompt": { "type": "string", "description": "交给子智能体的完整子任务描述（自包含）" },
                    "description": { "type": "string", "description": "可选：子任务 3-5 词短描述（便于展示）" },
                    "subagent_type": { "type": "string", "enum": types, "description": "子智能体类型；缺省 general-purpose" },
                    "background": { "type": "boolean", "description": "true = 后台执行（立即返回，完成后自动通知）；缺省 false 前台等待结果" }
                },
                "required": ["prompt"]
            }))
            .guard(GuardHints {
                idempotent: false,
                ..Default::default()
            })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        // prompt 取所有权（后台 spawn 闭包需 'static）。
        let Some(prompt) = input.get("prompt").and_then(|v| v.as_str()).map(String::from) else {
            return Ok(ToolResult::err("task: 'prompt' is required"));
        };
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("子任务");
        let stype = input
            .get("subagent_type")
            .and_then(|v| v.as_str())
            .unwrap_or(cmx_agent_core::agents::GENERAL_PURPOSE);
        let background = input.get("background").and_then(|v| v.as_bool()).unwrap_or(false);

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

        // 类型解析（未知/停用 → 报错列出可用）。
        let spec = match self.lookup(stype) {
            Ok(s) => s,
            Err(e) => return Ok(ToolResult::err(e)),
        };

        // 每父会话并发槽（前台 + 后台合计，RAII 释放；guard 存活到任务收尾）。
        let parent_id = ctx.session_id.to_string();
        let Some(_slot) = self.handle.acquire_child_slot(&parent_id) else {
            return Ok(ToolResult::err(format!(
                "task: 子智能体并发已达上限 {MAX_CHILDREN_PER_PARENT}（等已有子任务完成后再派）"
            )));
        };

        // 有效工具集 = 类型选择对父可见集求值（未知名忽略）→ 控制面三件一律收走（v1 硬规则）。
        let visible: Vec<String> = agent.tools().specs().into_iter().map(|s| s.name).collect();
        let mut effective = spec.tools.effective(visible);
        effective.retain(|n| !CONTROL_PLANE_TOOLS.contains(&n.as_str()));
        // 子注册表必须新建：ToolRegistry::clone 是共享同表句柄，clone 后过滤会改掉父进程共享表。
        let mut child_reg = cmx_agent_core::ToolRegistry::new();
        for name in effective {
            if let Some(t) = agent.tools().get(&name) {
                child_reg.register(t);
            }
        }

        // 专属模型解析（None = 父当前模型槽；失败显式报错不静默降级）。
        let model = match self.resolver.resolve(spec.model.as_deref()) {
            Ok(m) => m,
            Err(e) => {
                return Ok(ToolResult::err(format!(
                    "task: 子智能体类型 '{stype}' 的模型不可用：{e}"
                )))
            }
        };

        // 派生子 Agent：守卫/审批/提问服务/策略全量共享父（工具白名单是额外收紧，不是放宽）。
        let child = match Agent::builder()
            .model(model)
            .tools(child_reg)
            .guards(agent.guards().clone())
            .approver(agent.approver())
            .questions(agent.questions())
            .policy(agent.policy())
            .build()
        {
            Ok(a) => a,
            Err(e) => return Ok(ToolResult::err(format!("task: 子智能体装配失败：{e}"))),
        };

        // 隔离会话：全新 Session + 类型系统提示词（空 = 默认子代理提示词）。
        let sub_id = format!(
            "subtask-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let system = if spec.system_prompt.trim().is_empty() {
            SUBAGENT_SYSTEM.to_string()
        } else {
            spec.system_prompt.clone()
        };

        // 主体透传：父回合以 IM 绑定用户身份跑时（TURN_SUBJECT task-local 由内核在回合外层置位），
        // 子回合**同一主体**过守卫。None（桌面登录常态）维持 run_turn 回落语义不变。
        let turn_subject = cmx_agent_core::TURN_SUBJECT.try_with(|s| s.clone()).ok().flatten();

        // 计划模式活开关快照（§7.5）：前台子回合随父 future 树自动继承，无需重挂；
        // 后台子回合 task-local 不跨 spawn，spawn 闭包内**显式 re-scope**（不可绕过）。
        let plan_flag = cmx_agent_core::TURN_PLAN_MODE.try_with(|f| f.clone()).ok();

        if !background {
            return Ok(self
                .run_foreground(
                    &child,
                    &sub_id,
                    &system,
                    &prompt,
                    turn_subject,
                    depth,
                    &parent_id,
                    ctx.allowed_roots,
                )
                .await);
        }

        // —— 后台派生（阶段三）——
        // fail-closed：注入器未装配（CLI/e2e 无头）→ 显式报错，不静默丢结果。
        let Some(injector) = self.handle.injector.get().cloned() else {
            return Ok(ToolResult::err(
                "task: 后台执行不可用（当前环境未装配完成注入器）。请去掉 background 参数前台执行",
            ));
        };
        let bg_cancel = TurnCancel::new();
        self.handle.register_cancel(&parent_id, &sub_id, bg_cancel.clone());
        let handle = self.handle.clone();
        let sink = self.handle.log_sink.get().cloned();
        let roots = ctx.allowed_roots.to_vec(); // spawn 闭包需 'static：所有权快照
        let slot = _slot; // RAII guard 移交后台任务（任务收尾才释放槽）
        let child = Arc::new(child);
        let sub_id_bg = sub_id.clone(); // 回包还要用原 id
        tokio::spawn(async move {
            run_background_child(
                handle,
                child,
                sub_id_bg,
                parent_id,
                system,
                prompt,
                turn_subject,
                bg_cancel,
                depth,
                plan_flag,
                roots,
                sink,
                slot,
                injector,
            )
            .await;
        });

        Ok(ToolResult::ok(json!({
            "task_id": sub_id,
            "subagent_type": stype,
            "description": description,
            "background": true,
            "note": "已在后台启动，完成后自动通知（<task_result> 消息送达本会话），勿轮询",
        })))
    }
}

impl TaskTool {
    /// 前台子回合：在本（父）任务树内跑——TURN_PLAN_MODE / TURN_SUBJECT 等 task-local
    /// 随 await 链自动继承；SUBAGENT_TURN / SUBAGENT_DEPTH 显式重挂。
    #[allow(clippy::too_many_arguments)]
    async fn run_foreground(
        &self,
        child: &Agent,
        sub_id: &str,
        system: &str,
        prompt: &str,
        turn_subject: Option<cmx_agent_core::guard::Subject>,
        depth: usize,
        parent_id: &str,
        roots: &[std::path::PathBuf],
    ) -> ToolResult {
        let mut sub = Session::new(sub_id.to_string()).with_system(system);
        let child_cancel = TurnCancel::new();
        self.handle
            .register_cancel(parent_id, sub_id, child_cancel.clone());
        // 子回合整体在 depth+1 的 task-local 作用域内运行；SUBAGENT_TURN=true 让内核把
        // 交互工具门控为 dismissed（与控制面收走构成双保险）。
        // roots 用父回合工作区根快照（不读共享 policy——两会话并发回合互不踩）。
        // 用可取消入口：父会话取消/删除经 cancel_for_parent 级联打断子回合（修复旧 no-op）。
        let outcome = match &turn_subject {
            Some(subj) => {
                cmx_agent_core::SUBAGENT_TURN
                    .scope(
                        true,
                        SUBAGENT_DEPTH.scope(
                            depth + 1,
                            child.run_turn_observed_as_cancellable(
                                &mut sub,
                                prompt,
                                None,
                                Some(subj),
                                Some(&child_cancel),
                                Some(roots),
                            ),
                        ),
                    )
                    .await
            }
            None => {
                cmx_agent_core::SUBAGENT_TURN
                    .scope(
                        true,
                        SUBAGENT_DEPTH.scope(
                            depth + 1,
                            child.run_turn_observed_as_cancellable(
                                &mut sub,
                                prompt,
                                None,
                                None,
                                Some(&child_cancel),
                                Some(roots),
                            ),
                        ),
                    )
                    .await
            }
        };
        self.handle.remove_cancel(parent_id, sub_id);

        // 子会话日志落库（审计）：失败不致命（warn 即可），不写 meta → UI 列表不显示。
        if let Some(sink) = self.handle.log_sink.get() {
            sink(sub_id, sub.log.events());
        }

        match outcome {
            Ok(o) => {
                let final_text = truncate_result(&o.final_text.unwrap_or_default());
                ToolResult::ok(json!({
                    "final": final_text,
                    "steps": o.steps,
                    "reason": format!("{:?}", o.reason),
                    "events": sub.log.len(),
                }))
            }
            Err(e) => ToolResult::err(format!("task: 子智能体执行失败：{e}")),
        }
    }
}

/// 后台子回合（阶段三）：在独立任务里跑；tokio task-local 不跨 spawn，故 SUBAGENT_TURN /
/// SUBAGENT_DEPTH / TURN_PLAN_MODE 全部**显式 re-scope**（计划模式继承不可绕过，§7.5）。
/// 结束后取消旗标摘除、日志落库、结果以 `<task_result>` 经注入器送回父会话（父忙则排队）。
#[allow(clippy::too_many_arguments)]
async fn run_background_child(
    handle: Arc<SubagentHandle>,
    child: Arc<Agent>,
    sub_id: String,
    parent_id: String,
    system: String,
    prompt: String,
    turn_subject: Option<cmx_agent_core::guard::Subject>,
    bg_cancel: TurnCancel,
    depth: usize,
    plan_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    roots: Vec<std::path::PathBuf>,
    sink: Option<SubagentLogSink>,
    _slot: ChildSlotGuard,
    injector: SubagentInjector,
) {
    // 内层 spawn 捕获 JoinError：panic → state="failed" 注入摘要（外层任务不被炸死）。
    let inner_sub_id = sub_id.clone();
    let inner_sink = sink.clone();
    let inner = tokio::spawn(async move {
        let run_fut = async move {
            let mut sub =
                Session::new(inner_sub_id.clone()).with_system(system);
            let outcome = match &turn_subject {
                Some(subj) => child
                    .run_turn_observed_as_cancellable(
                        &mut sub,
                        &prompt,
                        None,
                        Some(subj),
                        Some(&bg_cancel),
                        Some(&roots),
                    )
                    .await,
                None => child
                    .run_turn_observed_as_cancellable(
                        &mut sub,
                        &prompt,
                        None,
                        None,
                        Some(&bg_cancel),
                        Some(&roots),
                    )
                    .await,
            };
            if let Some(sink) = inner_sink.as_ref() {
                sink(&inner_sub_id, sub.log.events());
            }
            outcome.map(|o| o.final_text.unwrap_or_default())
        };
        // 显式 re-scope：task-local 不跨 tokio::spawn（§7.5 自查点）。
        let scoped = cmx_agent_core::SUBAGENT_TURN.scope(true, SUBAGENT_DEPTH.scope(depth + 1, run_fut));
        match plan_flag {
            Some(flag) => cmx_agent_core::TURN_PLAN_MODE.scope(flag, scoped).await,
            None => scoped.await,
        }
    });

    let joined = inner.await;
    handle.remove_cancel(&parent_id, &sub_id);
    let (state, body) = match joined {
        Ok(Ok(text)) => ("completed", truncate_result(&text)),
        Ok(Err(e)) => ("failed", truncate_result(&format!("子任务执行失败：{e}"))),
        Err(je) => ("failed", format!("子任务异常终止：{je}")),
    };
    let payload = format!("<task_result id=\"{sub_id}\" state=\"{state}\">{body}</task_result>");
    injector(parent_id, payload).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::agents::builtin_specs;
    use cmx_agent_core::guard::SandboxMode;
    use cmx_agent_core::model::{ModelResponse, MockModel, ModelSeam};
    use cmx_agent_core::{GuardPipeline, Policy, ToolRegistry};
    use std::path::PathBuf;

    /// 测试解析器：返回与父相同的模型 Arc（子回合与父共用同一脚本 MockModel，
    /// 保持旧测试「脚本顺序计数」语义成立）。
    struct SameModel(Arc<dyn ModelSeam>);
    impl ModelResolver for SameModel {
        fn resolve(&self, _provider: Option<&str>) -> Result<Arc<dyn ModelSeam>, String> {
            Ok(self.0.clone())
        }
    }

    fn test_specs() -> Arc<std::sync::RwLock<Vec<AgentSpec>>> {
        Arc::new(std::sync::RwLock::new(builtin_specs()))
    }

    fn tool_ctx(roots: &[PathBuf]) -> ToolCtx<'_> {
        ToolCtx {
            sandbox: SandboxMode::WorkspaceWrite,
            allowed_roots: roots,
            session_id: "parent",
        }
    }

    // 构造一个「父」agent：MockModel 第一步调 task 子任务，看到结果后收尾。
    fn build_agent_with_task(handle: Arc<SubagentHandle>, sub_reply: &str) -> Arc<Agent> {
        let model = Arc::new(MockModel::new([
            // 第一次 complete（父，无历史工具结果）→ 调 task
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c1",
                "task",
                json!({ "prompt": "算个数", "description": "子活" }),
            )]),
            // 子智能体的 complete → 直接收尾
            ModelResponse::text(sub_reply),
            // 父看到 task 结果 → 收尾
            ModelResponse::text("父任务完成"),
        ]));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(
            handle.clone(),
            test_specs(),
            Arc::new(SameModel(model.clone())),
        )));
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
        let handle = Arc::new(SubagentHandle::new(1));
        let model = Arc::new(MockModel::new([
            ModelResponse::calls(vec![
                cmx_agent_core::ToolCall::with_id("c1", "task", json!({ "prompt": "子任务A" })),
                cmx_agent_core::ToolCall::with_id("c2", "task", json!({ "prompt": "子任务B" })),
            ]),
        ]));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(
            handle.clone(),
            test_specs(),
            Arc::new(SameModel(model.clone())),
        )));
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
        let handle = Arc::new(SubagentHandle::new(0));
        let agent = build_agent_with_task(handle.clone(), "不该被调用");
        let roots = vec![PathBuf::from("/tmp")];
        let ctx = tool_ctx(&roots);
        let t = TaskTool::new(handle, test_specs(), Arc::new(SameModel(agent.model())));
        let _ = &agent;
        let r = t.invoke(json!({"prompt":"x"}), &ctx).await.unwrap();
        assert!(!r.ok, "max_depth=0 应拒绝");
    }

    #[tokio::test]
    async fn unknown_type_lists_available() {
        let handle = Arc::new(SubagentHandle::new(2));
        let model = Arc::new(MockModel::saying("不该被调到"));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(
            handle.clone(),
            test_specs(),
            Arc::new(SameModel(model.clone())),
        )));
        let agent = Agent::builder()
            .model(model)
            .tools(reg)
            .guards(GuardPipeline::new())
            .policy(Policy::default())
            .build()
            .unwrap();
        let agent = Arc::new(agent);
        handle.attach(&agent);
        let roots = vec![PathBuf::from("/tmp")];
        let ctx = tool_ctx(&roots);
        let t = TaskTool::new(handle, test_specs(), Arc::new(SameModel(agent.model())));
        let r = t
            .invoke(json!({"prompt":"x","subagent_type":"nope"}), &ctx)
            .await
            .unwrap();
        assert!(!r.ok);
        let msg = r.output["error"].as_str().unwrap();
        assert!(msg.contains("未知子智能体类型"), "{msg}");
        assert!(msg.contains("general-purpose") && msg.contains("explore"), "{msg}");
    }

    #[tokio::test]
    async fn explore_type_strips_write_tools_and_control_plane() {
        // explore = Allow 只读 8 件 + 控制面收走：派生的子注册表应恰好含白名单内、
        // 且无 task/ask_user/exit_plan；fs_write 不在白名单 → 不可见。
        let probe_seen = Arc::new(Mutex::new(Vec::new()));
        struct ListProbe(Arc<Mutex<Vec<String>>>);
        #[async_trait::async_trait]
        impl Tool for ListProbe {
            fn spec(&self) -> ToolSpec {
                ToolSpec::new("probe_tools", "列出本进程可见工具")
            }
            async fn invoke(
                &self,
                _input: Value,
                ctx: &ToolCtx<'_>,
            ) -> Result<ToolResult, ToolError> {
                // 经 ctx 无注册表；改从 closure 外侧校验——这里只记录调用发生。
                self.0.lock().expect("probe").push(ctx.session_id.to_string());
                Ok(ToolResult::ok(json!({"ok": true})))
            }
        }
        // 白名单含 update_plan 但测试注册表里没有 update_plan 工具 → 未知名忽略。
        // 注册 fs_read + probe_tools（probe 不在 explore 白名单 → 子代理应看不到）。
        use crate::FsReadTool;
        let handle = Arc::new(SubagentHandle::new(2));
        let model = Arc::new(MockModel::new([
            // 父 → 调 task(explore)
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c1",
                "task",
                json!({ "prompt": "调研", "subagent_type": "explore" }),
            )]),
            // 子 → 调 probe_tools（不应存在：unknown tool 回灌）
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c2",
                "probe_tools",
                json!({}),
            )]),
            ModelResponse::text("子完成"),
            ModelResponse::text("父完成"),
        ]));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(
            handle.clone(),
            test_specs(),
            Arc::new(SameModel(model.clone())),
        )));
        reg.register(Arc::new(FsReadTool));
        reg.register(Arc::new(ListProbe(probe_seen.clone())));
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
        // 子会话事件经 log_sink 捕获（子代理的工具结果不进父日志——只回灌 final 文本）。
        let sub_events = Arc::new(Mutex::new(Vec::new()));
        {
            let cap = sub_events.clone();
            handle.attach_log_sink(Arc::new(move |_id, events| {
                cap.lock().unwrap().extend(events.iter().cloned());
            }));
        }
        let mut session = Session::new("parent");
        agent.run_turn(&mut session, "派 explore 调研").await.unwrap();
        // 子调 probe_tools 不在其注册表（不在 explore 白名单）→ 路由 unknown tool 回灌在子日志里。
        let unknown_in_child = sub_events.lock().unwrap().iter().any(|e| {
            matches!(&e.kind, cmx_agent_core::event::EventKind::ToolResult { output, .. }
                if output.get("error").and_then(|v| v.as_str()).map(|s| s.contains("unknown tool 'probe_tools'")).unwrap_or(false))
        });
        assert!(unknown_in_child, "explore 子代理不应看到白名单外的 probe_tools");
        assert!(
            probe_seen.lock().unwrap().is_empty(),
            "probe_tools 不应被任何一方执行"
        );
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
        let probe_seen = std::sync::Arc::new(std::sync::Mutex::new(None));
        let probe = Arc::new(SubjectProbe(probe_seen.clone()));
        let handle = Arc::new(SubagentHandle::new(2));
        let model = Arc::new(MockModel::new([
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c1", "task", json!({ "prompt": "查点东西" }),
            )]),
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id("c2", "probe", json!({}))]),
            ModelResponse::text("子完成"),
            ModelResponse::text("父完成"),
        ]));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(
            handle.clone(),
            test_specs(),
            Arc::new(SameModel(model.clone())),
        )));
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
        agent
            .run_turn_as(&mut session, "IM 来活", &subject)
            .await
            .unwrap();
        assert_eq!(
            probe_seen.lock().expect("probe lock").as_deref(),
            Some("bob"),
            "子回合内读到的回合主体应是 IM 绑定用户 bob"
        );
    }

    #[tokio::test]
    async fn missing_prompt_errors() {
        let handle = Arc::new(SubagentHandle::new(2));
        let model = Arc::new(MockModel::saying("m"));
        let roots = vec![PathBuf::from("/tmp")];
        let ctx = tool_ctx(&roots);
        let t = TaskTool::new(handle, test_specs(), Arc::new(SameModel(model)));
        let r = t.invoke(json!({}), &ctx).await.unwrap();
        assert!(!r.ok);
    }

    #[tokio::test]
    async fn concurrency_limit_rejects_fifth() {
        // 每父会话上限 4：直接压 handle 的槽接口（一步 6 个 task 需要模型脚本长跑，此处验证计数语义）。
        let handle = Arc::new(SubagentHandle::new(2));
        let _g1 = handle.acquire_child_slot("p");
        let _g2 = handle.acquire_child_slot("p");
        let _g3 = handle.acquire_child_slot("p");
        let _g4 = handle.acquire_child_slot("p");
        assert!(handle.acquire_child_slot("p").is_none(), "第 5 个应被拒");
        assert!(handle.acquire_child_slot("other").is_some(), "其它父会话不受影响");
        drop(_g1);
        assert!(handle.acquire_child_slot("p").is_some(), "释放一个槽后可再取");
    }

    #[tokio::test]
    async fn background_requires_injector() {
        let handle = Arc::new(SubagentHandle::new(2));
        let model = Arc::new(MockModel::saying("m"));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(
            handle.clone(),
            test_specs(),
            Arc::new(SameModel(model.clone())),
        )));
        let agent = Agent::builder()
            .model(model)
            .tools(reg)
            .guards(GuardPipeline::new())
            .policy(Policy::default())
            .build()
            .unwrap();
        let agent = Arc::new(agent);
        handle.attach(&agent);
        let roots = vec![PathBuf::from("/tmp")];
        let ctx = tool_ctx(&roots);
        let t = TaskTool::new(handle, test_specs(), Arc::new(SameModel(agent.model())));
        let r = t
            .invoke(json!({"prompt":"x","background":true}), &ctx)
            .await
            .unwrap();
        assert!(!r.ok, "注入器未装配时 background=true 必须 fail-closed 拒绝");
        let msg = r.output["error"].as_str().unwrap();
        assert!(msg.contains("后台执行不可用"), "{msg}");
    }

    #[tokio::test]
    async fn background_injects_task_result_on_completion() {
        // 有注入器：后台子任务完成后 <task_result> 应送达注入器（= 父会话）。
        let handle = Arc::new(SubagentHandle::new(2));
        let model = Arc::new(MockModel::new([
            ModelResponse::calls(vec![cmx_agent_core::ToolCall::with_id(
                "c1",
                "task",
                json!({ "prompt": "后台活", "background": true }),
            )]),
            ModelResponse::text("父立即返回"),
        ]));
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(TaskTool::new(
            handle.clone(),
            test_specs(),
            Arc::new(SameModel(model.clone())),
        )));
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

        let delivered = Arc::new(Mutex::new(Vec::new()));
        let delivered2 = delivered.clone();
        handle.attach_injector(Arc::new(move |parent, text| {
            let d = delivered2.clone();
            Box::pin(async move {
                d.lock().unwrap().push((parent, text));
            }) as std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>
        }));

        let mut session = Session::new("parent-bg");
        let outcome = agent.run_turn(&mut session, "后台派活").await.unwrap();
        assert!(matches!(
            outcome.reason,
            cmx_agent_core::event::StopReason::Completed
        ));
        // 等后台任务收尾（子回合脚本 fallback=text("done")，很快）
        for _ in 0..100 {
            if !delivered.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let got = delivered.lock().unwrap();
        assert_eq!(got.len(), 1, "应恰好注入一条 <task_result>");
        let (parent, text) = &got[0];
        assert_eq!(parent, "parent-bg");
        assert!(text.starts_with("<task_result") && text.contains("state=\"completed\""), "{text}");
        assert!(text.contains("done"), "{text}");
    }
}
