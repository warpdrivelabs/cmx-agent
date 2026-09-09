//! 内嵌 frontend/dist 静态资产表（方案 §10.2 唯一静态服务实现）：
//! 递归扫描 → `include_bytes!` 二进制内嵌 → MIME 映射 → 写 OUT_DIR/ui_assets.rs。
//! dist 缺失直接 panic（fail-fast，不允许编出"运行时 404"的二进制）。
//! sourcemap（.map）不内嵌；发布产物已关闭 sourcemap（§10.2 第 5 条）。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let dist = PathBuf::from(&manifest).join("../../frontend/dist");
    println!("cargo:rerun-if-changed={}", dist.display());

    if !dist.is_dir() {
        panic!(
            "frontend/dist 不存在：请先执行 `cd frontend && npm install && npm run build`（方案 §10.2 fail-fast，不允许编出运行时 404 的二进制）"
        );
    }

    let mut assets: Vec<(String, PathBuf, &'static str)> = Vec::new();
    collect(&dist, &dist, &mut assets);
    if assets.is_empty() {
        panic!("frontend/dist 为空：构建产物异常，请重新执行 `npm run build`");
    }

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR");
    let dest = Path::new(&out_dir).join("ui_assets.rs");
    let mut code = String::from(
        "/// 由 build.rs 自动生成：frontend/dist 内嵌资产表（方案 §10.2）。请勿手改。\n\
         pub struct UiAsset {\n    pub path: &'static str,\n    pub mime: &'static str,\n    pub bytes: &'static [u8],\n}\n\n\
         pub const UI_ASSETS: &[UiAsset] = &[\n",
    );
    for (rel, abs, mime) in &assets {
        code.push_str(&format!(
            "    UiAsset {{ path: {:?}, mime: {:?}, bytes: include_bytes!({:?}) }},\n",
            rel.replace('\\', "/"),
            mime,
            abs
        ));
    }
    code.push_str("];\n");
    fs::write(&dest, code).expect("write ui_assets.rs");
    println!(
        "cargo:warning=cmx-agent-web：已内嵌 {} 个前端资产（frontend/dist）",
        assets.len()
    );
}

/// 递归收集（按文件名排序，保证生成物稳定可 diff）。
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf, &'static str)>) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => panic!("读取 {} 失败：{e}", dir.display()),
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            let rel = path
                .strip_prefix(root)
                .expect("dist 内相对路径")
                .to_string_lossy()
                .replace('\\', "/");
            if rel.ends_with(".map") {
                continue; // sourcemap 不内嵌
            }
            out.push((rel.clone(), path, mime_of(&rel)));
        }
    }
}

/// MIME 映射（§10.2 第 3 条：未知扩展名按 octet-stream）。
fn mime_of(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" | "htm" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "xml" => "application/xml",
        _ => "application/octet-stream",
    }
}
