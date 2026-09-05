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
use cmx_agent_core::{Approver, ToolCall};
use tokio::sync::oneshot;

/// 交互式审批者。待决审批以 `call_id` → oneshot 发送端登记。
pub struct InteractiveApprover {
    pending: Mutex<HashMap<String, oneshot::Sender<bool>>>,
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
            allow_all: Mutex::new(HashSet::new()),
            timeout,
        }
    }

    /// 前端送回决定：弹出该 `call_id` 的等待端并唤醒（同步，不需运行时）。返回是否命中一个待决审批。
    pub fn decide(&self, call_id: &str, approved: bool) -> bool {
        let tx = self.pending.lock().expect("pending lock").remove(call_id);
        match tx {
            Some(tx) => tx.send(approved).is_ok(),
            None => false,
        }
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

    /// 当前待决审批的 call_id 列表（调试/兜底用）。
    pub fn pending_ids(&self) -> Vec<String> {
        self.pending.lock().expect("pending lock").keys().cloned().collect()
    }
}

#[async_trait]
impl Approver for InteractiveApprover {
    async fn resolve(&self, call: &ToolCall, _reason: &str) -> (bool, String) {
        let (tx, rx) = oneshot::channel::<bool>();
        {
            let mut p = self.pending.lock().expect("pending lock");
            // 同一 call_id 若已有待决（不应发生），丢弃旧的（其接收端会得到 Err → 视为拒绝）。
            p.insert(call.id.clone(), tx);
        }
        // 挂起等前端决定；超时按拒绝。
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(approved)) => (approved, "user".into()),
            Ok(Err(_canceled)) => {
                // 发送端被丢弃（如被同 id 覆盖）→ 拒绝
                self.pending.lock().expect("pending lock").remove(&call.id);
                (false, "canceled".into())
            }
            Err(_timeout) => {
                self.pending.lock().expect("pending lock").remove(&call.id);
                (false, "timeout".into())
            }
        }
    }

    /// 本会话是否已授予「全部允许」。
    fn is_preapproved(&self, session_id: &str) -> bool {
        self.allow_all.lock().expect("allow_all lock").contains(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn decide_approves_pending() {
        let approver = std::sync::Arc::new(InteractiveApprover::new(Duration::from_secs(5)));
        let call = ToolCall::with_id("c1", "bash", json!({"cmd":"ls"}));
        let a2 = approver.clone();
        // 并发：一个 task 挂起等待，另一处 decide 唤醒
        let h = tokio::spawn(async move { a2.resolve(&call, "需审批").await });
        // 让 resolve 先登记
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(approver.pending_ids(), vec!["c1".to_string()]);
        assert!(approver.decide("c1", true));
        let (ok, by) = h.await.unwrap();
        assert!(ok);
        assert_eq!(by, "user");
        assert!(approver.pending_ids().is_empty());
    }

    #[tokio::test]
    async fn decide_rejects_pending() {
        let approver = std::sync::Arc::new(InteractiveApprover::new(Duration::from_secs(5)));
        let call = ToolCall::with_id("c2", "bash", json!({}));
        let a2 = approver.clone();
        let h = tokio::spawn(async move { a2.resolve(&call, "x").await });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(approver.decide("c2", false));
        let (ok, _) = h.await.unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn timeout_rejects() {
        let approver = InteractiveApprover::new(Duration::from_millis(80));
        let call = ToolCall::with_id("c3", "bash", json!({}));
        let (ok, by) = approver.resolve(&call, "x").await;
        assert!(!ok);
        assert_eq!(by, "timeout");
    }

    #[test]
    fn decide_unknown_id_is_noop() {
        let approver = InteractiveApprover::default();
        assert!(!approver.decide("nope", true));
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
