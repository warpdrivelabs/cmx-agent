//! 会话落库测试：JSONL 往返、增量 append、重启恢复回合号、损坏检测、路径注入防御。

use std::path::PathBuf;
use std::sync::Arc;

use cmx_agent_app::store::{FileSessionStore, SessionMeta, SessionStore};
use cmx_agent_app::{AppError, DesktopAppBuilder};
use cmx_agent_core::event::{EventKind, SessionEvent};
use cmx_agent_core::{MockModel, ModelResponse, ToolCall};

/// 唯一临时目录，测试结束自清。
struct TempDir(PathBuf);
impl TempDir {
    fn new(tag: &str) -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("cmx-agent-{tag}-{n}"));
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn sample_events() -> Vec<SessionEvent> {
    vec![
        SessionEvent {
            seq: 1,
            ts: chrono::Utc::now(),
            kind: EventKind::TurnStarted {
                turn: 1,
                user_input: "hi".into(),
            },
        },
        SessionEvent {
            seq: 2,
            ts: chrono::Utc::now(),
            kind: EventKind::UserMessage { text: "hi".into() },
        },
        SessionEvent {
            seq: 3,
            ts: chrono::Utc::now(),
            kind: EventKind::TurnEnded {
                turn: 1,
                reason: cmx_agent_core::event::StopReason::Completed,
                steps: 1,
            },
        },
    ]
}

#[test]
fn append_and_load_roundtrip() {
    let tmp = TempDir::new("store-rt");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    let evs = sample_events();
    store.append_events("s1", &evs).unwrap();

    let session = store.load("s1").unwrap();
    assert_eq!(session.log.len(), 3);
    // 事件逐条相等（seq/ts/kind 全保真）
    for (a, b) in session.log.events().iter().zip(evs.iter()) {
        assert_eq!(a, b);
    }
}

#[test]
fn incremental_append_does_not_rewrite_history() {
    let tmp = TempDir::new("store-incr");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    store.append_events("s1", &sample_events()).unwrap();
    // 再追加两条
    let more = vec![
        SessionEvent {
            seq: 4,
            ts: chrono::Utc::now(),
            kind: EventKind::Note { text: "n1".into() },
        },
        SessionEvent {
            seq: 5,
            ts: chrono::Utc::now(),
            kind: EventKind::Note { text: "n2".into() },
        },
    ];
    store.append_events("s1", &more).unwrap();

    let session = store.load("s1").unwrap();
    assert_eq!(session.log.len(), 5);
    assert_eq!(session.log.events()[4].seq, 5);
    // 行数应正好 5（无重写、无重复）
    let raw = std::fs::read_to_string(tmp.path().join("sessions/s1/log.jsonl")).unwrap();
    assert_eq!(raw.lines().filter(|l| !l.trim().is_empty()).count(), 5);
}

#[test]
fn restored_session_continues_turn_numbering() {
    let tmp = TempDir::new("store-turnno");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    store.append_events("s1", &sample_events()).unwrap(); // 含 1 个 TurnEnded
    let session = store.load("s1").unwrap();
    // 已完成 1 个回合 → 下一个回合号应为 2
    assert_eq!(session.next_turn_no(), 2);
}

#[test]
fn list_sorts_by_updated_desc() {
    let tmp = TempDir::new("store-list");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    let t0 = chrono::Utc::now();
    for (i, id) in ["a", "b", "c"].iter().enumerate() {
        let meta = SessionMeta {
            id: id.to_string(),
            title: None,
            system: None,
            created_at: t0,
            updated_at: t0 + chrono::Duration::seconds(i as i64),
            plan_mode: false,
            event_count: 0,
            workspace_id: None,
        };
        store.put_meta(&meta).unwrap();
    }
    let list = store.list().unwrap();
    let ids: Vec<_> = list.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["c", "b", "a"], "newest updated first");
}

#[test]
fn load_missing_is_not_found() {
    let tmp = TempDir::new("store-missing");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    let err = store.load("ghost").unwrap_err();
    assert!(matches!(err, AppError::NotFound(_)));
}

#[test]
fn corrupt_line_is_skipped_not_fatal() {
    let tmp = TempDir::new("store-corrupt");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    store.append_events("s1", &sample_events()).unwrap();
    // 中段坏行=并发残迹/崩溃残留：跳过并恢复其余事件（红蓝审查 P2-5 放宽——双壳共享数据根后
    // 中段交错残迹会砖死整个会话，比丢一行审计事件更伤；模型上下文对孤儿工具调用合成 interrupted）。
    let path = tmp.path().join("sessions/s1/log.jsonl");
    let content = std::fs::read_to_string(&path).unwrap();
    let mut lines: Vec<String> = content.lines().map(String::from).collect();
    lines.insert(lines.len() / 2, "{not valid json}".into());
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();

    let session = store.load("s1").unwrap();
    assert_eq!(
        session.log.len(),
        sample_events().len(),
        "除坏行外其余事件应完整恢复"
    );
}

#[test]
fn trailing_partial_line_is_tolerated() {
    // 末行损坏 = 进程中断的 append 残迹：跳过自愈，不砖死整个会话。
    let tmp = TempDir::new("store-tail");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    store.append_events("s1", &sample_events()).unwrap();
    let path = tmp.path().join("sessions/s1/log.jsonl");
    let mut content = std::fs::read_to_string(&path).unwrap();
    content.push_str("{truncated json");
    std::fs::write(&path, content).unwrap();
    let session = store.load("s1").unwrap();
    assert_eq!(session.log.len(), 3, "有效事件应完整恢复，仅跳过末尾残行");
}

#[test]
fn path_injection_is_rejected() {
    let tmp = TempDir::new("store-inject");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    for bad in ["../evil", "a/b", "..", "", "x\0y", "CON", "nul", "Com1", "abc.", "x "] {
        let err = store.append_events(bad, &sample_events()).unwrap_err();
        assert!(
            matches!(err, AppError::BadRequest(_)),
            "id '{bad}' must be rejected"
        );
    }
}

#[test]
fn delete_is_idempotent() {
    let tmp = TempDir::new("store-del");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    store.append_events("s1", &sample_events()).unwrap();
    store.delete("s1").unwrap();
    assert!(matches!(
        store.load("s1").unwrap_err(),
        AppError::NotFound(_)
    ));
    // 再删不报错
    store.delete("s1").unwrap();
}

/// 端到端：跑一个真回合，落库，再从磁盘恢复并续跑第二回合——回合号、历史都续上。
#[tokio::test]
async fn end_to_end_persist_and_resume_across_restart() {
    let tmp = TempDir::new("e2e-resume");

    // 第一次"启动"：建 app，发一条消息
    {
        let model = Arc::new(MockModel::new([
            ModelResponse::calls(vec![ToolCall::with_id(
                "c1",
                "add",
                serde_json::json!({"a":2,"b":3}),
            )]),
            ModelResponse::text("5"),
        ]));
        let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), model)
            .build()
            .unwrap();
        let out = app.send("chat-1", "算 2+3").await.unwrap();
        assert_eq!(out.turn, 1);
        assert!(out.new_events.iter().any(|e| matches!(&e.kind,
            EventKind::ToolResult { output, .. } if output["sum"] == 5.0)));
    }

    // 第二次"启动"（新 app 实例，同 data_dir）：历史应在磁盘上
    {
        let model = Arc::new(MockModel::saying("好的"));
        let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), model)
            .build()
            .unwrap();
        // 会话列表能看到上次的 chat-1
        let sessions = app.list_sessions().unwrap();
        assert!(sessions.iter().any(|m| m.id == "chat-1"));

        let out = app.send("chat-1", "继续").await.unwrap();
        assert_eq!(out.turn, 2, "restored session must continue at turn 2");

        // 全量事件里应同时有第一回合的 add 结果与第二回合的 user 输入
        let all = app.get_events("chat-1").unwrap();
        assert!(all.iter().any(|e| matches!(&e.kind,
            EventKind::ToolResult { output, .. } if output["sum"] == 5.0)));
        assert_eq!(
            all.iter()
                .filter(|e| matches!(e.kind, EventKind::TurnEnded { .. }))
                .count(),
            2
        );
    }
}

/// 会话标题：首条用户消息自动填充；后续回合不覆盖（标题稳定）。
#[tokio::test]
async fn session_title_derived_from_first_message_and_stable() {
    let tmp = TempDir::new("title");
    let model = Arc::new(MockModel::saying("ok"));
    let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), model)
        .build()
        .unwrap();

    app.send("t1", "介绍 Polymer 项目").await.unwrap();
    let m = app
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|m| m.id == "t1")
        .unwrap();
    assert_eq!(m.title.as_deref(), Some("介绍 Polymer 项目"));

    // 第二回合不同文案，标题应保持首条不变
    app.send("t1", "再展开讲讲架构细节和依赖关系以及后续规划安排等等")
        .await
        .unwrap();
    let m2 = app
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|m| m.id == "t1")
        .unwrap();
    assert_eq!(
        m2.title.as_deref(),
        Some("介绍 Polymer 项目"),
        "title must stay stable across turns"
    );
}

/// 超长首条消息标题应截断并加省略号。
#[tokio::test]
async fn long_title_is_truncated() {
    let tmp = TempDir::new("title-long");
    let model = Arc::new(MockModel::saying("ok"));
    let app = DesktopAppBuilder::new(tmp.path(), tmp.path(), model)
        .build()
        .unwrap();
    let long = "这是一条非常非常长的用户指令用来测试标题截断是否正确工作真的很长很长很长很长很长";
    app.send("t2", long).await.unwrap();
    let m = app
        .list_sessions()
        .unwrap()
        .into_iter()
        .find(|m| m.id == "t2")
        .unwrap();
    let title = m.title.unwrap();
    assert!(
        title.ends_with('…'),
        "long title must end with ellipsis: {title}"
    );
    assert!(title.chars().count() <= 25, "title must be truncated");
}

#[test]
fn load_events_window_tails_pages_and_full() {
    let tmp = TempDir::new("store-window");
    let store = FileSessionStore::new(tmp.path()).unwrap();
    let n = 25usize;
    let evs: Vec<SessionEvent> = (0..n)
        .map(|i| SessionEvent {
            seq: (i + 1) as u64,
            ts: chrono::Utc::now(),
            kind: EventKind::UserMessage {
                text: format!("m{i}"),
            },
        })
        .collect();
    store.append_events("s1", &evs).unwrap();
    store
        .put_meta(&SessionMeta {
            id: "s1".into(),
            title: Some("我的任务".into()),
            system: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            event_count: n,
            workspace_id: None,
            plan_mode: false,
        })
        .unwrap();

    // 尾窗口：最近 10 条（start=15，标题一并回传）
    let w = store.load_events_window("s1", Some(10), None).unwrap();
    assert_eq!(w.total, 25);
    assert_eq!(w.start, 15);
    assert_eq!(w.events.len(), 10);
    assert_eq!(w.title.as_deref(), Some("我的任务"));
    assert!(matches!(&w.events[0].kind, EventKind::UserMessage { text } if text == "m15"));

    // 向前翻页：before=15 → [5,15)
    let w2 = store.load_events_window("s1", Some(10), Some(w.start)).unwrap();
    assert_eq!(w2.start, 5);
    assert_eq!(w2.events.len(), 10);
    assert!(matches!(&w2.events[0].kind, EventKind::UserMessage { text } if text == "m5"));
    assert!(matches!(&w2.events[9].kind, EventKind::UserMessage { text } if text == "m14"));

    // 全量（limit=None）与超额 limit：均 start=0、返回全部
    assert_eq!(store.load_events_window("s1", None, None).unwrap().events.len(), 25);
    let big = store.load_events_window("s1", Some(1000), None).unwrap();
    assert_eq!(big.start, 0);
    assert_eq!(big.events.len(), 25);
}
