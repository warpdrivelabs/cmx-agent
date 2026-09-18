//! 真技能体系（姊妹方案 §2.1）：`<数据根>/skills/<目录名>/SKILL.md`。
//!
//! frontmatter 仅 `name`/`description` 两键，手写解析不引新依赖（宽松容错：缺 frontmatter
//! 时 name 回落目录名、description 空；非法 name 的目录**不静默吞**——条目带 `invalid`
//! 提示浮出）。加载不做文件监听：`ListSlash`/注入同一次惰性扫描，本地目录成本可忽略。
//! 参考形态：opencode `skill/index.ts`（SKILL.md + frontmatter 是两家共识）、codex `ext/skills`。

use std::path::{Path, PathBuf};

/// 一条技能条目（`ListSlash` 与注入共用）。
#[derive(Debug, Clone)]
pub struct SkillEntry {
    /// 斜杠名（frontmatter `name`，缺省回落目录名）。
    pub name: String,
    pub description: String,
    /// 技能目录（注入消息的「基准目录」尾注用——正文相对路径以该目录解析）。
    pub dir: PathBuf,
    /// SKILL.md 正文（frontmatter 之外；注入时整篇给出）。
    pub body: String,
    /// 非空 = 该条目有问题（名称不合规/与命令或先到技能重名），只提示不执行。
    pub invalid: Option<String>,
}

/// 技能名规范（与目录名同规）。
fn valid_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 64
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
}

/// 数据根下的技能目录。
pub fn skills_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("skills")
}

/// 首次创建目录时放 `README.txt` 说明格式 + 一份示例技能（保证「看得到、点得动」，用户可删）。
pub fn ensure_initialized(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let readme = dir.join("README.txt");
    if !readme.exists() {
        std::fs::write(
            &readme,
            "每个子目录放一个 SKILL.md 即是一个技能（/ 菜单的「技能」组）。\n\
             frontmatter 两键可选：name（斜杠名，缺省用目录名，仅小写字母/数字/_/-）、\n\
             description（菜单与模型技能清单里的说明）。正文 = 被调用时注入给模型的指令。\n",
        )?;
    }
    let demo = dir.join("meeting-notes");
    if !demo.exists() {
        std::fs::create_dir_all(&demo)?;
        std::fs::write(
            demo.join("SKILL.md"),
            "---\nname: meeting-notes\ndescription: 把会议录音转写或速记整理成结构化纪要（示例技能，可删除）\n---\n\
             把用户提供的会议转写/速记整理成结构化纪要，严格输出以下结构：\n\n\
             ## 会议主题\n## 参会人\n## 关键结论\n- （逐条列出，保留精确数字/日期/金额）\n\
             ## 待办事项\n- [ ] 事项 —— 负责人 · 截止时间\n\
             ## 遗留问题\n\n\
             规则：信息缺失的小节写「（未提及）」；不确定的转写内容标注「（存疑）」，不要编造。\n\
             输入是长速记时先分段理解再汇总，不要逐句复述原文。\n",
        )?;
    }
    Ok(())
}

/// 扫描技能目录（`ListSlash` 与 send 分发共用同一次扫描——清单与目录恒同源）。
/// 目录不存在时返回空表（调用方通常已先 `ensure_initialized`）。
pub fn scan(dir: &Path) -> Vec<SkillEntry> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    let mut out: Vec<SkillEntry> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for d in dirs {
        let md_path = d.join("SKILL.md");
        let Ok(text) = std::fs::read_to_string(&md_path) else {
            continue; // 无 SKILL.md 的目录不是技能
        };
        let (fm_name, fm_desc, body) = parse_frontmatter(&text);
        let dir_name = d
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let name = fm_name.unwrap_or_else(|| dir_name.clone());
        let mut invalid = None;
        if !valid_name(&name) {
            invalid = Some(format!(
                "名称「{name}」不合规（仅小写字母/数字/_/-），修正 SKILL.md 的 name 或目录名后生效"
            ));
        } else if seen.contains(&name) {
            invalid = Some(format!("与更早的技能「{name}」重名，本条不生效（改 name 区分）"));
        } else {
            seen.push(name.clone());
        }
        out.push(SkillEntry {
            name,
            description: fm_desc.unwrap_or_default(),
            dir: d,
            body,
            invalid,
        });
    }
    out
}

/// 手写 frontmatter 解析：`---` 围栏内 `key: value`，只认 name/description 两键。
/// 无围栏/围栏未闭合 → 整篇当正文（宽松容错）。
fn parse_frontmatter(text: &str) -> (Option<String>, Option<String>, String) {
    let t = text.trim_start_matches('\u{feff}');
    let mut lines = t.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None, t.to_string());
    }
    let mut name = None;
    let mut desc = None;
    let mut rest_start = None;
    for (i, line) in lines.enumerate() {
        if line.trim() == "---" {
            // i 是扣除首行后的序号；正文起点按全文行号 = 首行(1) + 闭合行(1) + i
            rest_start = Some(i + 2);
            break;
        }
        let line = line.trim();
        if let Some(v) = line.strip_prefix("name:") {
            name = Some(v.trim().to_string());
        } else if let Some(v) = line.strip_prefix("description:") {
            desc = Some(v.trim().to_string());
        }
    }
    let body = match rest_start {
        Some(skip) => t.lines().skip(skip).collect::<Vec<_>>().join("\n"),
        None => t.to_string(), // 围栏未闭合：整篇按正文处理，不认字段
    };
    (name, desc, body)
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn parse_frontmatter_full_and_missing() {
        let (n, d, b) = parse_frontmatter("---\nname: foo\ndescription: 说明\n---\n正文");
        assert_eq!(n.as_deref(), Some("foo"));
        assert_eq!(d.as_deref(), Some("说明"));
        assert_eq!(b, "正文");
        let (n2, d2, b2) = parse_frontmatter("直接正文");
        assert!(n2.is_none() && d2.is_none());
        assert_eq!(b2, "直接正文");
        let (_, _, b3) = parse_frontmatter("---\nname: x\n没有闭合");
        assert!(b3.contains("没有闭合"));
    }

    #[test]
    fn scan_seeds_and_flags() {
        let tmp = std::env::temp_dir().join(format!("cmx-skills-test-{}", std::process::id()));
        let dir = tmp.join("skills");
        let _ = std::fs::remove_dir_all(&dir);
        ensure_initialized(&dir).unwrap();
        // 种子示例可用
        let entries = scan(&dir);
        assert!(entries.iter().any(|e| e.name == "meeting-notes" && e.invalid.is_none()));
        // 非法名目录 → invalid 提示而非静默
        let bad = dir.join("Bad Name");
        std::fs::create_dir_all(&bad).unwrap();
        std::fs::write(bad.join("SKILL.md"), "正文").unwrap();
        // 重名 → 后到者标记
        let dup = dir.join("another-meeting");
        std::fs::create_dir_all(&dup).unwrap();
        std::fs::write(dup.join("SKILL.md"), "---\nname: meeting-notes\n---\nx").unwrap();
        let entries = scan(&dir);
        let bad_e = entries.iter().find(|e| e.dir == bad).unwrap();
        assert!(bad_e.invalid.as_deref().unwrap().contains("不合规"));
        // 重名标记：先到者（目录序在前）生效，后到者带「重名」提示——两者恰一有效
        let first = entries.iter().find(|e| e.dir == dup).unwrap();
        let seed = entries
            .iter()
            .find(|e| e.dir.file_name().unwrap() == "meeting-notes")
            .unwrap();
        let (dup_flagged, other) = if first.invalid.is_some() { (first, seed) } else { (seed, first) };
        assert!(dup_flagged.invalid.as_deref().unwrap().contains("重名"));
        assert!(other.invalid.is_none());
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
