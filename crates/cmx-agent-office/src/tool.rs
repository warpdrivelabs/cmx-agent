//! 办公文档工具：`doc_read`（按扩展名读 Excel/PDF/Word/PPT/文本）+ `xlsx_write`（写 Excel）。
//!
//! 绝对路径直接使用；相对路径按首个工作根解析，无工作根时使用当前目录。重解析依赖隔离在本 crate。

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use cmx_agent_core::tool::GuardHints;
use cmx_agent_core::{Tool, ToolCtx, ToolError, ToolResult, ToolSpec};
use serde_json::{Value, json};

use crate::{excel, text};

fn resolve(path: &str, ctx: &ToolCtx) -> Result<PathBuf, String> {
    let raw = PathBuf::from(path);
    if raw.is_absolute() {
        return Ok(raw);
    }
    let root = match ctx.workspace_roots.first() {
        Some(root) => root.clone(),
        None => std::env::current_dir().map_err(|e| format!("current_dir: {e}"))?,
    };
    Ok(root.join(raw))
}
fn ext_of(p: &Path) -> String {
    p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

pub struct DocReadTool;

#[async_trait]
impl Tool for DocReadTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "doc_read",
            "读办公文档：Excel(xlsx/xls/ods)→表格数据 · PDF→文本 · Word(docx)→正文 · PPT(pptx)→各页文本 · txt/md/csv→文本",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "文档路径（绝对路径或相对工作根/当前目录）" },
                "max_rows": { "type": "integer", "default": 200, "description": "Excel 每 sheet 最多返回行数" },
                "max_chars": { "type": "integer", "default": 20000, "description": "文本类最多返回字符数" }
            },
            "required": ["path"]
        }))
        .guard(GuardHints { requires_auth: Some("fs:read".into()), idempotent: true, ..Default::default() })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("doc_read: 'path' is required"));
        };
        let abs = match resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("doc_read: {e}"))),
        };
        if !abs.exists() {
            return Ok(ToolResult::err(format!("doc_read: 文件不存在 {}", abs.display())));
        }
        let max_rows = input.get("max_rows").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
        let max_chars = input.get("max_chars").and_then(|v| v.as_u64()).unwrap_or(20000) as usize;
        let ext = ext_of(&abs);

        let result = match ext.as_str() {
            "xlsx" | "xls" | "xlsm" | "ods" => match excel::read_excel(&abs, max_rows, 64) {
                Ok(v) => json!({ "kind": "excel", "path": abs.display().to_string(), "sheets": v["sheets"] }),
                Err(e) => return Ok(ToolResult::err(format!("doc_read: {e}"))),
            },
            "pdf" => match text::read_pdf(&abs) {
                Ok(t) => json!({ "kind": "pdf", "text": truncate(&t, max_chars), "chars": t.chars().count() }),
                Err(e) => return Ok(ToolResult::err(format!("doc_read: {e}"))),
            },
            "docx" => match text::read_docx(&abs) {
                Ok(t) => json!({ "kind": "docx", "text": truncate(&t, max_chars), "chars": t.chars().count() }),
                Err(e) => return Ok(ToolResult::err(format!("doc_read: {e}"))),
            },
            "pptx" => match text::read_pptx(&abs) {
                Ok(slides) => json!({
                    "kind": "pptx",
                    "slides": slides.iter().map(|(i,t)| json!({"slide": i, "text": truncate(t, 4000)})).collect::<Vec<_>>(),
                }),
                Err(e) => return Ok(ToolResult::err(format!("doc_read: {e}"))),
            },
            "txt" | "md" | "csv" | "tsv" | "json" | "log" | "" => {
                let t = std::fs::read_to_string(&abs).unwrap_or_default();
                json!({ "kind": "text", "text": truncate(&t, max_chars), "chars": t.chars().count() })
            }
            other => return Ok(ToolResult::err(format!("doc_read: 暂不支持的文档类型 '.{other}'"))),
        };
        Ok(ToolResult::ok(result))
    }
}

pub struct XlsxWriteTool;

#[async_trait]
impl Tool for XlsxWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "xlsx_write",
            "生成 Excel(.xlsx)：给 sheets=[{name, rows:[[单元格,...],...]}]，纯数字字符串写成数字。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "输出（绝对路径或相对工作根/当前目录） .xlsx 路径" },
                "sheets": {
                    "type": "array",
                    "items": { "type": "object", "properties": {
                        "name": {"type":"string"},
                        "rows": {"type":"array","items":{"type":"array"}}
                    }, "required": ["rows"] }
                }
            },
            "required": ["path", "sheets"]
        }))
        .guard(GuardHints { requires_auth: Some("fs:write".into()), idempotent: false, writes: true, write_path_args: vec!["path".into()], ..Default::default() })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("xlsx_write: 'path' is required"));
        };
        let Some(sheets) = input.get("sheets") else {
            return Ok(ToolResult::err("xlsx_write: 'sheets' is required"));
        };
        let abs = match resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("xlsx_write: {e}"))),
        };
        match excel::write_excel(&abs, sheets) {
            Ok(()) => Ok(ToolResult::ok(json!({ "path": abs.display().to_string(), "ok": true }))),
            Err(e) => Ok(ToolResult::err(format!("xlsx_write: {e}"))),
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}\n…（已截断，共 {} 字符）", s.chars().count())
    }
}

/// 生成 PPT(.pptx)：每页 = 标题 + 要点列表（纯 Rust 组包 zip+PresentationML，无外部依赖）。
pub struct PptxWriteTool;

#[async_trait]
impl Tool for PptxWriteTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec::new(
            "pptx_write",
            "生成 PPT(.pptx)：给 slides=[{title, bullets:[要点,...]}]，每页标题+要点列表（标题+正文版式）。",
        )
        .schema(json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "输出（绝对路径或相对工作根/当前目录） .pptx 路径" },
                "slides": {
                    "type": "array",
                    "items": { "type": "object", "properties": {
                        "title": {"type":"string"},
                        "bullets": {"type":"array","items":{"type":"string"}}
                    }, "required": ["title"] }
                }
            },
            "required": ["path", "slides"]
        }))
        .guard(GuardHints { requires_auth: Some("fs:write".into()), idempotent: false, writes: true, write_path_args: vec!["path".into()], ..Default::default() })
    }

    async fn invoke(&self, input: Value, ctx: &ToolCtx<'_>) -> Result<ToolResult, ToolError> {
        let Some(path) = input.get("path").and_then(|v| v.as_str()) else {
            return Ok(ToolResult::err("pptx_write: 'path' is required"));
        };
        let Some(slides) = input.get("slides") else {
            return Ok(ToolResult::err("pptx_write: 'slides' is required"));
        };
        let abs = match resolve(path, ctx) {
            Ok(p) => p,
            Err(e) => return Ok(ToolResult::err(format!("pptx_write: {e}"))),
        };
        let n = slides.as_array().map(|a| a.len()).unwrap_or(0);
        match crate::pptx::write_pptx(&abs, slides) {
            Ok(()) => Ok(ToolResult::ok(json!({
                "path": abs.display().to_string(), "slides": n, "ok": true,
                "note": "标题+要点版式；可用 Office/WPS 打开后进一步美化"
            }))),
            Err(e) => Ok(ToolResult::err(format!("pptx_write: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> (PathBuf, Vec<PathBuf>) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "cmx-office-{}-{}-{}", std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        (root.clone(), vec![root])
    }

    #[tokio::test]
    async fn xlsx_write_then_read_roundtrip() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        // 写
        let w = XlsxWriteTool.invoke(json!({
            "path": "out.xlsx",
            "sheets": [{"name":"销售","rows":[["季度","销售额"],["Q1",120],["Q2",150]]}]
        }), &ctx).await.unwrap();
        assert!(w.ok, "{w:?}");
        assert!(root.join("out.xlsx").exists());
        // 读回
        let r = DocReadTool.invoke(json!({"path":"out.xlsx"}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["kind"], "excel");
        let sheets = r.output["sheets"].as_array().unwrap();
        assert_eq!(sheets[0]["name"], "销售");
        let data = sheets[0]["data"].as_array().unwrap();
        assert_eq!(data[0][0], "季度");
        assert_eq!(data[1][1], "120"); // 120 数字读回为 "120"
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn read_plain_text() {
        let (root, roots) = tmp();
        std::fs::write(root.join("a.md"), "# 标题\n正文").unwrap();
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = DocReadTool.invoke(json!({"path":"a.md"}), &ctx).await.unwrap();
        assert!(r.ok);
        assert_eq!(r.output["kind"], "text");
        assert!(r.output["text"].as_str().unwrap().contains("标题"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn paths_use_first_workspace_or_current_dir() {
        let (root, _) = tmp();
        let roots = vec![root.join("first"), root.join("second")];
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        assert_eq!(resolve("out.xlsx", &ctx).unwrap(), roots[0].join("out.xlsx"));
        let outside = root.join("outside.xlsx");
        assert_eq!(resolve(outside.to_str().unwrap(), &ctx).unwrap(), outside);

        let ctx = ToolCtx { workspace_roots: &[], session_id: "test" };
        assert_eq!(resolve("out.xlsx", &ctx).unwrap(), std::env::current_dir().unwrap().join("out.xlsx"));
        assert_eq!(resolve(outside.to_str().unwrap(), &ctx).unwrap(), outside);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn read_text_outside_workspace() {
        let (root, _) = tmp();
        let workspace = root.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let roots = vec![workspace];
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let outside = root.join("outside.txt");
        std::fs::write(&outside, "根外临时文档").unwrap();
        for path in [outside.clone(), PathBuf::from("../outside.txt")] {
            let r = DocReadTool.invoke(json!({"path":path}), &ctx).await.unwrap();
            assert!(r.ok, "{r:?}");
            assert_eq!(r.output["text"], "根外临时文档");
        }
        let ctx = ToolCtx { workspace_roots: &[], session_id: "test" };
        let r = DocReadTool.invoke(json!({"path":outside}), &ctx).await.unwrap();
        assert!(r.ok, "{r:?}");
        assert_eq!(r.output["text"], "根外临时文档");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn write_and_read_office_files_outside_workspace() {
        let (root, _) = tmp();
        let workspace = root.join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let roots = vec![workspace];
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        // 绝对路径与 .. 都只指向本测试生成的临时文件；读回校验两个写工具。
        for path in [root.join("absolute.xlsx"), PathBuf::from("../relative.xlsx")] {
            let w = XlsxWriteTool.invoke(json!({
                "path":path, "sheets":[{"name":"根外","rows":[["回归",42]]}]
            }), &ctx).await.unwrap();
            assert!(w.ok, "{w:?}");
            let r = DocReadTool.invoke(json!({"path":path}), &ctx).await.unwrap();
            assert!(r.ok, "{r:?}");
            assert_eq!(r.output["sheets"][0]["data"][0][1], "42");
        }
        for path in [root.join("absolute.pptx"), PathBuf::from("../relative.pptx")] {
            let w = PptxWriteTool.invoke(json!({
                "path":path, "slides":[{"title":"根外演示","bullets":["回归要点"]}]
            }), &ctx).await.unwrap();
            assert!(w.ok, "{w:?}");
            let r = DocReadTool.invoke(json!({"path":path}), &ctx).await.unwrap();
            assert!(r.ok, "{r:?}");
            assert_eq!(r.output["kind"], "pptx");
            let text = r.output["slides"][0]["text"].as_str().unwrap();
            assert!(text.contains("根外演示") && text.contains("回归要点"), "{r:?}");
        }
        assert!(root.join("relative.xlsx").exists());
        assert!(root.join("relative.pptx").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn missing_file_errors() {
        let (root, roots) = tmp();
        let ctx = ToolCtx { workspace_roots: &roots, session_id: "test" };
        let r = DocReadTool.invoke(json!({"path":"nope.xlsx"}), &ctx).await.unwrap();
        assert!(!r.ok);
        std::fs::remove_dir_all(&root).ok();
    }
}
