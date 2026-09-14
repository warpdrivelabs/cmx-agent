//! 沙箱路径围栏（编码面工具共用）。应用层纵深防御：即便 OS 未上 Seatbelt/Landlock，
//! 也先把一切文件操作限制在 `ctx.allowed_roots` 之内。
//!
//! 关键难点：**尚不存在的文件**（fs_write 新建）无法 `canonicalize`；而 macOS 临时目录根常是
//! 符号链接（`/var`→`/private/var`）。`resolve` 用「规范化最长已存在祖先 + 词法拼接剩余段」的方式，
//! 同时正确处理「新文件」与「符号链接根」，并挡住 `../` 逃逸。

use std::path::{Component, Path, PathBuf};

use cmx_agent_core::ToolCtx;

/// 词法归一化：移除 `.`，按 `..` 回退，不触碰文件系统。
pub fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// 规范化「最长已存在祖先」再词法拼接剩余段（解析符号链接 + 支持不存在的路径）。
pub fn canonicalize_partial(p: &Path) -> PathBuf {
    // 从完整路径逐级向上，找到第一个能 canonicalize 的祖先。
    let mut ancestor = p.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(c) = std::fs::canonicalize(&ancestor) {
            let mut out = c;
            for seg in tail.iter().rev() {
                out.push(seg);
            }
            return normalize(&out);
        }
        match ancestor.file_name() {
            Some(name) => {
                tail.push(name.to_os_string());
                if !ancestor.pop() {
                    break;
                }
            }
            None => break,
        }
    }
    normalize(p)
}

/// `target` 是否在某个允许根之下（两侧都解析符号链接）。
pub fn within_roots(target: &Path, roots: &[PathBuf]) -> bool {
    let t = canonicalize_partial(target);
    roots.iter().any(|r| {
        let r = std::fs::canonicalize(r).unwrap_or_else(|_| normalize(r));
        t.starts_with(&r)
    })
}

/// 解析并校验一个（可为相对）路径必须落在沙箱内。相对路径按第一个 root 解释。
/// 返回可直接用于 fs 操作的绝对路径，或人类可读的拒绝原因。
pub fn resolve(path: &str, ctx: &ToolCtx) -> Result<PathBuf, String> {
    if ctx.allowed_roots.is_empty() {
        return Err("no allowed_roots configured (sandbox denies all fs)".into());
    }
    let raw = PathBuf::from(path);
    let abs = if raw.is_absolute() {
        raw
    } else {
        ctx.allowed_roots[0].join(raw)
    };
    if !within_roots(&abs, ctx.allowed_roots) {
        return Err(format!(
            "path '{}' escapes sandbox allowed_roots",
            abs.display()
        ));
    }
    Ok(abs)
}

/// 沙箱工作根（shell cwd / glob 基准）：第一个 allowed_root。
pub fn first_root<'a>(ctx: &ToolCtx<'a>) -> Option<&'a PathBuf> {
    ctx.allowed_roots.first()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cmx_agent_core::guard::SandboxMode;

    #[test]
    fn normalize_strips_dotdot() {
        assert_eq!(normalize(Path::new("a/b/../c")), PathBuf::from("a/c"));
    }

    #[test]
    fn resolve_new_file_under_symlinked_root_ok() {
        // 用系统临时目录（macOS 下常为符号链接根）建一个真实根，验证「新文件」不被误拒。
        let root = crate::testutil::unique_dir("cmx-sbx");
        let roots = vec![root.clone()];
        let ctx = ToolCtx {
             sandbox: SandboxMode::WorkspaceWrite,
             allowed_roots: &roots,
             session_id: "test",
         };
        let p = resolve("sub/new.txt", &ctx).expect("new file within sandbox");
        assert!(p.ends_with("sub/new.txt"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_dotdot_escape_denied() {
        let root = crate::testutil::unique_dir("cmx-sbx2");
        let roots = vec![root.clone()];
        let ctx = ToolCtx {
             sandbox: SandboxMode::WorkspaceWrite,
             allowed_roots: &roots,
             session_id: "test",
         };
        assert!(resolve("../escape.txt", &ctx).is_err());
        std::fs::remove_dir_all(&root).ok();
    }
}
