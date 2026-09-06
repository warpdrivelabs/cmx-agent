//! 轻量 HTML→文本 / 标题抽取 / DuckDuckGo 结果解析。手写扫描，不引重解析库（对齐 cmx-agent-office）。

/// ASCII 大小写无关的子串查找（needle 恒为 ASCII 标记；返回字节下标，非 ASCII 内容不影响索引）。
fn find_ci_from(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() || from > h.len() {
        return None;
    }
    let mut i = from;
    while i + n.len() <= h.len() {
        let mut j = 0;
        while j < n.len() && h[i + j].to_ascii_lowercase() == n[j].to_ascii_lowercase() {
            j += 1;
        }
        if j == n.len() {
            return Some(i);
        }
        i += 1;
    }
    None
}
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    find_ci_from(hay, needle, 0)
}

/// 删除 `open`…`close` 之间（含标记）的所有片段（未闭合则删到结尾）。
fn remove_ci(s: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(a) = find_ci_from(s, open, pos) {
        out.push_str(&s[pos..a]);
        match find_ci_from(s, close, a + open.len()) {
            Some(b) => pos = b + close.len(),
            None => {
                pos = s.len();
                break;
            }
        }
    }
    out.push_str(&s[pos..]);
    out
}

/// 大小写无关地把 `tag` 全部替换为 `rep`。
fn replace_ci(s: &str, tag: &str, rep: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pos = 0;
    while let Some(a) = find_ci_from(s, tag, pos) {
        out.push_str(&s[pos..a]);
        out.push_str(rep);
        pos = a + tag.len();
    }
    out.push_str(&s[pos..]);
    out
}

/// 去掉所有 `<...>` 标签。
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut intag = false;
    for c in s.chars() {
        match c {
            '<' => intag = true,
            '>' => intag = false,
            _ => {
                if !intag {
                    out.push(c);
                }
            }
        }
    }
    out
}

/// 解 HTML 实体（常见命名 + 数字 &#NN; / &#xHH;）。
fn decode_entities(s: &str) -> String {
    let named = s
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&mdash;", "—")
        .replace("&ndash;", "–")
        .replace("&hellip;", "…")
        .replace("&middot;", "·")
        .replace("&copy;", "©")
        .replace("&reg;", "®")
        .replace("&amp;", "&"); // 放最后，避免二次解码
    decode_numeric(&named)
}
fn decode_numeric(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if b[i] == b'&' && i + 2 < s.len() && b[i + 1] == b'#' {
            if let Some(semi) = s[i..].find(';') {
                let ent = &s[i + 2..i + semi];
                let code = if ent.starts_with(['x', 'X']) {
                    u32::from_str_radix(&ent[1..], 16).ok()
                } else {
                    ent.parse::<u32>().ok()
                };
                if let Some(cp) = code.and_then(char::from_u32) {
                    out.push(cp);
                    i += semi + 1;
                    continue;
                }
            }
        }
        let ch = s[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// 逐行 trim + 折叠行内空白 + 合并连续空行。
pub fn collapse_ws(s: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    for raw in s.lines() {
        let t = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            if lines.last().map(|l| !l.is_empty()).unwrap_or(false) {
                lines.push(String::new());
            }
        } else {
            lines.push(t);
        }
    }
    while lines.last().map(|l| l.is_empty()).unwrap_or(false) {
        lines.pop();
    }
    lines.join("\n")
}
fn oneline(s: &str) -> String {
    collapse_ws(s).replace('\n', " ")
}

/// 按字符数截断。
pub fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}\n…（已截断，共 {} 字符）", s.chars().count())
    }
}

/// 提取 <title> 文本。
pub fn extract_title(html: &str) -> String {
    if let Some(a) = find_ci(html, "<title") {
        if let Some(gt) = html[a..].find('>') {
            let start = a + gt + 1;
            if let Some(rel) = find_ci_from(html, "</title>", start) {
                return oneline(&decode_entities(&strip_tags(&html[start..rel])));
            }
        }
    }
    String::new()
}

/// HTML → 可读纯文本。
pub fn html_to_text(html: &str) -> String {
    let mut s = remove_ci(html, "<script", "</script>");
    s = remove_ci(&s, "<style", "</style>");
    s = remove_ci(&s, "<!--", "-->");
    s = remove_ci(&s, "<head", "</head>"); // 去头部(meta/link/title 等，正文在 body)
    for tag in [
        "</p>", "</div>", "</li>", "</tr>", "</h1>", "</h2>", "</h3>", "</h4>", "</h5>", "</h6>",
        "</section>", "</article>", "</header>", "</footer>", "</ul>", "</ol>", "</table>",
        "</blockquote>", "<br>", "<br/>", "<br />", "<li>", "<tr>", "<p>",
    ] {
        s = replace_ci(&s, tag, "\n");
    }
    s = strip_tags(&s);
    s = decode_entities(&s);
    collapse_ws(&s)
}

// ── 搜索结果解析（DuckDuckGo HTML/lite 结构）──

fn attr_after(html: &str, from: usize, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let a = find_ci_from(html, &key, from)?;
    let start = a + key.len();
    let end = html[start..].find('"')?;
    Some(html[start..start + end].to_string())
}
fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(x) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(x);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
/// DuckDuckGo 的 href 常是 `//duckduckgo.com/l/?uddg=<编码真实URL>&…` 重定向，解出真实 URL。
fn resolve_ddg_href(href: &str) -> String {
    if let Some(u) = find_ci(href, "uddg=") {
        let start = u + 5;
        let end = href[start..].find('&').map(|e| start + e).unwrap_or(href.len());
        return pct_decode(&href[start..end]);
    }
    if let Some(stripped) = href.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        href.to_string()
    }
}

/// 解析搜索结果页 → (标题, 真实URL, 摘要) 列表。兼容 DDG lite(`result-link`/`result-snippet`)
/// 与 DDG html(`result__a`/`result__snippet`) 两种标记；按文档位置把摘要配到最近的结果。
pub fn parse_results(html: &str) -> Vec<(String, String, String)> {
    let links = collect_links(html);
    let snips = collect_snips(html);
    let mut out = Vec::new();
    for (i, (pos, title, url)) in links.iter().enumerate() {
        let next = links.get(i + 1).map(|l| l.0).unwrap_or(usize::MAX);
        let snippet = snips
            .iter()
            .find(|(sp, _)| *sp > *pos && *sp < next)
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        out.push((title.clone(), url.clone(), snippet));
    }
    out
}

fn collect_links(html: &str) -> Vec<(usize, String, String)> {
    let mut v: Vec<(usize, String, String)> = Vec::new();
    for marker in ["class=\"result-link\"", "class=\"result__a\""] {
        let mut pos = 0;
        while let Some(a) = find_ci_from(html, marker, pos) {
            let astart = html[..a].rfind('<').unwrap_or(a);
            let href = attr_after(html, astart, "href").unwrap_or_default();
            let gt = find_ci_from(html, ">", a).unwrap_or(a);
            let end = find_ci_from(html, "</a>", gt).unwrap_or(gt);
            let title = oneline(&decode_entities(&strip_tags(&html[gt + 1..end])));
            let url = resolve_ddg_href(&href);
            if !url.is_empty() && !title.is_empty() {
                v.push((astart, title, url));
            }
            pos = end + 4;
        }
    }
    v.sort_by_key(|x| x.0);
    v
}

fn collect_snips(html: &str) -> Vec<(usize, String)> {
    let mut v: Vec<(usize, String)> = Vec::new();
    for marker in ["class=\"result-snippet\"", "class=\"result__snippet\""] {
        let mut pos = 0;
        while let Some(a) = find_ci_from(html, marker, pos) {
            let gt = find_ci_from(html, ">", a).unwrap_or(a);
            let end = find_ci_from(html, "</", gt).unwrap_or(gt);
            v.push((a, oneline(&decode_entities(&strip_tags(&html[gt + 1..end])))));
            pos = end + 2;
        }
    }
    v.sort_by_key(|x| x.0);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_to_text_strips_script_style_tags() {
        let h = "<html><head><title>标题</title><style>x{}</style></head><body>\
                 <script>alert('x')</script><p>第一段 &amp; 内容</p><div>第二段</div></body></html>";
        assert_eq!(extract_title(h), "标题");
        let t = html_to_text(h);
        assert!(t.contains("第一段 & 内容"), "{t}");
        assert!(t.contains("第二段"));
        assert!(!t.contains("alert"), "脚本应被剔除：{t}");
        assert!(!t.contains("标题"), "head/title 不进正文：{t}");
    }

    #[test]
    fn decode_numeric_entities() {
        assert_eq!(html_to_text("<p>A&#66;C&#x44;</p>"), "ABCD");
    }

    #[test]
    fn parse_ddg_results_with_uddg_redirect() {
        let h = r#"<div>
          <a class="result-link" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fa&rut=1">结果一标题</a>
          <td class="result-snippet">这是第一条摘要。</td>
          <a class="result-link" href="https://direct.example.org/b">结果二</a>
          <td class="result-snippet">第二条摘要</td>
        </div>"#;
        let r = parse_results(h);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].0, "结果一标题");
        assert_eq!(r[0].1, "https://example.com/a");
        assert_eq!(r[0].2, "这是第一条摘要。");
        assert_eq!(r[1].1, "https://direct.example.org/b");
    }

    #[test]
    fn parse_ddg_html_variant() {
        // DDG html 主站用 result__a / result__snippet
        let h = r#"<div class="result">
          <a class="result__a" href="https://rustlang.org/">Rust 语言</a>
          <a class="result__snippet" href="x">系统级编程语言</a>
        </div>"#;
        let r = parse_results(h);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, "Rust 语言");
        assert_eq!(r[0].1, "https://rustlang.org/");
        assert_eq!(r[0].2, "系统级编程语言");
    }
}
