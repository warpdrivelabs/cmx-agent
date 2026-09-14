//! 交互式人在环审批（X4）：把 [`cmx_agent_core::Approver`] 实现成"挂起等前端点按"。
//!
//! 回合跑到需审批的工具时，内核先落 `ApprovalRequested` 事件（经流式推给前端弹审批卡片），随后
//! `await` 本审批者的 `resolve`——它注册一个以 `call_id` 为键的 oneshot 并**挂起等待**。用户在卡片上
//! 点「允许/拒绝」→ 前端发 `{cmd:"approve",call_id,approved}` → `AgentApp` 同步 `decide` 弹出并唤醒
//! 该 oneshot → `resolve` 返回，回合继续。超时（默认 300s）无人应答则按拒绝处理，避免回合永久挂起。

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use cmx_agent_core::{call_summary, Approver, ToolCall};
use serde::Serialize;
use tokio::sync::oneshot;

/// 前端送回的审批决定：是否批准 + 可选附言（「告诉模型接下来应该怎么做」，拒绝时回灌给模型）。
#[derive(Debug, Clone)]
pub struct ApprovalReply {
    pub approved: bool,
    pub note: String,
}

/// 一条待决审批的展示信息（刷新后在途恢复：重画审批卡用）。
#[derive(Debug, Clone, Serialize)]
pub struct PendingApprovalInfo {
    pub call_id: String,
    pub session_id: String,
    pub tool: String,
    pub reason: String,
    pub summary: String,
}

/// 交互式审批者。待决审批以 `call_id` → oneshot 发送端登记。
pub struct InteractiveApprover {
    pending: Mutex<HashMap<String, oneshot::Sender<ApprovalReply>>>,
    /// call_id → 展示信息：会话中断时需精准拒绝该会话的所有待决审批；刷新后恢复卡面也要查它。
    pending_info: Mutex<HashMap<String, PendingApprovalInfo>>,
    /// 已授予「本对话全部允许」的会话 id 集合——命中则内核跳过审批直接放行（不弹卡）。
    /// 进程内内存态：App 重启即清空（重启后不应静默沿用旧授权，需重新确认）。
    allow_all: Mutex<HashSet<String>>,
    timeout: Duration,
}

impl Default for InteractiveApprover {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}

impl InteractiveApprover {
    pub fn new(timeout: Duration) -> Self {
        Self {
            pending: Mutex::new(HashMap::new()),
            pending_info: Mutex::new(HashMap::new()),
            allow_all: Mutex::new(HashSet::new()),
            timeout,
        }
    }

    /// 前端送回决定：弹出该 `call_id` 的等待端并唤醒（同步，不需运行时）。返回是否命中一个待决审批。
    /// `session_hint` 非空时校验会话绑定——call_id 由模型自报、跨会话可能重号，
    /// B 会话的决定不得作用于 A 会话挂起的审批（防跨会话误批/误拒）。
    pub fn decide(&self, call_id: &str, approved: bool, session_hint: &str, note: &str) -> bool {
        let bound = self
            .pending_info
            .lock()
            .expect("pending info lock")
            .get(call_id)
            .map(|i| i.session_id.clone());
        if !session_hint.is_empty() && bound.as_deref().is_some_and(|sid| sid != session_hint) {
            eprintln!(
                "[approval] 审批 {call_id} 属于会话 {:?}，拒绝来自会话 {session_hint:?} 的决定",
                bound.as_deref().unwrap_or("")
            );
            return false;
        }
        let tx = self.pending.lock().expect("pending lock").remove(call_id);
        self.pending_info.lock().expect("pending info lock").remove(call_id);
        match tx {
            Some(tx) => tx
                .send(ApprovalReply { approved, note: note.to_string() })
                .is_ok(),
            None => false,
        }
    }

    /// 全量撤销（登出/换账号）：拒绝所有待决审批 + 清空「全部允许」授权——
    /// 免审批授权不得跨登录身份沿用。
    pub fn revoke_all(&self) {
        let ids: Vec<String> = self
            .pending
            .lock()
            .expect("pending lock")
            .keys()
            .cloned()
            .collect();
        for id in ids {
            let _ = self.decide(&id, false, "", "");
        }
        self.allow_all.lock().expect("allow_all lock").clear();
    }

    /// 授予某会话「本对话全部允许」——此后该会话需审批的工具全部自动放行，不再弹卡。
    pub fn allow_session(&self, session_id: &str) {
        self.allow_all
            .lock()
            .expect("allow_all lock")
            .insert(session_id.to_string());
    }

    /// 撤销某会话的「全部允许」授权（如用户在设置里关闭 / 会话删除时清理）。
    pub fn revoke_session(&self, session_id: &str) {
        self.allow_all.lock().expect("allow_all lock").remove(session_id);
    }

    /// 某会话当前待决审批的展示信息（刷新后在途恢复：前端重画审批卡）。
    pub fn pending_in_session(&self, session_id: &str) -> Vec<PendingApprovalInfo> {
        self.pending_info
            .lock()
            .expect("pending info lock")
            .values()
            .filter(|i| i.session_id == session_id)
            .cloned()
            .collect()
    }

    /// 当前待决审批的 call_id 列表（调试/兜底用）。
    pub fn pending_ids(&self) -> Vec<String> {
        self.pending.lock().expect("pending lock").keys().cloned().collect()
    }
}

#[async_trait]
impl Approver for InteractiveApprover {
    async fn resolve(&self, call: &ToolCall, reason: &str) -> (bool, String) {
        let (ok, by, _) = self.resolve_for_session_with_note("", call, reason).await;
        (ok, by)
    }

    async fn resolve_for_session(
        &self,
        session_id: &str,
        call: &ToolCall,
        reason: &str,
    ) -> (bool, String) {
        let (ok, by, _) = self.resolve_for_session_with_note(session_id, call, reason).await;
        (ok, by)
    }

    async fn resolve_for_session_with_note(
        &self,
        session_id: &str,
        call: &ToolCall,
        reason: &str,
    ) -> (bool, String, Option<String>) {
        let (tx, rx) = oneshot::channel::<ApprovalReply>();
        {
            let mut p = self.pending.lock().expect("pending lock");
            // 同一 call_id 若已有待决（不应发生），丢弃旧的（其接收端会得到 Err → 视为拒绝）。
            p.insert(call.id.clone(), tx);
        }
        self.pending_info.lock().expect("pending info lock").insert(
            call.id.clone(),
            PendingApprovalInfo {
                call_id: call.id.clone(),
                session_id: session_id.to_string(),
                tool: call.name.clone(),
                reason: reason.to_string(),
                summary: call_summary(call),
            },
        );
        // 挂起等前端决定；超时 / 发送端被覆盖（同 id 重登记）按拒绝，by 区分留审计。
        let (reply, by) = match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(reply)) => (Some(reply), "user"),
            Ok(Err(_canceled)) => {
                self.pending.lock().expect("pending lock").remove(&call.id);
                (None, "canceled")
            }
            Err(_timeout) => {
                self.pending.lock().expect("pending lock").remove(&call.id);
                (None, "timeout")
            }
        };
        self.pending_info.lock().expect("pending info lock").remove(&call.id);
        match reply {
            // 决定来自 decide（user 主动点按/中断清理走 decide 发送）
            Some(r) if r.approved => (true, "user".into(), None),
            Some(r) => (false, "user".into(), (!r.note.trim().is_empty()).then_some(r.note)),
            None => (false, by.into(), None),
        }
    }

    /// 本会话是否已授予「全部允许」。
    fn is_preapproved(&self, session_id: &str) -> bool {
        self.allow_all.lock().expect("allow_all lock").contains(session_id)
    }

    fn cancel_session(&self, session_id: &str) {
        let ids: Vec<String> = self
            .pending_info
            .lock()
            .expect("pending info lock")
            .values()
            .filter(|i| i.session_id == session_id)
            .map(|i| i.call_id.clone())
            .collect();
        for call_id in ids {
            let _ = self.decide(&call_id, false, "", "");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn decide_approves_pending() {
        let approver = std::sync::Arc::new(InteractiveApprover::new(Duration::from_secs(5)));
        let call = ToolCall::with_id("c1", "shell", json!({"cmd":"ls"}));
        let a2 = approver.clone();
        // 并发：一个 task 挂起等待，另一处 decide 唤醒
        let h = tokio::spawn(async move { a2.resolve(&call, "需审批").await });
        // 让 resolve 先登记
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(approver.pending_ids(), vec!["c1".to_string()]);
        assert!(approver.decide("c1", true, "", ""));
        let (ok, by) = h.await.unwrap();
        assert!(ok);
        assert_eq!(by, "user");
        assert!(approver.pending_ids().is_empty());
    }

    #[tokio::test]
    async fn decide_rejects_pending_with_note() {
        let approver = std::sync::Arc::new(InteractiveApprover::new(Duration::from_secs(5)));
        let call = ToolCall::with_id("c2", "shell", json!({}));
        let a2 = approver.clone();
        let h = tokio::spawn(async move { a2.resolve_for_session_with_note("s1", &call, "x").await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        // 拒绝并附言：附言应原样带回（「告诉模型接下来应该怎么做」）
        assert!(approver.decide("c2", false, "", "改用 echo 写入"));
        let (ok, by, note) = h.await.unwrap();
        assert!(!ok);
        assert_eq!(by, "user");
        assert_eq!(note.as_deref(), Some("改用 echo 写入"));
        // 待决展示信息（刷新恢复用）：应含会话/工具/摘要
        assert!(approver.pending_in_session("s1").is_empty());
    }

    #[tokio::test]
    async fn pending_in_session_lists_suspended_approval() {
        let approver = std::sync::Arc::new(InteractiveApprover::new(Duration::from_secs(5)));
        let call = ToolCall::with_id("c4", "shell", json!({"cmd":"touch a.txt"}));
        let a2 = approver.clone();
        let h = tokio::spawn(async move { a2.resolve_for_session("s9", &call, "需审批").await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let list = approver.pending_in_session("s9");
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].call_id, "c4");
        assert_eq!(list[0].tool, "shell");
        assert_eq!(list[0].summary, "touch a.txt", "单字符串字段应直接取值");
        assert!(approver.pending_in_session("other").is_empty(), "按会话过滤");
        assert!(approver.decide("c4", true, "s9", ""));
        let _ = h.await.unwrap();
    }

    #[tokio::test]
    async fn timeout_rejects() {
        let approver = InteractiveApprover::new(Duration::from_millis(80));
        let call = ToolCall::with_id("c3", "shell", json!({}));
        let (ok, by) = approver.resolve(&call, "x").await;
        assert!(!ok);
        assert_eq!(by, "timeout");
    }

    #[test]
    fn decide_unknown_id_is_noop() {
        let approver = InteractiveApprover::default();
        assert!(!approver.decide("nope", true, "", ""));
    }

    #[test]
    fn allow_session_scopes_preapproval_per_conversation() {
        let approver = InteractiveApprover::default();
        assert!(!approver.is_preapproved("s1"));
        approver.allow_session("s1");
        assert!(approver.is_preapproved("s1"), "授权会话应被 pre-approve");
        assert!(!approver.is_preapproved("s2"), "授权仅限该会话，不外溢");
        approver.revoke_session("s1");
        assert!(!approver.is_preapproved("s1"), "撤销后应恢复需审批");
    }
}
