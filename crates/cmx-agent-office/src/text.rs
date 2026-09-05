//! PDF / Word(docx) / PowerPoint(pptx) 文本抽取。
//! - PDF：pdf-extract 抽全文。
//! - docx/pptx：本质是 zip+XML——解压后从 `<w:t>`（Word）/`<a:t>`（PPT）标签抽文本，简单可靠、无需完整 XML 解析。

use std::io::Read;

/// PDF 全文抽取。
pub fn read_pdf(path: &std::path::Path) -> Result<String, String> {
    pdf_extract::extract_text(path).map_err(|e| format!("PDF 抽取失败: {e}"))
}

/// docx 抽取正文（word/document.xml 里的 `<w:t>` 文本，段落按 `</w:p>` 分行）。
pub fn read_docx(path: &std::path::Path) -> Result<String, String> {
    let xml = read_zip_entry(path, "word/document.xml")?;
    // 段落边界 </w:p> → 换行；抽 <w:t ...>text</w:t>
    let mut out = String::new();
    for para in xml.split("</w:p>") {
        let line = extract_tag_text(para, "w:t");
        if !line.trim().is_empty() {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    Ok(out.trim_end().to_string())
}

/// pptx 抽取每张幻灯片文本（ppt/slides/slideN.xml 里的 `<a:t>`）。返回 (slide_index, text)。
pub fn read_pptx(path: &std::path::Path) -> Result<Vec<(usize, String)>, String> {
    let names = zip_entry_names(path)?;
    // 收集 ppt/slides/slideN.xml，按 N 排序
    let mut slides: Vec<(usize, String)> = Vec::new();
    let mut slide_files: Vec<(usize, String)> = names
        .iter()
        .filter_map(|n| {
            let base = n.strip_prefix("ppt/slides/slide")?.strip_suffix(".xml")?;
            base.parse::<usize>().ok().map(|idx| (idx, n.clone()))
        })
        .collect();
    slide_files.sort_by_key(|(i, _)| *i);
    for (idx, file) in slide_files {
        let xml = read_zip_entry(path, &file)?;
        let text = extract_tag_text(&xml, "a:t");
        slides.push((idx, text.trim().to_string()));
    }
    Ok(slides)
}

/// 抽取所有 `<tag ...>inner</tag>` 的 inner 文本并以空格连接。轻量扫描（非完整 XML 解析，足够抽正文）。
fn extract_tag_text(xml: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut out: Vec<String> = Vec::new();
    let mut rest = xml;
    while let Some(op) = rest.find(&open) {
        // 找到开标签结尾 '>'
        let after = &rest[op..];
        let Some(gt) = after.find('>') else { break };
        let content_start = op + gt + 1;
        let Some(cl) = rest[content_start..].find(&close) else { break };
        let text = &rest[content_start..content_start + cl];
        out.push(unescape_xml(text));
        rest = &rest[content_start + cl + close.len()..];
    }
    out.join(" ")
}

fn unescape_xml(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
}

fn read_zip_entry(path: &std::path::Path, entry: &str) -> Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("非法 zip: {e}"))?;
    let mut f = zip.by_name(entry).map_err(|_| format!("缺少 {entry}"))?;
    let mut s = String::new();
    f.read_to_string(&mut s).map_err(|e| format!("读取 {entry} 失败: {e}"))?;
    Ok(s)
}

fn zip_entry_names(path: &std::path::Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("非法 zip: {e}"))?;
    Ok((0..zip.len()).filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_tag_text_basic() {
        let xml = r#"<w:p><w:r><w:t>你好</w:t></w:r><w:r><w:t xml:space="preserve"> 世界</w:t></w:r></w:p>"#;
        assert_eq!(extract_tag_text(xml, "w:t"), "你好  世界");
    }

    #[test]
    fn unescape_works() {
        assert_eq!(unescape_xml("a&amp;b&lt;c"), "a&b<c");
    }
}
