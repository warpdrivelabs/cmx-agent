//! 普通工作目录解析：绝对路径原样接受，相对路径基于第一个工作根，无根则使用进程 cwd。

use std::path::PathBuf;

use cmx_agent_core::ToolCtx;

/// 默认工作目录，不限制工具可访问的路径。
pub fn working_dir(ctx: &ToolCtx<'_>) -> Result<PathBuf, String> {
    match ctx.workspace_roots.first() {
        Some(root) => Ok(root.clone()),
        None => std::env::current_dir().map_err(|e| format!("无法解析当前工作目录: {e}")),
    }
}

/// 保留 `..` 和符号链接语义，由文件系统解释，不做路径围栏或规范化。
pub fn resolve(path: &str, ctx: &ToolCtx<'_>) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path);
    if raw.is_absolute() {
        Ok(raw)
    } else {
        Ok(working_dir(ctx)?.join(raw))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths_use_first_workspace_root() {
        let base = crate::testutil::unique_dir("cmx-paths");
        let roots = vec![base.join("first"), base.join("second")];
        let ctx = ToolCtx {
            workspace_roots: &roots,
            session_id: "test",
        };
        assert_eq!(
            resolve("sub/new.txt", &ctx).unwrap(),
            roots[0].join("sub/new.txt")
        );
        assert_eq!(
            resolve("../outside.txt", &ctx).unwrap(),
            roots[0].join("../outside.txt")
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn absolute_paths_do_not_require_roots_or_existing_files() {
        let base = crate::testutil::unique_dir("cmx-absolute");
        let ctx = ToolCtx {
            workspace_roots: &[],
            session_id: "test",
        };
        let target = base.join("new.txt");
        assert_eq!(resolve(target.to_str().unwrap(), &ctx).unwrap(), target);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn missing_roots_use_current_dir_without_changing_it() {
        let ctx = ToolCtx {
            workspace_roots: &[],
            session_id: "test",
        };
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(working_dir(&ctx).unwrap(), cwd);
        assert_eq!(
            resolve("relative.txt", &ctx).unwrap(),
            cwd.join("relative.txt")
        );
    }
}
