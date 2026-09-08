//! PPT(.pptx) 生成（U7 补）：纯 Rust 组包（zip + PresentationML），无外部进程/依赖。
//!
//! 结构为最小合规集：presentation + 每页 slide（标题占位 + 要点正文占位）+ 单 slideMaster/
//! slideLayout/theme。Office/WPS 打开不需要「修复」；只做「标题 + 要点列表」这一种版式，
//! 花哨排版由用户后续在 Office 里美化（智能体先把内容结构化落地）。

use std::io::Write;
use std::path::Path;

use serde_json::Value;

/// XML 文本转义（& < > " '）。
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            o => out.push(o),
        }
    }
    out
}

const NS: &str = "xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" \
xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
xmlns:p=\"http://schemas.openxmlformats.org/presentationml/2006/main\"";

/// 一页幻灯片：标题 + 要点列表（空要点 → 只有标题的章节页）。
struct Slide<'a> {
    title: &'a str,
    bullets: Vec<&'a str>,
}

/// 从工具入参解析 slides（宽松：bullets 可缺省/含非字符串项则跳过）。
fn parse_slides(v: &Value) -> Result<Vec<Slide<'_>>, String> {
    let arr = v.as_array().ok_or("slides 必须是数组 [{title, bullets}]")?;
    if arr.is_empty() {
        return Err("slides 不能为空（至少一页）".into());
    }
    if arr.len() > 100 {
        return Err("slides 最多 100 页".into());
    }
    arr.iter()
        .map(|s| {
            let title = s.get("title").and_then(|t| t.as_str()).unwrap_or("");
            let bullets = s
                .get("bullets")
                .and_then(|b| b.as_array())
                .map(|a| a.iter().filter_map(|x| x.as_str()).collect())
                .unwrap_or_default();
            Ok(Slide { title, bullets })
        })
        .collect()
}

fn slide_xml(s: &Slide) -> String {
    // 16:9（EMU）：标题上半区，要点正文占下半区。
    let mut paras = String::new();
    for b in &s.bullets {
        paras.push_str(&format!(
            "<a:p><a:pPr marL=\"342900\" indent=\"-342900\"><a:buFont typeface=\"Arial\" panose=\"020B0604020202020204\" pitchFamily=\"34\" charset=\"0\"/><a:buChar char=\"•\"/></a:pPr><a:r><a:rPr lang=\"zh-CN\" sz=\"1600\"/><a:t>{}</a:t></a:r></a:p>",
            esc(b)
        ));
    }
    if paras.is_empty() {
        paras = "<a:p><a:endParaRPr lang=\"zh-CN\"/></a:p>".into();
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:sld {NS}><p:cSld><p:spTree>\
<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>\
<p:sp><p:nvSpPr><p:cNvPr id=\"2\" name=\"Title\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>\
<p:nvPr><p:ph type=\"title\"/></p:nvPr></p:nvSpPr>\
<p:spPr><a:xfrm><a:off x=\"838200\" y=\"621615\"/><a:ext cx=\"10515600\" cy=\"1325563\"/></a:xfrm></p:spPr>\
<p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang=\"zh-CN\" sz=\"3200\" b=\"1\"/><a:t>{title}</a:t></a:r></a:p></p:txBody></p:sp>\
<p:sp><p:nvSpPr><p:cNvPr id=\"3\" name=\"Content\"/><p:cNvSpPr><a:spLocks noGrp=\"1\"/></p:cNvSpPr>\
<p:nvPr><p:ph type=\"body\" idx=\"1\"/></p:nvPr></p:nvSpPr>\
<p:spPr><a:xfrm><a:off x=\"838200\" y=\"2139885\"/><a:ext cx=\"10515600\" cy=\"4191000\"/></a:xfrm></p:spPr>\
<p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody></p:sp>\
</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>",
        title = esc(s.title),
    )
}

/// 生成 .pptx 到 `path`。`slides_v` = [{title, bullets:[...]}]。
pub fn write_pptx(path: &Path, slides_v: &Value) -> Result<(), String> {
    let slides = parse_slides(slides_v)?;
    let file = std::fs::File::create(path).map_err(|e| format!("创建文件失败: {e}"))?;
    let mut zip = zip::ZipWriter::new(std::io::BufWriter::new(file));
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();

    let put = |zip: &mut zip::ZipWriter<std::io::BufWriter<std::fs::File>>,
               name: &str,
               content: &str|
     -> Result<(), String> {
        zip.start_file(name, opts).map_err(|e| format!("zip 写 {name} 失败: {e}"))?;
        zip.write_all(content.as_bytes()).map_err(|e| format!("zip 写 {name} 失败: {e}"))
    };

    // —— [Content_Types].xml ——
    let mut ct = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\
<Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\
<Default Extension=\"xml\" ContentType=\"application/xml\"/>\
<Override PartName=\"/ppt/presentation.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml\"/>\
<Override PartName=\"/ppt/slideMasters/slideMaster1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml\"/>\
<Override PartName=\"/ppt/slideLayouts/slideLayout1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml\"/>\
<Override PartName=\"/ppt/theme/theme1.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.theme+xml\"/>",
    );
    for i in 1..=slides.len() {
        ct.push_str(&format!(
            "<Override PartName=\"/ppt/slides/slide{i}.xml\" ContentType=\"application/vnd.openxmlformats-officedocument.presentationml.slide+xml\"/>"
        ));
    }
    ct.push_str("</Types>");
    put(&mut zip, "[Content_Types].xml", &ct)?;

    // —— 包级关系 ——
    put(
        &mut zip,
        "_rels/.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument\" Target=\"ppt/presentation.xml\"/>\
</Relationships>",
    )?;

    // —— presentation.xml（页清单）——
    let mut sld_ids = String::new();
    let mut sld_rels = String::new();
    for (i, _) in slides.iter().enumerate() {
        let rid = format!("rId{}", i + 2);
        sld_ids.push_str(&format!("<p:sldId id=\"{}\" r:id=\"{rid}\"/>", 256 + i));
        sld_rels.push_str(&format!(
            "<Relationship Id=\"{rid}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide\" Target=\"slides/slide{}.xml\"/>",
            i + 1
        ));
    }
    put(
        &mut zip,
        "ppt/presentation.xml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:presentation {NS}>\
<p:sldMasterIdLst><p:sldMasterId id=\"2147483648\" r:id=\"rId1\"/></p:sldMasterIdLst>\
<p:sldIdLst>{sld_ids}</p:sldIdLst>\
<p:sldSz cx=\"12192000\" cy=\"6858000\"/><p:notesSz cx=\"6858000\" cy=\"9144000\"/>\
</p:presentation>"
        ),
    )?;
    put(
        &mut zip,
        "ppt/_rels/presentation.xml.rels",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"slideMasters/slideMaster1.xml\"/>\
{sld_rels}\
<Relationship Id=\"rId{}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"theme/theme1.xml\"/>\
</Relationships>",
            slides.len() + 2
        ),
    )?;

    // —— 母版 / 版式 / 主题（单套，最小组件）——
    put(
        &mut zip,
        "ppt/slideMasters/slideMaster1.xml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:sldMaster {NS}>\
<p:cSld><p:spTree>\
<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>\
</p:spTree></p:cSld>\
<p:clrMap bg1=\"lt1\" tx1=\"dk1\" bg2=\"lt2\" tx2=\"dk2\" accent1=\"accent1\" accent2=\"accent2\" accent3=\"accent3\" accent4=\"accent4\" accent5=\"accent5\" accent6=\"accent6\" hlink=\"hlink\" folHlink=\"folHlink\"/>\
<p:sldLayoutIdLst><p:sldLayoutId id=\"2147483649\" r:id=\"rId1\"/></p:sldLayoutIdLst>\
</p:sldMaster>"
        ),
    )?;
    put(
        &mut zip,
        "ppt/slideMasters/_rels/slideMaster1.xml.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>\
<Relationship Id=\"rId2\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme\" Target=\"../theme/theme1.xml\"/>\
</Relationships>",
    )?;
    put(
        &mut zip,
        "ppt/slideLayouts/slideLayout1.xml",
        &format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<p:sldLayout {NS} type=\"obj\" preserve=\"1\">\
<p:cSld name=\"标题和内容\"><p:spTree>\
<p:nvGrpSpPr><p:cNvPr id=\"1\" name=\"\"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>\
</p:spTree></p:cSld>\
<p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr>\
</p:sldLayout>"
        ),
    )?;
    put(
        &mut zip,
        "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster\" Target=\"../slideMasters/slideMaster1.xml\"/>\
</Relationships>",
    )?;
    put(&mut zip, "ppt/theme/theme1.xml", THEME_XML)?;

    // —— 各页 ——
    for (i, s) in slides.iter().enumerate() {
        put(&mut zip, &format!("ppt/slides/slide{}.xml", i + 1), &slide_xml(s))?;
        put(
            &mut zip,
            &format!("ppt/slides/_rels/slide{}.xml.rels", i + 1),
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
<Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout\" Target=\"../slideLayouts/slideLayout1.xml\"/>\
</Relationships>",
        )?;
    }

    zip.finish().map_err(|e| format!("zip 收尾失败: {e}"))?;
    Ok(())
}

/// 精简但合规的主题（12 色 + 双字体 + fmtScheme 各 3 档）。
const THEME_XML: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
<a:theme xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\" name=\"cmx\">\
<a:themeElements>\
<a:clrScheme name=\"cmx\">\
<a:dk1><a:sysClr val=\"windowText\" lastClr=\"000000\"/></a:dk1>\
<a:lt1><a:sysClr val=\"window\" lastClr=\"FFFFFF\"/></a:lt1>\
<a:dk2><a:srgbClr val=\"44546A\"/></a:dk2>\
<a:lt2><a:srgbClr val=\"E7E6E6\"/></a:lt2>\
<a:accent1><a:srgbClr val=\"4472C4\"/></a:accent1>\
<a:accent2><a:srgbClr val=\"ED7D31\"/></a:accent2>\
<a:accent3><a:srgbClr val=\"A5A5A5\"/></a:accent3>\
<a:accent4><a:srgbClr val=\"FFC000\"/></a:accent4>\
<a:accent5><a:srgbClr val=\"5B9BD5\"/></a:accent5>\
<a:accent6><a:srgbClr val=\"70AD47\"/></a:accent6>\
<a:hlink><a:srgbClr val=\"0563C1\"/></a:hlink>\
<a:folHlink><a:srgbClr val=\"954F72\"/></a:folHlink>\
</a:clrScheme>\
<a:fontScheme name=\"cmx\">\
<a:majorFont><a:latin typeface=\"Calibri Light\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:majorFont>\
<a:minorFont><a:latin typeface=\"Calibri\"/><a:ea typeface=\"\"/><a:cs typeface=\"\"/></a:minorFont>\
</a:fontScheme>\
<a:fmtScheme name=\"cmx\">\
<a:fillStyleLst>\
<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>\
<a:solidFill><a:schemeClr val=\"phClr\"><a:tint val=\"60000\"/></a:schemeClr></a:solidFill>\
<a:solidFill><a:schemeClr val=\"phClr\"><a:shade val=\"80000\"/></a:schemeClr></a:solidFill>\
</a:fillStyleLst>\
<a:lnStyleLst>\
<a:ln w=\"6350\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>\
<a:ln w=\"12700\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>\
<a:ln w=\"19050\"><a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill></a:ln>\
</a:lnStyleLst>\
<a:effectStyleLst>\
<a:effectStyle><a:effectLst/></a:effectStyle>\
<a:effectStyle><a:effectLst/></a:effectStyle>\
<a:effectStyle><a:effectLst/></a:effectStyle>\
</a:effectStyleLst>\
<a:bgFillStyleLst>\
<a:solidFill><a:schemeClr val=\"phClr\"/></a:solidFill>\
<a:solidFill><a:schemeClr val=\"phClr\"><a:tint val=\"95000\"/></a:schemeClr></a:solidFill>\
<a:solidFill><a:schemeClr val=\"phClr\"><a:shade val=\"90000\"/></a:schemeClr></a:solidFill>\
</a:bgFillStyleLst>\
</a:fmtScheme>\
</a:themeElements>\
</a:theme>";

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("cmx-pptx-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.join("demo.pptx")
    }

    /// 生成 → 重新解包断言：条目齐全、页数正确、标题/要点入库、XML 转义生效。
    #[test]
    fn writes_valid_package_with_slides() {
        let path = tmp_path("ok");
        let slides = serde_json::json!([
            {"title": "季度汇报", "bullets": ["收入 <增长> 20%", "成本下降"]},
            {"title": "下一步", "bullets": ["Q4 扩量", "线下&线上推广"]},
        ]);
        write_pptx(&path, &slides).unwrap();

        let f = std::fs::File::open(&path).unwrap();
        let mut z = zip::ZipArchive::new(f).unwrap();
        for name in [
            "[Content_Types].xml",
            "_rels/.rels",
            "ppt/presentation.xml",
            "ppt/_rels/presentation.xml.rels",
            "ppt/slideMasters/slideMaster1.xml",
            "ppt/slideMasters/_rels/slideMaster1.xml.rels",
            "ppt/slideLayouts/slideLayout1.xml",
            "ppt/slideLayouts/_rels/slideLayout1.xml.rels",
            "ppt/theme/theme1.xml",
            "ppt/slides/slide1.xml",
            "ppt/slides/slide2.xml",
            "ppt/slides/_rels/slide2.xml.rels",
        ] {
            assert!(z.by_name(name).is_ok(), "缺条目 {name}");
        }
        assert!(z.by_name("ppt/slides/slide3.xml").is_err(), "不应有第 3 页");

        let s1 = {
            let mut f = z.by_name("ppt/slides/slide1.xml").unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut f, &mut s).unwrap();
            s
        };
        assert!(s1.contains("季度汇报"));
        assert!(s1.contains("&lt;增长&gt;"), "尖括号应转义");
        let s2 = {
            let mut f = z.by_name("ppt/slides/slide2.xml").unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut f, &mut s).unwrap();
            s
        };
        assert!(s2.contains("线下&amp;线上推广"), "& 应转义");
        assert!(s2.contains("下一步"));

        let pres = {
            let mut f = z.by_name("ppt/presentation.xml").unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(&mut f, &mut s).unwrap();
            s
        };
        assert_eq!(pres.matches("<p:sldId ").count(), 2, "presentation 应登记 2 页");
        std::fs::remove_dir_all(path.parent().unwrap()).ok();
    }

    #[test]
    fn empty_slides_rejected() {
        let path = tmp_path("empty");
        let err = write_pptx(&path, &serde_json::json!([])).unwrap_err();
        assert!(err.contains("至少一页"));
    }
}
