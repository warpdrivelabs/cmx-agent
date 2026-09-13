// ── 会话 item 渲染（复刻 opencode timeline）──
// 一个回合 = 一张 .turn 卡；回合内行序：用户消息 / 思考 / 正文气泡 / 工具卡 / 分隔条。
// 工具调用与结果按 call_id 配对成**一张**可折叠工具卡（opencode BasicTool）：
// 一行 trigger（状态点 + 工具名 + 副标题 + 参数 chips + chevron），点击展开详情，运行中禁止展开；
// 连续只读工具（fs_read/glob/grep…）完成后折叠为一行「已引用 N 处」上下文组（opencode ContextGroup）。
// 思考过程 = 可折叠 reasoning 卡：流式展开、正文开始后自动折叠、标题显示摘要首行；busy 暂无输出时
// 显示 shimmer 思考行。中断（stopped）渲染「⎋ 已中断」分隔条（opencode interrupted divider）。

// 只读上下文工具：连续完成后折叠成一行引用组（对齐 opencode CONTEXT_GROUP_TOOLS = read/glob/grep/list）
const CONTEXT_GROUP_TOOLS = new Set(["fs_read","glob","grep","repo_map","doc_read","data_describe"]);
// 工具图标（沿用现有 emoji 体系，双主题安全）
const TOOL_GLYPHS = {
  task:"🤖", shell:"⌨️", web_fetch:"🌐", web_search:"🔎", browser_read:"🖥", browser_screenshot:"📷",
  browser_do:"⌨️", computer_use:"👁️", flow_start_instance:"⚙️", flow_complete_task:"⚙️",
  onto_put_object:"🧩", onto_execute_action:"🧩", report_compute:"📊", enterprise_context:"🏛️",
  business_chain:"🔗", plugin_list:"🧩", plugin_marketplace:"🛒", plugin_install:"🧩",
  fs_edit:"✏️", apply_patch:"🩹", fs_write:"📝", fs_read:"📄", glob:"📂", grep:"🔍",
  repo_map:"🗺️", doc_read:"📑", data_describe:"📈", chart:"📊", run_tests:"🧪", git:"🌿", plan:"🗒️",
};
function toolGlyph(name){ return TOOL_GLYPHS[name] || "🔧"; }
// 工具类别中文名（贴参考图：一行式「终端 cd …」「搜索 北京今天天气」），原名在 title 悬停可见
const TOOL_LABELS = {
  shell:"终端", run_tests:"测试", git:"Git",
  fs_read:"读取", fs_write:"写入", fs_edit:"编辑", apply_patch:"补丁",
  glob:"检索", grep:"检索", repo_map:"仓库地图",
  web_search:"搜索", web_fetch:"网页", browser_read:"网页", browser_screenshot:"截图", browser_do:"浏览器",
  computer_use:"视觉操作", doc_read:"文档", data_describe:"数据", chart:"图表",
  task:"子智能体", plan:"计划",
  add:"计算", echo:"回显", clock:"时钟", job:"任务", sandbox:"沙箱", proc:"进程",
};
function toolLabel(name){ return TOOL_LABELS[name] || name; }
// 副标题候选键（对齐 opencode label()：description/query/url/filePath/path/pattern/name + cmx 特有）
const TOOL_SUB_KEYS = ["cmd","path","filePath","url","query","pattern","name","definitionKey","taskId",
  "objectType","actionType","reportCode","description"];
function toolSubtitle(input){
  for(const k of TOOL_SUB_KEYS){
    const v=input&&input[k];
    if(typeof v==="string" && v) return v.length>160?v.slice(0,160)+"…":v;
  }
  return "";
}
// 其余标量参数 → chips（最多 3 个，对齐 opencode args()）
function toolArgChips(input){
  const skip=new Set(TOOL_SUB_KEYS), out=[];
  for(const [k,v] of Object.entries(input||{})){
    if(skip.has(k)) continue;
    if(typeof v==="string"||typeof v==="number"||typeof v==="boolean"){
      let s=String(v); if(s.length>40) s=s.slice(0,40)+"…";
      out.push(k+"="+s);
      if(out.length>=3) break;
    }
  }
  return out;
}

// ── 工具卡（opencode BasicTool）：一行 trigger（状态点 + 工具名 + 副标题 + 参数 chips），
// 点击展开详情，运行中禁止展开；无 chevron、无操作按钮（ZCode 式：执行细节行保持素净）──
function toolCardHtml(call){
  const sub=toolSubtitle(call.input);
  const args=toolArgChips(call.input);
  return `<button class="tc-trig" type="button">`
    +`<span class="tc-st run" title="运行中"><span class="tc-spin"></span></span>`
    +`<span class="tc-g">${esc(toolGlyph(call.name))}</span>`
    +`<span class="tc-name" title="${esc(call.name||"tool")}">${esc(toolLabel(call.name))}</span>`
    +(sub?`<span class="tc-sub" title="${esc(sub)}">${esc(sub)}</span>`:"")
    +args.map(a=>`<span class="tc-arg">${esc(a)}</span>`).join("")
    +`<span class="tc-chev">▾</span>`
    +`</button>`
    +`<div class="tc-body" hidden><div class="tcempty">等待结果…</div></div>`;
}
function setToolStatus(card, cls, tip){
  const st=card.querySelector(".tc-st");
  if(!st) return;
  st.className="tc-st "+cls;
  if(tip) st.title=tip;
  st.innerHTML = cls==="ok" ? ICON_CHECK : cls==="bad" ? ICON_X : `<span class="tc-spin"></span>`;
}
function bindToolCard(card){
  card.querySelector(".tc-trig").addEventListener("click",(e)=>{
    if(e.target.closest(".tcacts")) return;              // 操作按钮不触发展开
    if(card.classList.contains("run")) return;           // opencode：pending 禁止展开
    const body=card.querySelector(".tc-body");
    const open=body.hidden;
    body.hidden=!open;
    card.classList.toggle("open",open);
  });
}
// tool_invoked：按 call_id 建卡（重复事件幂等）。只读上下文工具的 invoke 不关闭引用组
//（连续 read/grep 类工具的 invoke 夹在两个 result 之间，组必须跨过它们才能合到一处）。
function ensureToolCard(log, call){
  if(!CONTEXT_GROUP_TOOLS.has((call&&call.name)||"")) closeCtxGroup(log);
  const id=(call&&call.id)||"";
  let card=id ? (log._tools&&log._tools.get(id)) : null;
  if(card) return card;   // 只看引用：历史回放在 detached 容器里做，isConnected 恒为 false 不可作判据
  card=el("tool tcard run");
  if(id) card.dataset.callid=id;
  card.dataset.tool=(call&&call.name)||"";
  card.innerHTML=toolCardHtml(call||{});
  bindToolCard(card);
  if(id){ log._tools=log._tools||new Map(); log._tools.set(id,card); }
  ensureTurn(log).append(card);
  return card;
}
// tool_result：按 call_id 配对回填；上下文工具折叠入组；找不到卡（分页边界）走旧式独立结果卡
function completeToolCard(log, ev){
  const id=ev.call_id||"";
  const card=id ? (log._tools&&log._tools.get(id)) : null;
  if(!card){   // 只看引用（isConnected 在历史回放的 detached 容器里恒假，会导致一工具两卡）
    closeCtxGroup(log);
    const t=el("tool"+(ev.ok?"":" denied"));
    t.innerHTML=renderToolResult(ev);
    ensureTurn(log).append(t);
    return;
  }
  card.classList.remove("run");
  card.classList.add(ev.ok?"done":"fail");
  setToolStatus(card, ev.ok?"ok":"bad", ev.ok?"完成":"失败 / 被拦截");
  card.querySelector(".tc-body").innerHTML=toolBodyHtml(ev);
  if(CONTEXT_GROUP_TOOLS.has(card.dataset.tool)){ foldIntoCtxGroup(log, card, !!ev.ok); return; }   // 失败也入组：满屏红叉比静默失败更吵（opencode 降噪）
  closeCtxGroup(log);                      // 非只读结果落卡后，引用组定格（后续只读工具另起一组）
}
// 上下文组：连续只读工具折成一行「已引用 N 处」，点击展开逐条；条目点击再看完整结果
function foldIntoCtxGroup(log, card, ok){
  if(!log._ctx){   // 只看引用：closeCtxGroup 在所有边界事件（正文/换回合/非只读工具）清空
    const wrap=el("ctxgroup");
    wrap.innerHTML=`<button class="cg-head" type="button"><span class="cg-g">🔍</span>`
      +`<span class="cg-label">已引用 <b>0</b> 处</span><span class="tc-chev">▾</span></button>`
      +`<div class="cg-list" hidden></div>`;
    wrap.querySelector(".cg-head").addEventListener("click",()=>{
      const list=wrap.querySelector(".cg-list");
      const open=list.hidden;
      list.hidden=!open;
      wrap.classList.toggle("open",open);
    });
    ensureTurn(log).append(wrap);
    log._ctx={wrap, n:0, fail:0};
  }
  const g=log._ctx;
  g.n+=1;
  if(!ok) g.fail+=1;
  g.wrap.querySelector(".cg-label b").textContent=g.n;
  let fl=g.wrap.querySelector(".cg-fail");
  if(g.fail>0){ if(!fl){ fl=el("cg-fail"); g.wrap.querySelector(".cg-label").after(fl); } fl.textContent=g.fail+" 失败"; }
  const item=el("cg-item");
  const sub=card.querySelector(".tc-sub");
  item.innerHTML=`<span class="cg-st ${ok?"ok":"bad"}" title="${ok?"完成":"失败 / 被拦截"}">${ok?"✓":"✗"}</span>`
    +`<span class="cg-g">${esc(toolGlyph(card.dataset.tool))}</span>`
    +`<span class="cg-name">${esc(toolLabel(card.dataset.tool))}</span>`
    +`<span class="cg-sub">${esc(sub?sub.textContent:"")}</span>`;
  const body=el("cg-body");
  body.hidden=true;
  body.append(card);                    // 原卡整体搬入（保留 .tool 内容样式），隐藏其 trigger
  const cb=card.querySelector(".tc-body");
  if(cb){ cb.hidden=false; }            // 组内 trigger 已隐藏 → 直接展开结果体
  card.classList.add("open");
  item.append(body);
  item.addEventListener("click",()=>{ body.hidden=!body.hidden; });
  g.wrap.querySelector(".cg-list").append(item);
}
function closeCtxGroup(log){ log._ctx=null; }
// 历史回放收敛：仍处 pending 的工具卡（中断/分页截断无回执）标记为「无回执」而非永久转圈
function settlePendingCards(root){
  root.querySelectorAll(".tool.tcard.run").forEach(card=>{
    card.classList.remove("run");
    card.classList.add("done");
    setToolStatus(card,"ok","无回执（已中断或历史截断）");
    const body=card.querySelector(".tc-body");
    if(body && !body.querySelector(".tterm,.tcode,.tdiff,.tdata,.tcsub,.wsr,.tchartimg"))
      body.innerHTML=`<div class="tcempty">（无回执：回合被中断或超出历史分页）</div>`;
  });
}

// ── 结果正文（只含内容，头部信息由 trigger 承担）──
function toolBodyHtml(ev){
  const o=ev.output||{};
  if(!ev.ok) return `<div class="tcempty">⛔ 被拦截 <code>${esc(compact(o))}</code></div>`;
  if(o.final!=null) return `<div class="tcsub md">${renderMarkdown(String(o.final))}</div>`;
  if(o.svg!=null){
    let b64=""; try{ b64=btoa(unescape(encodeURIComponent(o.svg))); }catch(e){}
    const saved=o.saved?` <span class="tcpath">已存 ${esc(String(o.saved).split("/").pop())}</span>`:"";
    return `<div class="tchead">📊 图表${saved}</div><div class="tchartimg"><img alt="chart" src="data:image/svg+xml;base64,${b64}"></div>`;
  }
  if(Array.isArray(o.results) && o.query!=null){
    const items=o.results.map(x=>`<div class="wsr"><div class="wsr-t">${esc(x.title||x.url||"")}</div>`
      +`<div class="wsr-u">${esc(x.url||"")}</div>`
      +(x.snippet?`<div class="wsr-s">${esc(x.snippet)}</div>`:"")+`</div>`).join("");
    return (items||`<div class="tcempty">（无结果）</div>`)
      +`<div class="tcmeta" style="margin-top:4px">${o.count??o.results.length} 条 · ${esc(String(o.query))}</div>`;
  }
  if(o.bytes!=null && o.width!=null && o.path!=null){
    const kb=(o.bytes/1024).toFixed(0);
    return `<div class="tcmeta">${o.width}×${o.height} · ${kb} KB</div><div class="wf-url">${esc(String(o.path))}</div>`;
  }
  if(o.computer_use===true){
    const acts=(o.actions||[]).map(a=>`<span class="cuact">${esc(String(a))}</span>`).join("");
    const badge=o.done?`<span class="apresolved ok">✓ 达成</span>`:`<span class="apresolved no">步数用尽</span>`;
    return (o.goal?`<div class="apreason">🎯 ${esc(String(o.goal))}</div>`:"")
      +(acts?`<div class="cutrace">${acts}</div>`:"")
      +(o.answer?`<div class="tcsub md">${renderMarkdown(String(o.answer))}</div>`:"")
      +`<div style="margin-top:6px">${badge}</div>`;
  }
  if(o.interacted===true && o.text!=null){
    const title=o.title?`<div class="wf-title">${esc(String(o.title))}</div>`:"";
    return title+(o.url?`<div class="wf-url">${esc(String(o.url))}</div>`:"")+`<pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  if(o.rendered===true && o.text!=null){
    const title=o.title?`<div class="wf-title">${esc(String(o.title))}</div>`:"";
    return title+(o.url?`<div class="wf-url">${esc(String(o.url))}</div>`:"")+`<pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  if(o.text!=null && o.url!=null && o.status!=null){
    const title=o.title?`<div class="wf-title">${esc(String(o.title))}</div>`:"";
    return title+`<div class="wf-url">${esc(String(o.url))}</div><div class="tcmeta">HTTP ${esc(String(o.status))} · ${esc(String(o.chars??""))} 字符</div><pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  if(o.rows!=null && Array.isArray(o.columns)){
    const cols=o.columns.map(c=>{
      if(c.type==="numeric") return `<tr><td>${esc(c.name)}</td><td>数值</td><td>min ${esc(String(c.min))} · max ${esc(String(c.max))} · 均值 ${esc(String(c.mean))}</td></tr>`;
      const top=(c.top||[]).map(t=>esc(String(t.value))+"("+esc(String(t.count))+")").join("、");
      return `<tr><td>${esc(c.name)}</td><td>文本</td><td>去重 ${esc(String(c.distinct))} · Top ${top}</td></tr>`;
    }).join("");
    return `<div class="tcmeta">${o.rows} 行 · ${o.columns.length} 列</div><table class="tdata"><thead><tr><th>列</th><th>类型</th><th>统计</th></tr></thead><tbody>${cols}</tbody></table>`;
  }
  if(o.kind==="excel" && Array.isArray(o.sheets)){
    const sh=o.sheets[0]||{}; const rows=(sh.data||[]).slice(0,12);
    const body=rows.map((r,i)=>`<tr>${r.slice(0,10).map(c=>i===0?`<th>${esc(String(c))}</th>`:`<td>${esc(String(c))}</td>`).join("")}</tr>`).join("");
    // sheet 名来自用户文件内容，必须转义（XSS 注入面：IM 遥控可驱动 agent 读任意 xlsx）。
    return `<div class="tcmeta">${o.sheets.length} sheet · ${esc(String(sh.name||""))} ${esc(String(sh.rows_total??"?"))}×${esc(String(sh.cols_total??"?"))}</div><table class="tdata"><tbody>${body}</tbody></table>`;
  }
  if(o.kind==="pptx" && Array.isArray(o.slides)){
    const s=o.slides.map(sl=>`<b>幻灯片 ${sl.slide}</b>\n${sl.text}`).join("\n\n");
    return `<pre class="tcode">${esc(s)}</pre>`;
  }
  if((o.kind==="pdf"||o.kind==="docx"||o.kind==="text") && o.text!=null){
    return `<div class="tcmeta">${o.chars??""} 字符</div><pre class="tcode">${esc(String(o.text))}</pre>`;
  }
  if(o.diagnostics!=null) return (o.diagnostics.length?`<pre class="tcode">${esc(o.diagnostics.join("\n"))}</pre>`:`<div class="tcempty">（无诊断）</div>`);
  if(o.hover!=null) return `<pre class="tcode">${esc(String(o.hover))||"（无信息）"}</pre>`;
  if(o.symbols!=null) return `<pre class="tcode">${esc(o.symbols.join("\n"))||"（无）"}</pre>`;
  if(o.locations!=null) return `<pre class="tcode">${esc(o.locations.join("\n"))||"（无）"}</pre>`;
  const r = o.result || o;
  if(r && (r.stdout!=null || r.exit_code!=null || r.timed_out!=null)){
    const out=(r.stdout||"").trimEnd(), err=(r.stderr||"").trimEnd();
    const code=r.exit_code;
    const failed = !!r.timed_out || (code!=null && code!==0);
    const tip = r.timed_out ? "超时" : ("退出码 "+(code ?? "?"));
    let h=`<div class="tcmeta ${failed?"bad":"ok"}" title="${tip}">${failed?"❌":"✅"} ${esc(tip)}</div>`;
    if(out) h+=`<pre class="tterm">${esc(out)}</pre>`;
    if(err) h+=`<pre class="tterm tterr">${esc(err)}</pre>`;
    if(!out && !err) h+=`<div class="tcempty">（无输出）</div>`;
    return h;
  }
  if(o.service==="cmx-plugin" && Array.isArray(o.plugins) && o.url!==undefined){
    const items=o.plugins.map(p=>`<div class="ecrow"><span class="ecl">${esc(String(p.kind||""))}</span><span class="ecc"><span class="cuact">${esc(String(p.name||""))}${p.version?(" <span style=\"color:var(--muted)\">v"+esc(String(p.version))+"</span>"):""}</span><span style="color:var(--muted);font-size:11.5px">${p.installable?"":"（无内嵌清单）"}${esc(String(p.description||""))}</span></span></div>`).join("");
    return `<div class="tcmeta">${o.count??0} 个可装 · ${esc(String(o.market||""))}</div>${items||'<div class="tcempty">（市场暂无可装插件）</div>'}<div class="tcempty">plugin_install 传 {name} 即可从市场安装</div>`;
  }
  if(o.service==="cmx-plugin" && Array.isArray(o.plugins)){
    const items=o.plugins.map(p=>`<div class="ecrow"><span class="ecl">${esc(String(p.kind||""))}</span><span class="ecc"><span class="cuact">${esc(String(p.name||""))}</span><span style="color:var(--muted);font-size:11.5px">${esc(String(p.description||""))}</span></span></div>`).join("");
    return `<div class="tcmeta">${o.count??0} 个</div>${items||'<div class="tcempty">（暂无插件；plugin_install 可装）</div>'}`;
  }
  if(o.service==="cmx-plugin" && o.installed){
    return `<div class="tcmeta">${esc(String(o.name||""))} · ${esc(String(o.kind||""))}</div><div class="tcempty">${esc(String(o.note||""))}</div>`;
  }
  if(o.service==="cmx-plugin" && o.plugin){
    const meta=o.kind==="http"?("HTTP "+(o.status||"")):("exit "+(o.exit_code??"?"));
    const body=o.kind==="http"?compact(o.body||{}):(String(o.stdout||"")+(o.stderr?"\n"+o.stderr:""));
    return `<div class="tcmeta">${esc(meta)}</div><pre class="tcode">${esc(String(body))}</pre>`;
  }
  if(o.service==="cmx-chain" && Array.isArray(o.steps)){
    const rows=o.steps.map(s=>`<div class="ecrow"><span class="ecl">${s.ok?"✅":"❌"} 步${s.step} ${esc(String(s.op||""))}</span><span class="ecc"><span class="cuact">${esc(compact(s.ok?(s.output||{}):{error:s.error}))}</span></span></div>`).join("");
    const badge=o.completed?`<span class="apresolved ok">✓ 全程完成</span>`:`<span class="apresolved no">中途失败</span>`;
    return rows+`<div style="margin-top:6px">${badge}</div>`;
  }
  if(o.service==="cmx-enterprise" && o.context){
    const c=o.context, sec=(label,g,fmt)=>{ if(!g||!g.items) return "";
      const chips=g.items.map(fmt).map(t=>`<span class="cuact">${esc(t)}</span>`).join("");
      return `<div class="ecrow"><span class="ecl">${label} <b>${g.total}</b></span><span class="ecc">${chips}</span></div>`; };
    return sec("对象",c.objectTypes,x=>(x.displayName||x.apiName||"")+"·"+(x.apiName||""))
      +sec("关系",c.relations,x=>(x.apiName||"")+"("+(x.from||"?")+"→"+(x.to||"?")+")")
      +sec("动作",c.actions,x=>(x.displayName||x.apiName||""))
      +sec("流程",c.flows,x=>(x.name||x.key||"")+(x.startable?"✓":""))
      +sec("报表",c.reports,x=>(x.name||x.code||""));
  }
  if(o.service==="cmx-flow" && (o.started||o.completed)){
    const id=o.instanceId||o.taskId||"";
    return id?`<div class="wf-url">${esc(String(id))}${o.status?" · "+esc(String(o.status)):""}</div>`:`<div class="tcempty">已完成</div>`;
  }
  if(o.service==="cmx-ontology" && (o.wrote||o.executed)){
    return `<pre class="tcode">${esc(compact(o.data||{}))}</pre>`;
  }
  if(o.service==="cmx-report" && o.computed){
    return `<div class="tcmeta">${o.cellCount??"?"} 格 · 错误 ${o.errorCount??0}</div>`;
  }
  if(o.text!=null) return `<pre class="tcode">${esc(String(o.text))}</pre>`;
  if(o.tree!=null) return `<pre class="tcode">${esc(String(o.tree))}</pre>`;
  return `<span class="chip">✅ <code>${esc(compact(o))}</code></span>`;
}

// 旧式完整结果卡（独立兜底：invoke 与 result 跨分页边界时使用）
function renderToolResult(ev){
  const o=ev.output||{};
  if(!ev.ok) return `<span class="chip">⛔ 被拦截 <code>${esc(compact(o))}</code></span>`;
  return toolBodyHtml(ev);
}
// 操作按钮（复制/赞/踩/分享）只挂每回合**最终回复**：新定稿出现时撤掉本回合旧气泡的操作行，
// 之前回合的操作行不动（ZCode：每回合末尾一组）。
function markFinalBubble(log, bub){
  if(log._turn) log._turn.querySelectorAll(".bubble > .tcacts").forEach(a=>a.remove());
  addBubbleActions(bub);
}
function compact(v){ try{ const s=JSON.stringify(v); return s.length>240?s.slice(0,240)+"…":s; }catch(e){ return String(v); } }

// ── 思考行（opencode Thinking row）：busy 且暂无可见输出时显示 shimmer 行 ──
function showTyping(log){
  if(log._typing) { ensureTurn(log).append(log._typing); log.scrollTop=1e9; return; }
  const t=el("thinking-row","<span class='th-ico'>✦</span><span class='th-txt shimmer'>思考中</span>");
  ensureTurn(log).append(t);
  log._typing=t; log.scrollTop=1e9;
}
function hideTyping(log){ if(log._typing){ log._typing.remove(); log._typing=null; } }

// ── 思考过程卡：流式展开，收尾自动折叠，头部显示「思考 · 持续 N 秒」（贴参考界面）──
// 多步回合可有多段思考（ZCode 式）：每段各一张卡、按发生顺序 append 到回合末尾；
// 仅流式中的同一张卡可复用；closed 的卡不复用——新段必须新卡，时序位置才正确。
function ensureReasoningCard(log, evTs){
  if(log._rcard && !log._rcard.classList.contains("closed")) return log._rcard;
  const card=el("reasoning");
  card.innerHTML=`<button class="reasoning-head" type="button"><span class="rh-ico">🧠</span>`
    +`<span class="rh-txt shimmer">思考</span><span class="rh-dur"></span><span class="chev">▾</span></button>`
    +`<div class="reasoning-body"></div>`;
  card.querySelector(".reasoning-head").addEventListener("click",()=>{
    if(card.classList.contains("streaming")) return;   // 流式中锁定（防误点打断阅读节奏）
    card.classList.toggle("closed");
  });
  ensureTurn(log).append(card);
  log._rcard=card;
  log._rsb=card.querySelector(".reasoning-body");
  log._rraw="";
  log._rStartTs=evTs||Date.now();                      // 思考起点（事件 ts，回放/实时都成立）
  return card;
}
// 时长格式化：<3s 说「几秒」，否则「N 秒 / X 分 Y 秒」（贴参考图「思考 · 持续了几秒」）
function fmtDur(ms){
  if(ms<3000) return "几秒";
  const s=Math.round(ms/1000);
  if(s<60) return s+" 秒";
  const m=Math.floor(s/60), r=s%60;
  return r? (m+" 分 "+r+" 秒") : (m+" 分钟");
}
function closeReasoning(log){
  if(log._rraf){ cancelAnimationFrame(log._rraf); log._rraf=null; }
  if(log._rcard){
    log._rcard.classList.remove("streaming","retrying");
    log._rcard.classList.add("closed");                 // 正文/收尾开始 → 自动折叠（opencode 行为）
    const head=log._rcard.querySelector(".rh-txt");
    if(head){ head.classList.remove("shimmer"); head.textContent="思考"; }
    const dur=log._rcard.querySelector(".rh-dur");
    const end=log._lastTs||Date.now();
    if(dur && log._rStartTs) dur.textContent="· 持续了"+fmtDur(Math.max(0,end-log._rStartTs));
  }
  // 本段若经 delta 流式渲过，记下全文供落库回执对账（同文跳过）；回放等非流式段不记。
  if(log._liveR) log._liveRDone = (log._rraw||"").trim();
  log._liveR = false;
  log._rsb=null; log._rraw=""; log._rcard=null;         // 每段思考各一张卡：收段即清引用，下段另起新卡
}

// ── 回合计时行「已工作 X」（贴参考图）：回合出现第一个内容事件时即插入（用户气泡下方），
// 进行中每秒实时跳动；turn_ended 定格时长并变成折叠开关（收起思考/工具执行细节）。
// 历史回放（log._history）不起定时器，直接在 turn_ended 一次性落定。 ──
function insertWorkDur(turn,row){
  const first=turn.firstElementChild;
  if(first&&first.classList.contains("user-chip")) first.after(row);   // 参考图：计时行在用户气泡下方
  else turn.prepend(row);
}
function ensureWorkDur(log){
  if(log._history||log._closed) return;                 // 回放不起实时行；已中断的回合不再拉起新计时行
  if(!log._turnStartTs) log._turnStartTs=Date.now();
  if(log._durRow) return;
  const turn=ensureTurn(log);
  const row=el("work-dur");
  row.innerHTML=`<span class="wd-t">已工作 ${esc(fmtDur(Date.now()-log._turnStartTs))}</span><span class="chev">▾</span>`;
  row.addEventListener("click",()=>turn.classList.toggle("folded"));
  insertWorkDur(turn,row);
  log._durRow=row;
  log._durTick=setInterval(()=>{                        // 实时跳动（codex StatusTimer 的渲染时现算版）
    if(!log._turnStartTs||!log._durRow) return;
    const t=log._durRow.querySelector(".wd-t");
    if(t) t.textContent="已工作 "+fmtDur(Math.max(0,Date.now()-log._turnStartTs));
  },1000);
}
function stopWorkDurTick(log){
  if(log._durTick){ clearInterval(log._durTick); log._durTick=null; }
}
function finalizeWorkDur(log){
  stopWorkDurTick(log);
  const end=log._lastTs||Date.now();
  if(log._durRow){                                      // 实时行 → 定格时长，保留为折叠开关
    const t=log._durRow.querySelector(".wd-t");
    if(t&&log._turnStartTs) t.textContent="已工作 "+fmtDur(Math.max(0,end-log._turnStartTs));
    log._durRow=null;
    return;
  }
  if(!log._turn||!log._turnStartTs) return;             // 回放/无回合：一次性补静态计时行
  const turn=log._turn;                                 // 局部捕获：turn_ended 收尾即置 null，点击时不能再摸 log._turn
  const row=el("work-dur");
  row.innerHTML=`<span class="wd-t">已工作 ${esc(fmtDur(Math.max(0,end-log._turnStartTs)))}</span><span class="chev">▾</span>`;
  row.addEventListener("click",()=>turn.classList.toggle("folded"));
  insertWorkDur(turn,row);
}

// ── 会话事件渲染主入口（历史回放与实时流共用）──
// 视觉对齐参考界面：无头像、无气泡底的连续文档式线程；
// 用户消息=小气泡条；回合计时行「已工作 X」可折叠思考/工具等执行细节。
function renderEvent(log, ev, sid){
  const k=ev.kind;
  if(log._emptyCard){ log._emptyCard.remove(); log._emptyCard=null; }   // 首条事件：撤空会话占位卡
  if(ev.ts) log._lastTs = (typeof ev.ts==="string" ? Date.parse(ev.ts) : ev.ts) || log._lastTs;
  if(k==="turn_started"){
    stopWorkDurTick(log); log._durRow=null;             // 新回合：上一回合的实时计时行已定格，清引用
    log._closed=false; log._liveR=false;                // 思考对账标记随回合重置
    log._turnStartTs=log._lastTs||Date.now(); return;   // 只记起点（计时行随首个内容事件出现）
  }
  if(k!=="user_message") ensureWorkDur(log);            // 内容事件：确保「已工作」计时行已在（实时态）
  if(k==="text_delta"){
    closeCtxGroup(log);
    if(!log._sb){ closeReasoning(log); const b=el("bubble bare md"); ensureTurn(log).append(b); log._sb=b; log._raw=""; }
    log._raw=(log._raw||"")+(ev.text||"");
    if(!log._raf){ log._raf=requestAnimationFrame(()=>{ log._raf=null; if(log._sb) log._sb.innerHTML=renderMarkdown(log._raw); log.scrollTop=1e9; }); }
    log.scrollTop=1e9; return;
  }
  if(k==="text_reset"){
    if(log._raf){ cancelAnimationFrame(log._raf); log._raf=null; }
    log._raw=""; if(log._sb) log._sb.innerHTML="";
    return;
  }
  if(k==="reasoning_reset"){
    // 流中断重试：**保留已显示的思考正文**——这是用户唯一能看到的思考内容，清了就只剩空卡
    // （网关重试失败时下文不再来，空卡无解）。置灰 + 标「重试中」；缓冲作废，下一轮尝试的
    // delta 从头覆盖显示（后端 StreamAcc 也是按尝试重置的，两次尝试内容本就不该拼接）。
    // 注意先把挂起的 rAF 同步落定再取消：reset 可能在任何一帧绘制前到达，直取消息丢整段正文。
    if(log._rraf){ cancelAnimationFrame(log._rraf); log._rraf=null; }
    if(log._rsb) log._rsb.textContent=log._rraw||log._rsb.textContent;
    log._rraw="";
    if(log._rcard){
      log._rcard.classList.add("retrying");
      const dur=log._rcard.querySelector(".rh-dur");
      if(dur) dur.textContent="· 重试中…";
    }
    return;
  }
  if(k==="reasoning_delta" || k==="reasoning"){
    // 落库回执对账：本段已用 delta 流式渲过（closeReasoning 存了全文）→ 同文回执跳过，防重复卡。
    if(k==="reasoning"){
      const t=(ev.text||"").trim();
      if(log._liveRDone!=null && t && t===log._liveRDone){ log._liveRDone=null; return; }
    }
    closeCtxGroup(log);
    hideTyping(log);                                    // 思考卡本身就是可见输出：撤「思考中」占位行
    const card=ensureReasoningCard(log, log._lastTs||Date.now());
    card.classList.add("streaming");
    card.classList.remove("closed","retrying");
    if(k!=="reasoning"){ const d=card.querySelector(".rh-dur"); if(d) d.textContent=""; }  // 撤「重试中」
    if(k==="reasoning"){ log._rraw=ev.text||""; }
    else { log._rraw=(log._rraw||"")+(ev.text||""); log._liveR=true; }
    if(k==="reasoning"){
      if(log._rraf){ cancelAnimationFrame(log._rraf); log._rraf=null; }
      if(log._rsb) log._rsb.textContent=log._rraw;
    } else if(!log._rraf){
      log._rraf=requestAnimationFrame(()=>{ log._rraf=null;
        if(log._rsb){ log._rsb.textContent=log._rraw; log._rsb.scrollTop=log._rsb.scrollHeight; }  // 正文超 260px 时跟随到底，所见即所想
        log.scrollTop=1e9; });
    }
    log.scrollTop=1e9; return;
  }
  if(k==="user_message"){
    closeCtxGroup(log);
    if(log._skipUser){ log._skipUser=false; return; }   // 已乐观渲染，跳过流里的回显
    log._sb=null; closeReasoning(log); ensureTurn(log).append(el("user-chip", esc(ev.text)));
  }
  else if(k==="model_message"){
    closeCtxGroup(log);
    if(log._raf){ cancelAnimationFrame(log._raf); log._raf=null; }  // 取消 pending rAF，避免收尾后又渲一次
    if(log._sb){ log._sb.innerHTML=renderMarkdown(log._raw||ev.text||""); const bub=log._sb; log._sb=null; markFinalBubble(log,bub); }  // 收尾：定稿 markdown
    else if(ev.text){ closeReasoning(log); const b=el("bubble bare md"); b.innerHTML=renderMarkdown(ev.text); ensureTurn(log).append(b); markFinalBubble(log,b); }
  }
  else if(k==="tool_invoked"){ log._sb=null; closeReasoning(log); ensureToolCard(log, ev.call||{}); }
  else if(k==="tool_result"){ log._sb=null; completeToolCard(log, ev); }
  else if(k==="approval_requested"){ log._sb=null; closeCtxGroup(log);
    const t=el("tool approval"); t.dataset.callid=ev.call_id||""; t.dataset.sid=sid||"";
    t.innerHTML=`<div class="tchead">⏸ <b>需要审批</b> <span class="tcpath">${esc(ev.tool||"")}</span></div>`
      +`<div class="apreason">${esc(ev.reason||"该操作需人工确认")}</div>`
      +`<div class="aprow">`
      +`<button class="apbtn reject" data-act="approveTool" data-callid="${esc(ev.call_id||"")}" data-ok="0" data-all="0">✕ 拒绝</button>`
      +`<button class="apbtn allow" data-act="approveTool" data-callid="${esc(ev.call_id||"")}" data-ok="1" data-all="0">✓ 允许</button>`
      +`<button class="apbtn allowall" data-act="approveTool" data-callid="${esc(ev.call_id||"")}" data-ok="1" data-all="1" title="本对话后续需审批的操作不再逐次询问">🔓 本对话全部允许</button>`
      +`</div>`;
    ensureTurn(log).append(t); }
  else if(k==="approval_resolved"){
    let matched=false;
    log.querySelectorAll(".tool.approval").forEach(c=>{ if(c.dataset.callid===ev.call_id){ matched=true;
      const row=c.querySelector(".aprow"); if(row) row.remove();
      const prev=c.querySelector(".apresolved"); if(prev) prev.remove();
      const st=el("apresolved "+(ev.approved?"ok":"no"), ev.approved?("✓ 已允许"+(ev.by&&ev.by!=='user'?"（"+esc(ev.by)+"）":"")):"✕ 已拒绝");
      c.appendChild(st);
    }});
    if(!matched && ev.approved && (ev.by||"").indexOf("auto")===0){
      ensureTurn(log).append(el("meta apauto","🔓 已自动允许（本对话全部允许）"));
    }
  }
  else if(k==="turn_ended"){
    log._sb=null; hideTyping(log); closeCtxGroup(log); closeReasoning(log);
    const turn=log._turn;
    if(ev.reason==="stopped"){
      // 用户已手动中断过（分隔条已画）→ 只关回合；否则画「已中断」分隔条（opencode interrupted）
      if(log._intMarked){ log._intMarked=false; }
      else (turn||ensureTurn(log)).append(el("turn-divider int","⎋ 已中断"));
    }
    else if(ev.reason==="max_steps"){ ensureTurn(log).append(el("meta note","— 达到步数上限（"+ev.steps+" 步）—")); }
    else if(ev.reason==="error"){ ensureTurn(log).append(el("meta note bad","— 回合异常结束 —")); }
    // 回合计时行定格（实时行就地落字；回放补静态行）——贴参考图「已工作 1 分 11 秒」，点击折叠执行细节
    finalizeWorkDur(log);
    log._turnStartTs=null;
    // completed：不留收尾行（参考界面仅以留白分隔，减少噪音）
    log._turn=null;
  }
  else if(k==="note"){ log._sb=null; closeCtxGroup(log); ensureTurn(log).append(el("meta note",esc(ev.text||""))); }
  log.scrollTop=1e9;
}
