//! 登录会话落盘（auth.json）读写回归：损坏容忍、字段保真、路径缺失不 panic。

use std::path::PathBuf;

use cmx_agent_app::{AuthSessionFile, read_auth_session, write_auth_session};

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
}
impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn sample() -> AuthSessionFile {
    AuthSessionFile {
        user_id: "75033899799601497".into(),
        username: "9999".into(),
        nickname: Some("工号9999".into()),
        roles: vec!["user".into()],
        must_change_password: true,
        access_token: "acc".into(),
        refresh_token: "ref".into(),
        access_expires_at: 1_790_755_637,
        refresh_expires_at: 1_789_560_437,
        saved_at: 1_788_955_637,
    }
}

#[test]
fn session_file_roundtrip_preserves_fields() {
    let tmp = TempDir::new("sess-roundtrip");
    let path = tmp.0.join("auth.json");
    write_auth_session(&path, &sample());
    let loaded = read_auth_session(&path).expect("should read back");
    assert_eq!(loaded.user_id, "75033899799601497");
    assert_eq!(loaded.username, "9999");
    assert_eq!(loaded.nickname.as_deref(), Some("工号9999"));
    assert_eq!(loaded.roles, vec!["user".to_string()]);
    assert!(loaded.must_change_password);
    assert_eq!(loaded.access_token, "acc");
    assert_eq!(loaded.refresh_token, "ref");
    assert_eq!(loaded.refresh_expires_at, 1_789_560_437);
}

#[test]
fn missing_or_corrupt_session_reads_none() {
    let tmp = TempDir::new("sess-bad");
    let path = tmp.0.join("auth.json");
    assert!(read_auth_session(&path).is_none(), "缺失文件 → None");
    std::fs::write(&path, "{not json").unwrap();
    assert!(read_auth_session(&path).is_none(), "损坏文件 → None");
}

#[test]
fn restore_without_persist_path_is_false() {
    // 未启用登录门/未装配落盘路径（如 CLI、测试构建）：回放直接 false，不触碰文件系统。
    let tmp = TempDir::new("sess-restore-none");
    let model = std::sync::Arc::new(cmx_agent_core::MockModel::saying("hi"));
    let app = cmx_agent_app::DesktopAppBuilder::new(tmp.0.join("w"), tmp.0.clone(), model)
        .build()
        .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let restored = rt.block_on(app.try_restore_session());
    assert!(!restored);
}
