//! 双壳统一数据根（M1：同核多壳 → 同数据根）。
//!
//! Web 壳与 Tauri 壳此前各选各的目录（web 用 `%TEMP%`，Tauri 用 ProjectDirs），
//! 导致 `model.json` / `mcp.json` / `plugins/` / sessions / workspace 都要配两份。
//! 统一后两壳共享一份：模型配置一次、双壳生效；会话历史跨壳可见（同一台机同一用户，合理）。
//!
//! 解析优先级：`CMX_AGENT_DATA_DIR` 环境变量 > `ProjectDirs("com","pansoft","truemate")` 的
//! data_dir（Windows `%APPDATA%\pansoft\truemate\data`，macOS
//! `~/Library/Application Support/com.pansoft.truemate`，Linux `$XDG_DATA_HOME/com.pansoft.truemate`）。
//! 测试 / 多实例隔离：给 env 指一个临时目录即可，两壳行为一致。

use std::ffi::OsString;
use std::path::PathBuf;

/// 双壳统一数据根。两壳 main 各自调用，勿在壳里再自算目录（会再次分叉）。
pub fn shared_data_dir() -> PathBuf {
    data_dir_from(std::env::var_os("CMX_AGENT_DATA_DIR"))
}

/// 纯解析（便于测 env 覆盖语义）：env 非空则赢，否则 ProjectDirs。不建目录（交给调用方初始化）。
fn data_dir_from(env: Option<OsString>) -> PathBuf {
    if let Some(p) = env.filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    directories::ProjectDirs::from("com", "pansoft", "truemate")
        .expect("解析用户数据目录失败（ProjectDirs）")
        .data_dir()
        .to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        assert_eq!(
            data_dir_from(Some(OsString::from("/tmp/cmx-agent-isolated"))),
            PathBuf::from("/tmp/cmx-agent-isolated")
        );
    }

    #[test]
    fn empty_env_falls_back_to_project_dirs() {
        // 空 env 视为未设置，回落 ProjectDirs（三平台都解析出非空绝对路径）。
        let dir = data_dir_from(Some(OsString::new()));
        assert!(dir.is_absolute(), "ProjectDirs 应给出绝对路径，got {dir:?}");
        assert!(dir.components().count() >= 2, "路径过浅: {dir:?}");
        // 应用已改名 TrueMate：默认根不再叫 cmx-agent。
        assert!(
            !dir.to_string_lossy().contains("cmx-agent"),
            "数据根不应再含 cmx-agent: {dir:?}"
        );
    }

    #[test]
    fn none_env_falls_back_to_project_dirs() {
        let dir = data_dir_from(None);
        assert!(dir.is_absolute(), "ProjectDirs 应给出绝对路径，got {dir:?}");
    }
}
