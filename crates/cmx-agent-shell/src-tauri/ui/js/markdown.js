// ── 轻量 Markdown 渲染（块级 + 行内）：标题/列表/表格/代码块/引用/粗斜体/行内代码/链接 ──
// 安全：所有文本经 esc() 转义后再套标签，模型输出的原始 HTML 一律当文本（避免注入）。
function mdInline(t){
  return esc(t)
    .replace(/`([^`]+)`/g,'<code>$1</code>')
    .replace(/\*\*([^*]+)\*\*/g,'<strong>$1</strong>')
    .replace(/(^|[^*])\*([^*\n]+)\*(?!\*)/g,'$1<em>$2</em>')
    .replace(/~~([^~]+)~~/g,'<del>$1</del>')
    .replace(/\[([^\]]+)\]\(([^)\s]+)\)/g,'<a href="$2" target="_blank" rel="noopener">$1</a>');
}
function renderMarkdown(src){
  if(!src) return "";
  const lines=String(src).replace(/\r\n/g,"\n").split("\n");
  const out=[]; let i=0;
  const isSpecial=(l)=> /^(#{1,6}\s|```|>\s?|\s*[-*+]\s|\s*\d+\.\s|(-{3,}|\*{3,}|_{3,})\s*$)/.test(l);
  while(i<lines.length){
    let line=lines[i];
    if(/^```/.test(line)){                                   // 代码块
      const buf=[]; i++;
      while(i<lines.length && !/^```/.test(lines[i])){ buf.push(lines[i]); i++; }
      i++; out.push('<pre class="md-pre"><code>'+esc(buf.join("\n"))+'</code></pre>'); continue;
    }
    const h=line.match(/^(#{1,6})\s+(.*)$/);                 // 标题
    if(h){ const n=h[1].length; out.push(`<h${n} class="md-h">`+mdInline(h[2])+`</h${n}>`); i++; continue; }
    if(/^(-{3,}|\*{3,}|_{3,})\s*$/.test(line)){ out.push('<hr class="md-hr">'); i++; continue; }  // 分隔线
    // 表格：本行含 | 且下一行是 |---| 分隔
    if(line.includes("|") && i+1<lines.length && /^\s*\|?[\s:|-]+\|?\s*$/.test(lines[i+1]) && lines[i+1].includes("-")){
      const row=(l)=> l.replace(/^\s*\|/,"").replace(/\|\s*$/,"").split("|").map(c=>c.trim());
      const heads=row(line); i+=2; const body=[];
      while(i<lines.length && lines[i].includes("|") && lines[i].trim()!==""){ body.push(row(lines[i])); i++; }
      let tb='<table class="md-table"><thead><tr>'+heads.map(c=>'<th>'+mdInline(c)+'</th>').join("")+'</tr></thead><tbody>';
      for(const r of body){ tb+='<tr>'+r.map(c=>'<td>'+mdInline(c)+'</td>').join("")+'</tr>'; }
      out.push(tb+'</tbody></table>'); continue;
    }
    if(/^>\s?/.test(line)){                                  // 引用
      const buf=[]; while(i<lines.length && /^>\s?/.test(lines[i])){ buf.push(lines[i].replace(/^>\s?/,"")); i++; }
      out.push('<blockquote class="md-quote">'+buf.map(mdInline).join("<br>")+'</blockquote>'); continue;
    }
    if(/^\s*[-*+]\s+/.test(line)){                           // 无序列表
      const buf=[]; while(i<lines.length && /^\s*[-*+]\s+/.test(lines[i])){ buf.push('<li>'+mdInline(lines[i].replace(/^\s*[-*+]\s+/,""))+'</li>'); i++; }
      out.push('<ul class="md-ul">'+buf.join("")+'</ul>'); continue;
    }
    if(/^\s*\d+\.\s+/.test(line)){                           // 有序列表
      const buf=[]; while(i<lines.length && /^\s*\d+\.\s+/.test(lines[i])){ buf.push('<li>'+mdInline(lines[i].replace(/^\s*\d+\.\s+/,""))+'</li>'); i++; }
      out.push('<ol class="md-ol">'+buf.join("")+'</ol>'); continue;
    }
    if(line.trim()===""){ i++; continue; }
    const buf=[line]; i++;                                   // 段落（连续非空非特殊行，软换行→<br>）
    while(i<lines.length && lines[i].trim()!=="" && !isSpecial(lines[i]) &&
          !(lines[i].includes("|") && i+1<lines.length && /^\s*\|?[\s:|-]+\|?\s*$/.test(lines[i+1]))){ buf.push(lines[i]); i++; }
    out.push('<p class="md-p">'+buf.map(mdInline).join("<br>")+'</p>');
  }
  return out.join("");
}
