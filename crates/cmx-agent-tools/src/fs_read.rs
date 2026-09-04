//! `fs_read` —— 读取一个文本文件的前若干字节。**演示沙箱纵深**：
//! 目标路径必须落在 `ctx.allowed_roots` 之内（工作区隔离），否则拒绝——即便 OS 层还没上 Seatbelt/Landlock，
//! 应用层也先做一道路径围栏（纵深防御）。needs `requires_auth = "fs:read"`（护栏① 演示）。

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub struct FsReadTool;

/// 判断 `target` 是否在某个允许根之下（按规范化后的前缀）。
///
/// 安全要点：对 target 与 root **都**尽力 `canonicalize`（解析符号链接）——否则在 macOS 上
/// `/var`→`/private/var` 这类符号链接会让"字面 target"与"规范化 root"前缀不匹配；更重要的是，
/// canonicalize target 能挡住"根内符号链接指向根外"的逃逸。canonicalize 失败（路径不存在）时
/// 回退到词法归一，仍能挡 `../` 字面逃逸（纵深防御）。
fn within_roots(target: &Path, roots: &[PathBuf]) -> bool {
    let t = std::fs::canonicalize(target).unwrap_or_else(|_| normalize(target));
    roots.iter().any(|r| {
        let r = std::fs::canonicalize(r).unwrap_or_else(|_| normalize(r));
        t.starts_with(&r)
    })
}

/// 词法归一化：移除 `.`，按 `..` 回退，不触碰文件系统。防 `../` 逃逸。
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[async_trait]
impl Tool for FsReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "fs_read",
            "读取 allowed_roots 内某文本文件的前 max_bytes 字节",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "max_bytes": { "type": "integer", "default": 4096 }
            },
            "required": ["path"]
        }))
        .guard(GuardHints {
            requires_auth: Some("fs:read".into()),
            idempotent: true,
            ..Default::default()
        })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let path = match input.get("path").and_then(|v| v.as_str()) {
            Some(p) => PathBuf::from(p),
            None => return Ok(ToolResult::err("fs_read: 'path' is required")),
        };
        let max_bytes = input
            .get("max_bytes")
            .and_then(|v| v.as_u64())
            .unwrap_or(4096) as usize;

        if ctx.allowed_roots.is_empty() {
            return Ok(ToolResult::err(
                "fs_read: no allowed_roots configured (sandbox denies all fs)",
            ));
        }
        if !within_roots(&path, ctx.allowed_roots) {
            return Ok(ToolResult::err(format!(
                "fs_read: path '{}' escapes sandbox allowed_roots",
                path.display()
            )));
        }

        match std::fs::read(&path) {
            Ok(bytes) => {
                let n = bytes.len().min(max_bytes);
                let text = String::from_utf8_lossy(&bytes[..n]).to_string();
                Ok(ToolResult::ok(
                    json!({ "path": path.display().to_string(), "bytes": n, "text": text }),
                ))
            }
            Err(e) => Ok(ToolResult::err(format!("fs_read: {e}"))),
        }
    }
}
