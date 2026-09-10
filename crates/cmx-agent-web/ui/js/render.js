// ── 工具卡片渲染：bash/run_tests 显示终端块；write/edit 显示内容；其余紧凑 JSON ──
function renderToolInvoke(c){
  const name=c.name||"", input=c.input||{};
  if(name==="task" && input.prompt!=null) return `<div class="tchead">🤖 <b>子智能体</b> <span class="tcpath">${esc(input.label||"子任务")}</span></div><div class="tcsub">${esc(String(input.prompt))}</div>`;
  if(name==="shell" && input.cmd!=null) return `<div class="tchead">🔧 <b>shell</b></div><pre class="tterm">$ ${esc(input.cmd)}</pre>`;
  if(name==="web_fetch" && input.url!=null) return `<div class="tchead">🌐 <b>web_fetch</b> <span class="tcpath">${esc(input.url)}</span></div>`;
  if(name==="web_search" && input.query!=null) return `<div class="tchead">🔎 <b>web_search</b> <span class="tcpath">${esc(input.query)}</span></div>`;
  if(name==="browser_read" && input.url!=null) return `<div class="tchead">🖥 <b>browser_read</b> <span class="tcpath">${esc(input.url)}</span></div>`;
  if(name==="browser_screenshot" && input.url!=null) return `<div class="tchead">📷 <b>browser_screenshot</b> <span class="tcpath">${esc(input.url)}</span></div>`;
  if(name==="browser_do" && input.url!=null) return `<div class="tchead">⌨️ <b>browser_do</b> <span class="tcpath">${esc(input.url)}</span> <span class="tcmeta">${(input.actions||[]).length} 步交互</span></div>`;
  if(name==="computer_use" && input.url!=null) return `<div class="tchead">👁️ <b>computer_use</b> <span class="tcpath">${esc(input.url)}</span></div>`+(input.goal?`<div class="apreason">🎯 ${esc(String(input.goal))}</div>`:"");
  // U11 引擎写侧调用头（写操作，随后会弹审批卡）
  if(name==="flow_start_instance") return `<div class="tchead">⚙️ <b>flow 起实例</b> <span class="tcpath">${esc(input.definitionKey||"")}</span></div>`;
  if(name==="flow_complete_task") return `<div class="tchead">⚙️ <b>flow 办理任务</b> <span class="tcpath">${esc(input.taskId||"")}</span></div>`;
  if(name==="onto_put_object") return `<div class="tchead">🧩 <b>onto 写对象</b> <span class="tcpath">${esc(input.objectType||"")}</span></div>`;
  if(name==="onto_execute_action") return `<div class="tchead">🧩 <b>onto 执行动作</b> <span class="tcpath">${esc(input.actionType||"")}</span>${input.dryRun?' <span class="tcmeta">试算</span>':''}</div>`;
  if(name==="report_compute") return `<div class="tchead">📊 <b>report 计算</b> <span class="tcpath">${esc(input.reportCode||"")}</span></div>`;
  if(name==="enterprise_context") return `<div class="tchead">🏛️ <b>企业上下文</b> <span class="tcmeta">拉取域模型${(input.scopes&&input.scopes.length)?"："+esc(input.scopes.join("/")):""}</span></div>`;
  if(name==="business_chain") return `<div class="tchead">🔗 <b>业务联动</b> <span class="tcmeta">${(input.steps||[]).length} 步流水线</span></div>`
    +((input.steps||[]).length?`<div class="cutrace">${(input.steps||[]).map(s=>`<span class="cuact">${esc(String(s.op||"?"))}</span>`).join("")}</div>`:"");
  if(name==="plugin_list") return `<div class="tchead">🧩 <b>plugin_list</b> <span class="tcmeta">列出已装插件</span></div>`;
  if(name==="plugin_marketplace") return `<div class="tchead">🛒 <b>plugin_marketplace</b> <span class="tcpath">${esc(input.url||"")}</span> <span class="tcmeta">浏览远程市场</span></div>`;
  if(name==="plugin_install") return `<div class="tchead">🧩 <b>plugin_install</b> <span class="tcpath">${esc((input.manifest&&input.manifest.name)||input.name||"")}</span>${input.name&&!input.manifest?' <span class="tcmeta">来自市场</span>':''}</div>`;
  // fs_edit → 红绿 diff（old_string 删除 / new_string 新增）
  if(name==="fs_edit" && input.old_string!=null && input.new_string!=null)
    return `<div class="tchead">✏️ <b>fs_edit</b> <span class="tcpath">${esc(input.path||"")}</span></div>${renderDiff(input.old_string, input.new_string)}`;
  // apply_patch → 直接按补丁行着色（+绿 -红 @@/***头）
  if(name==="apply_patch" && input.patch!=null)
    return `<div class="tchead">🩹 <b>apply_patch</b></div>${renderPatch(input.patch)}`;
  // fs_write → 内容代码块（新建/覆盖）
  if(name==="fs_write" && input.content!=null)
    return `<div class="tchead">🔧 <b>fs_write</b> <span class="tcpath">${esc(input.path||"")}</span></div><pre class="tcode">${esc(String(input.content))}</pre>`;
  return `<div class="tchead">🔧 调用 <b>${esc(name)}</b> <code>${esc(compact(input))}</code></div>`;
}
// U5：红绿 diff 渲染。fs_edit 的 old→new 替换（各行加 -/+ 标记）。
function renderDiff(oldStr, newStr){
  let h='<pre class="tdiff">';
  for(const l of String(oldStr).split("\n")) h+=`<span class="dl del">- ${esc(l)}</span>`;
  for(const l of String(newStr).split("\n")) h+=`<span class="dl add">+ ${esc(l)}</span>`;
  return h+'</pre>';
}
// U5：Codex 风格补丁按行着色。
function renderPatch(txt){
  let h='<pre class="tdiff">';
  for(const raw of String(txt).split("\n")){
    let cls="ctx";
    if(raw.startsWith("+")) cls="add";
    else if(raw.startsWith("-")) cls="del";
    else if(raw.startsWith("@@")||raw.startsWith("***")) cls="hunk";
    h+=`<span class="dl ${cls}">${esc(raw)||" "}</span>`;
  }
  return h+'</pre>';
}
function renderToolResult(ev){
  const o=ev.output||{};
  if(!ev.ok) return `<span class="chip">⛔ 被拦截 <code>${esc(compact(o))}</code></span>`;
  // 子智能体 task：final=子智能体最终回答（渲染为 markdown 小块）
  if(o.final!=null) return `<div class="tchead">🤖 子智能体完成 <span class="tcmeta">${o.steps??"?"} 步</span></div><div class="tcsub md">${renderMarkdown(String(o.final))}</div>`;
  // chart：内联 SVG 图表（base64 img，安全）
  if(o.svg!=null){
    let b64=""; try{ b64=btoa(unescape(encodeURIComponent(o.svg))); }catch(e){}
    const saved=o.saved?` <span class="tcpath">已存 ${esc(String(o.saved).split("/").pop())}</span>`:"";
    return `<div class="tchead">📊 图表${saved}</div><div class="tchartimg"><img alt="chart" src="data:image/svg+xml;base64,${b64}"></div>`;
  }
  // U10 web_search：结果列表（标题/链接/摘要）
  if(Array.isArray(o.results) && o.query!=null){
    const items=o.results.map(x=>`<div class="wsr"><div class="wsr-t">${esc(x.title||x.url||"")}</div>`
      +`<div class="wsr-u">${esc(x.url||"")}</div>`
      +(x.snippet?`<div class="wsr-s">${esc(x.snippet)}</div>`:"")+`</div>`).join("");
    return `<div class="tchead">🔎 搜索结果 <span class="tcmeta">${o.count??o.results.length} 条 · ${esc(String(o.query))}</span></div>`
      +(items||`<div class="tcempty">（无结果）</div>`);
  }
  // U8 browser_screenshot：截图落工作区（不内联图片，避免 webview 资源协议限制，展示路径+尺寸）
  if(o.bytes!=null && o.width!=null && o.path!=null){
    const kb=(o.bytes/1024).toFixed(0);
    return `<div class="tchead">📷 网页截图 <span class="tcmeta">${o.width}×${o.height} · ${kb} KB</span></div>`
      +`<div class="wf-url">${esc(String(o.path))}</div>`;
  }
  // U8 computer_use：视觉回环轨迹 + 结论
  if(o.computer_use===true){
    const acts=(o.actions||[]).map(a=>`<span class="cuact">${esc(String(a))}</span>`).join("");
    const badge=o.done?`<span class="apresolved ok">✓ 达成</span>`:`<span class="apresolved no">步数用尽</span>`;
    return `<div class="tchead">👁️ computer-use <span class="tcmeta">${o.steps??0} 步</span></div>`
      +(o.goal?`<div class="apreason">🎯 ${esc(String(o.goal))}</div>`:"")
      +(acts?`<div class="cutrace">${acts}</div>`:"")
      +(o.answer?`<div class="tcsub md">${renderMarkdown(String(o.answer))}</div>`:"")
      +`<div style="margin-top:6px">${badge}</div>`;
  }
  // U8 browser_do：交互后结果文本（标题 + URL + 正文；步数）
  if(o.interacted===true && o.text!=null){
    const title=o.title?`<div class="wf-title">${esc(String(o.title))}</div>`:"";
    return `<div class="tchead">⌨️ 浏览器交互 <span class="tcmeta">${o.steps??0} 步 · ${o.chars??""} 字符</span></div>`
      +title+(o.url?`<div class="wf-url">${esc(String(o.url))}</div>`:"")+`<pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  // U8 browser_read：无头渲染后的正文（同 web_fetch 版式，头标「渲染」）
  if(o.rendered===true && o.text!=null){
    const title=o.title?`<div class="wf-title">${esc(String(o.title))}</div>`:"";
    return `<div class="tchead">🖥 渲染网页 <span class="tcmeta">${o.chars??""} 字符</span></div>`
      +title+(o.url?`<div class="wf-url">${esc(String(o.url))}</div>`:"")+`<pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  // U10 web_fetch：网页正文（标题 + URL + 正文文本）
  if(o.text!=null && o.url!=null && o.status!=null){
    const head=`<div class="tchead">🌐 网页 <span class="tcmeta">${o.chars??""} 字符 · HTTP ${o.status}</span></div>`;
    const title=o.title?`<div class="wf-title">${esc(String(o.title))}</div>`:"";
    const link=`<div class="wf-url">${esc(String(o.url))}</div>`;
    return head+title+link+`<pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  // data_describe：数据概览（行数 + 列统计）
  if(o.rows!=null && Array.isArray(o.columns)){
    const cols=o.columns.map(c=>{
      if(c.type==="numeric") return `<tr><td>${esc(c.name)}</td><td>数值</td><td>min ${c.min} · max ${c.max} · 均值 ${c.mean}</td></tr>`;
      const top=(c.top||[]).map(t=>esc(String(t.value))+"("+t.count+")").join("、");
      return `<tr><td>${esc(c.name)}</td><td>文本</td><td>去重 ${c.distinct} · Top ${top}</td></tr>`;
    }).join("");
    return `<div class="tchead">📈 数据概览 <span class="tcmeta">${o.rows} 行 · ${o.columns.length} 列</span></div>`
      +`<table class="tdata"><thead><tr><th>列</th><th>类型</th><th>统计</th></tr></thead><tbody>${cols}</tbody></table>`;
  }
  // U6 办公文档读取
  if(o.kind==="excel" && Array.isArray(o.sheets)){
    const sh=o.sheets[0]||{}; const rows=(sh.data||[]).slice(0,12);
    const body=rows.map((r,i)=>`<tr>${r.slice(0,10).map(c=>i===0?`<th>${esc(String(c))}</th>`:`<td>${esc(String(c))}</td>`).join("")}</tr>`).join("");
    return `<div class="tchead">📊 Excel <span class="tcmeta">${o.sheets.length} sheet · ${sh.name||""} ${sh.rows_total??"?"}×${sh.cols_total??"?"}</span></div><table class="tdata"><tbody>${body}</tbody></table>`;
  }
  if(o.kind==="pptx" && Array.isArray(o.slides)){
    const s=o.slides.map(sl=>`<b>幻灯片 ${sl.slide}</b>\n${sl.text}`).join("\n\n");
    return `<div class="tchead">📑 PPT <span class="tcmeta">${o.slides.length} 页</span></div><pre class="tcode">${esc(s)}</pre>`;
  }
  if((o.kind==="pdf"||o.kind==="docx"||o.kind==="text") && o.text!=null){
    const label={pdf:"📄 PDF",docx:"📝 Word",text:"📄 文本"}[o.kind];
    return `<div class="tchead">${label} <span class="tcmeta">${o.chars??""} 字符</span></div><pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  // LSP 代码智能：诊断/悬停/符号/定位
  if(o.diagnostics!=null) return `<div class="tchead">🔎 LSP 诊断 <span class="tcmeta">${o.diagnostics.length} 条</span></div>`+(o.diagnostics.length?`<pre class="tcode">${esc(o.diagnostics.join("\n"))}</pre>`:`<div class="tcempty">（无诊断）</div>`);
  if(o.hover!=null) return `<div class="tchead">🔎 LSP 悬停</div><pre class="tcode">${esc(String(o.hover))||"（无信息）"}</pre>`;
  if(o.symbols!=null) return `<div class="tchead">🔎 LSP 符号 <span class="tcmeta">${o.symbols.length}</span></div><pre class="tcode">${esc(o.symbols.join("\n"))||"（无）"}</pre>`;
  if(o.locations!=null) return `<div class="tchead">🔎 LSP 定位 <span class="tcmeta">${o.locations.length}</span></div><pre class="tcode">${esc(o.locations.join("\n"))||"（无）"}</pre>`;
  // bash / run_tests 风格：有 stdout/exit_code → 终端块
  const r = o.result || o;   // run_tests 把 shell 结果包在 result 里
  if(r && (r.stdout!=null || r.exit_code!=null || r.timed_out!=null)){
    const out=(r.stdout||"").trimEnd(), err=(r.stderr||"").trimEnd();
    const code=r.exit_code;
    const failed = !!r.timed_out || (code!=null && code!==0);
    const tip = r.timed_out ? "超时" : ("退出码 "+(code ?? "?"));
    const status = `<span class="tcmeta tcstatus ${failed?"bad":"ok"}" title="${tip}">${failed?ICON_X:ICON_CHECK}</span>`;
    let h=`<div class="tchead">${failed?"❌":"✅"} 结果 ${status}</div>`;
    if(out) h+=`<pre class="tterm">${esc(out)}</pre>`;
    if(err) h+=`<pre class="tterm tterr">${esc(err)}</pre>`;
    if(!out && !err) h+=`<div class="tcempty">（无输出）</div>`;
    return h;
  }
  // U15 插件：远程市场（有 url）/ plugin_list 清单（有 dir）/ http/command/wasm 结果 / 安装
  if(o.service==="cmx-plugin" && Array.isArray(o.plugins) && o.url!==undefined){
    const items=o.plugins.map(p=>`<div class="ecrow"><span class="ecl">${esc(String(p.kind||""))}</span><span class="ecc"><span class="cuact">${esc(String(p.name||""))}${p.version?(" <span style=\"color:var(--muted)\">v"+esc(String(p.version))+"</span>"):""}</span><span style="color:var(--muted);font-size:11.5px">${p.installable?"":"（无内嵌清单）"}${esc(String(p.description||""))}</span></span></div>`).join("");
    return `<div class="tchead">🛒 插件市场 ${esc(String(o.market||""))} <span class="tcmeta">${o.count??0} 个可装</span></div>${items||'<div class="tcempty">（市场暂无可装插件）</div>'}<div class="tcempty">plugin_install 传 {name} 即可从市场安装</div>`;
  }
  if(o.service==="cmx-plugin" && Array.isArray(o.plugins)){
    const items=o.plugins.map(p=>`<div class="ecrow"><span class="ecl">${esc(String(p.kind||""))}</span><span class="ecc"><span class="cuact">${esc(String(p.name||""))}</span><span style="color:var(--muted);font-size:11.5px">${esc(String(p.description||""))}</span></span></div>`).join("");
    return `<div class="tchead">🧩 已装插件 <span class="tcmeta">${o.count??0} 个</span></div>${items||'<div class="tcempty">（暂无插件；plugin_install 可装）</div>'}`;
  }
  if(o.service==="cmx-plugin" && o.installed){
    return `<div class="tchead">🧩 插件已安装 <span class="tcmeta">${esc(String(o.name||""))} · ${esc(String(o.kind||""))}</span></div><div class="tcempty">${esc(String(o.note||""))}</div>`;
  }
  if(o.service==="cmx-plugin" && o.plugin){
    const meta=o.kind==="http"?("HTTP "+(o.status||"")):("exit "+(o.exit_code??"?"));
    const body=o.kind==="http"?compact(o.body||{}):(String(o.stdout||"")+(o.stderr?"\n"+o.stderr:""));
    return `<div class="tchead">🧩 ${esc(String(o.plugin))} <span class="tcmeta">${esc(meta)}</span></div><pre class="tcode">${esc(String(body))}</pre>`;
  }
  // U14 业务联动流水线：分步轨迹 + 是否全程完成
  if(o.service==="cmx-chain" && Array.isArray(o.steps)){
    const rows=o.steps.map(s=>`<div class="ecrow"><span class="ecl">${s.ok?"✅":"❌"} 步${s.step} ${esc(String(s.op||""))}</span><span class="ecc"><span class="cuact">${esc(compact(s.ok?(s.output||{}):{error:s.error}))}</span></span></div>`).join("");
    const badge=o.completed?`<span class="apresolved ok">✓ 全程完成</span>`:`<span class="apresolved no">中途失败</span>`;
    return `<div class="tchead">🔗 业务联动 <span class="tcmeta">${o.stepCount??0} 步</span></div>${rows}<div style="margin-top:6px">${badge}</div>`;
  }
  // U12 企业上下文：域模型分组摘要
  if(o.service==="cmx-enterprise" && o.context){
    const c=o.context, sec=(label,g,fmt)=>{ if(!g||!g.items) return "";
      const chips=g.items.map(fmt).map(t=>`<span class="cuact">${esc(t)}</span>`).join("");
      return `<div class="ecrow"><span class="ecl">${label} <b>${g.total}</b></span><span class="ecc">${chips}</span></div>`; };
    return `<div class="tchead">🏛️ 企业域模型</div>`
      +sec("对象",c.objectTypes,x=>(x.displayName||x.apiName||"")+"·"+(x.apiName||""))
      +sec("关系",c.relations,x=>(x.apiName||"")+"("+(x.from||"?")+"→"+(x.to||"?")+")")
      +sec("动作",c.actions,x=>(x.displayName||x.apiName||""))
      +sec("流程",c.flows,x=>(x.name||x.key||"")+(x.startable?"✓":""))
      +sec("报表",c.reports,x=>(x.name||x.code||""));
  }
  // U11 引擎写侧：flow 起实例 / 办任务 · onto 建对象 / 执行动作 · report 计算
  if(o.service==="cmx-flow" && (o.started||o.completed)){
    const act=o.started?"⚙️ 已发起流程实例":"⚙️ 已办理任务";
    const id=o.instanceId||o.taskId||"";
    return `<div class="tchead">${act} <span class="tcmeta">cmx-flow</span></div>`
      +(id?`<div class="wf-url">${esc(String(id))}${o.status?" · "+esc(String(o.status)):""}</div>`:"");
  }
  if(o.service==="cmx-ontology" && (o.wrote||o.executed)){
    const act=o.wrote?("🧩 已写对象 "+esc(String(o.objectType||""))):("🧩 已执行动作 "+esc(String(o.actionType||""))+(o.dryRun?"（试算）":""));
    return `<div class="tchead">${act} <span class="tcmeta">cmx-ontology</span></div>`
      +`<pre class="tcode">${esc(compact(o.data||{}))}</pre>`;
  }
  if(o.service==="cmx-report" && o.computed){
    return `<div class="tchead">📊 已计算报表 ${esc(String(o.reportCode||""))} <span class="tcmeta">${o.cellCount??"?"} 格 · 错误 ${o.errorCount??0}</span></div>`;
  }
  // fs_read / repo_map 等：有 text/tree → 文本块；否则紧凑 JSON
  if(o.text!=null) return `<div class="tchead">✅ 读取 <span class="tcpath">${esc(o.path||"")}</span></div><pre class="tcode">${esc(String(o.text))}</pre>`;
  if(o.tree!=null) return `<div class="tchead">✅ 目录结构</div><pre class="tcode">${esc(String(o.tree))}</pre>`;
  return `<span class="chip">✅ 结果 <code>${esc(compact(o))}</code></span>`;
}
function compact(v){ try{ const s=JSON.stringify(v); return s.length>240?s.slice(0,240)+"…":s; }catch(e){ return String(v); } }

// 打字等待指示器（AI 头像 + 三跳点）。多步回合里每次「AI 在忙但暂无可见输出」都显示。
function showTyping(log){
  if(log._typing) { log.append(log._typing); log.scrollTop=1e9; return; }  // 已存在则移到底部
  const r=el("row typing-row");
  r.append(el("avatar a","AI"), el("bubble typing","<span class='dot'></span><span class='dot'></span><span class='dot'></span>"));
  log.append(r); log._typing=r; log.scrollTop=1e9;
}
function hideTyping(log){ if(log._typing){ log._typing.remove(); log._typing=null; } }
