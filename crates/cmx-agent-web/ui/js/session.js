// ── 会话渲染（渲染到指定 log 容器）──
// 打字机：text_delta 累加到 log._raw，每次按 markdown 重渲当前气泡（log._sb）。
function renderEvent(log, ev, sid){
  const k=ev.kind;
  if(k==="text_delta"){
    if(!log._sb){ const r=el("row"); const b=el("bubble md"); r.append(el("avatar a","AI"),b); log.append(r); log._sb=b; log._raw=""; }
    log._raw=(log._raw||"")+(ev.text||"");
    log._sb.innerHTML=renderMarkdown(log._raw);
    log.scrollTop=1e9; return;
  }
  if(k==="user_message"){
    if(log._skipUser){ log._skipUser=false; return; }   // 已乐观渲染，跳过流里的回显
    log._sb=null; const r=el("row user"); r.append(el("avatar u","你"),el("bubble",esc(ev.text))); log.append(r);
  }
  else if(k==="model_message"){
    let bub=null;
    if(log._sb){ log._sb.innerHTML=renderMarkdown(log._raw||ev.text||""); bub=log._sb; log._sb=null; }  // 收尾：定稿 markdown
    else if(ev.text){ const r=el("row"); const b=el("bubble md"); b.innerHTML=renderMarkdown(ev.text); r.append(el("avatar a","AI"),b); log.append(r); bub=b; }
    if(bub) addBubbleActions(bub);   // AI 文本气泡也挂操作按钮（底部行）
  }
  else if(k==="tool_invoked"){ log._sb=null; const t=el("tool"); t.innerHTML=renderToolInvoke(ev.call||{}); addToolActions(t); log.append(t); }
  else if(k==="tool_result"){ const t=el("tool"+(ev.ok?"":" denied")); t.innerHTML=renderToolResult(ev); if(ev.ok) addToolActions(t); log.append(t); }
  else if(k==="approval_requested"){ log._sb=null;
    const t=el("tool approval"); t.dataset.callid=ev.call_id||""; t.dataset.sid=sid||"";
    t.innerHTML=`<div class="tchead">⏸ <b>需要审批</b> <span class="tcpath">${esc(ev.tool||"")}</span></div>`
      +`<div class="apreason">${esc(ev.reason||"该操作需人工确认")}</div>`
      +`<div class="aprow">`
      +`<button class="apbtn reject" data-act="approveTool" data-callid="${esc(ev.call_id||"")}" data-ok="0" data-all="0">✕ 拒绝</button>`
      +`<button class="apbtn allow" data-act="approveTool" data-callid="${esc(ev.call_id||"")}" data-ok="1" data-all="0">✓ 允许</button>`
      +`<button class="apbtn allowall" data-act="approveTool" data-callid="${esc(ev.call_id||"")}" data-ok="1" data-all="1" title="本对话后续需审批的操作不再逐次询问">🔓 本对话全部允许</button>`
      +`</div>`;
    log.append(t); }
  else if(k==="approval_resolved"){
    let matched=false;
    log.querySelectorAll(".tool.approval").forEach(c=>{ if(c.dataset.callid===ev.call_id){ matched=true;
      const row=c.querySelector(".aprow"); if(row) row.remove();
      const prev=c.querySelector(".apresolved"); if(prev) prev.remove();
      const st=el("apresolved "+(ev.approved?"ok":"no"), ev.approved?("✓ 已允许"+(ev.by&&ev.by!=='user'?"（"+esc(ev.by)+"）":"")):"✕ 已拒绝");
      c.appendChild(st);
    }});
    // 「本对话全部允许」后的自动放行：无对应审批卡 → 追加一条低调审计提示，而非弹卡。
    if(!matched && ev.approved && (ev.by||"").indexOf("auto")===0){
      log.append(el("meta apauto","🔓 已自动允许（本对话全部允许）"));
    }
  }
  else if(k==="turn_ended"){ log._sb=null; log.append(el("meta","— 回合 #"+ev.turn+" 结束（"+ev.reason+"，"+ev.steps+" 步）—")); }
  // Note：内核注记，回合出错时把错误文案作为 Note 追加（与飞书回发同一份内容，保证两端一致）。
  else if(k==="note"){ log._sb=null; log.append(el("meta note",esc(ev.text||""))); }
  log.scrollTop=1e9;
}

// ── 会话事件总线实时通道（U16）──
// 后端常驻 task 把 AgentApp 的 SessionEventBus 广播成全局 `session_event` Tauri 事件；前端监听后
// 按 session_id 分流：已打开的 tab 实时 renderEvent，并节流刷新会话列表（让新 IM 会话出现/排序）。
// 本地发起的回合在 STREAMING 集合里 → 跳过（交给 streamSend 的流式通道，避免双重渲染）；
// IM（飞书/微信/钉钉…）来源会话永不在 STREAMING → 实时渲染，实现「飞书发消息实时显示到对话界面」。
let _refreshTasksTimer = null;
function scheduleRefreshTasks(){
  if(_refreshTasksTimer) return;          // 已排队：合并到下一次
  _refreshTasksTimer = setTimeout(()=>{ _refreshTasksTimer=null; refreshTasks(); }, 400);
}
function onSessionEvent(env){
  if(!env || !env.session_id || !env.event) return;
  const sid = env.session_id, ev = env.event;
  if(STREAMING.has(sid)) return;          // 本地流式中：交给 streamSend，避免重复
  let tab = findTab("s:"+sid);
  if(!tab){
    // 远程（IM）来源新会话：同步建 tab+log，让本条及后续实时事件立即渲染（不被 get_events 擦除）；
    // 后台 get_events 补历史时只 prepend 未渲染过的事件，不擦除。避免「首条消息被 openSession 擦掉」。
    openSessionLive(sid);
    tab = findTab("s:"+sid);
  }
  const log = tab && tab.view.querySelector(".log");
  if(log){
    // 去重：已渲染过的 seq 跳过（防历史 prepend 与实时事件重复）。
    if(ev.seq && (log._seqs=log._seqs||new Set()).has(ev.seq)) { /* 已渲染 */ }
    else {
      // session_event 通道（IM 来源）渲染 user_message 时无视 _skipUser——那是本地乐观渲染
      // （doSendTab）专用标志，跨回合/跨通道残留会误吞 IM 的首条用户消息。
      const saved = log._skipUser; log._skipUser = false;
      renderEvent(log, ev, sid);
      log._skipUser = saved;
      if(ev.seq) log._seqs.add(ev.seq);
    }
  }
  scheduleRefreshTasks();
}

// 远程来源会话首次有事件时同步建 tab：实时事件立即渲染进 log；后台 get_events 补历史（prepend，不擦除）。
// 这是「飞书首条消息实时显示」的关键——若用普通 openSession（get_events 返回后 log.innerHTML="" 重画），
// 会与并发到达的实时事件竞态，把首条 user_message 擦掉。
function openSessionLive(sessionId){
  const tabId="s:"+sessionId;
  if(findTab(tabId)) return;
  const t=openTab({id:tabId, kind:"session", title:sessionId, ico:"💬", sessionId});
  const node=document.getElementById("tpl-session").content.cloneNode(true);
  t.view.className="tabview";
  t.view.append(node);
  const inp=t.view.querySelector(".inp2");
  inp.addEventListener("keydown",e=>{ if(e.key==="Enter"&&!e.shiftKey){e.preventDefault();sendChatTab(t);} });
  inp.addEventListener("input",e=>autoGrow(e.target));
  t.view.querySelector(".tab-send").onclick=()=>sendChatTab(t);
  const log=t.view.querySelector(".log"); log.innerHTML=""; log._seqs=new Set(); log._sb=null;
  activateTab(tabId);
  refreshTasks();
  // 后台补历史：把未渲染过的事件 prepend 到 log 顶部（历史在上、实时新事件在下），不擦除。
  call({cmd:"get_events",session_id:sessionId,limit:HIST_PAGE}).then(resp=>{
    const d=(resp&&resp.ok&&resp.data)||{}; const evs=d.events||[];
    const frag=document.createDocumentFragment();
    if((d.start||0)>0) frag.append(makeLoadMore(log, sessionId, d.start, d.total||0));
    evs.forEach(ev=>{
      if(log._seqs.has(ev.seq)) return;
      const tmp=document.createElement('div'); tmp._sb=null;
      const s=tmp._skipUser; tmp._skipUser=false; renderEvent(tmp, ev, sessionId); tmp._skipUser=s;
      while(tmp.firstChild) frag.append(tmp.firstChild);
      log._seqs.add(ev.seq);
    });
    log.prepend(frag);
    if(d.title){ t.title=d.title||sessionId; renderTabs(); }
  });
}

function newTask(){ ACTIVE=null; CURRENT=null; TABS.forEach(t=>t.view.classList.remove("active")); renderTabs();
  document.getElementById("inp").value=""; document.getElementById("inp").focus(); }

// ── 连接器：作为一个 tab 打开 ──
async function openConnectors(){
  const t=openTab({id:"connectors", kind:"connectors", title:"专家·技能·连接器", ico:"🧩"});
  if(!t.view.dataset.built){
    t.view.dataset.built="1";
    const node=document.getElementById("tpl-connectors").content.cloneNode(true);
    t.view.className="tabview";
    t.view.append(node);
    t.view.querySelector(".tab-conn-refresh").onclick=()=>loadConnectors(t.view);
  }
  activateTab("connectors");
  loadConnectors(t.view);
}
async function loadConnectors(view){
  const box=view.querySelector(".connlist");
  box.innerHTML='<div style="color:var(--muted);padding:8px">正在探测连接器健康…</div>';
  const resp=await call({cmd:"list_connectors"});
  const conns=(resp.ok&&resp.data.connectors)||[];
  box.innerHTML="";
  let online=0;
  conns.forEach(c=>{
    const st=c.status||{}; if(st.online) online++;
    const card=el("conn "+c.id+(st.online?"":" offline"));
    const toolChips=(c.tools||[]).map(t=>`<span class="tchip" data-act="runConnectorTool" data-tool="${esc(t)}" data-online="${st.online?'1':'0'}">🔧 ${esc(t)}</span>`).join("");
    const statusHtml=st.online?`<span class="dot on"><span class="b"></span>在线</span>`:`<span class="dot off"><span class="b"></span>离线</span>`;
    const metaHtml=st.online?`服务：${esc(st.service_name||c.service)} · 运行 ${fmtUptime(st.uptime_secs)} · 延迟 ${st.latency_ms||"?"}ms`:`服务未启动或不可达：请启动 ${esc(c.service)} (${esc(c.base_url)})`;
    card.innerHTML=`<div class="ch"><div class="ico">${c.id==='flow'?'🔀':c.id==='onto'?'◈':'📊'}</div>
      <div><div class="cn">${esc(c.name)}</div><div class="cs">${esc(c.service)} · ${esc(c.base_url)}</div></div>${statusHtml}</div>
      <div class="desc">${esc(c.description||"")}</div><div class="tools">${toolChips}</div><div class="meta">${metaHtml}</div>`;
    box.append(card);
  });
  document.getElementById("conn-badge").textContent = online+"/"+conns.length;
}
function fmtUptime(s){ if(!s) return "?"; if(s<3600) return Math.floor(s/60)+"分"; if(s<86400) return Math.floor(s/3600)+"时"; return Math.floor(s/86400)+"天"; }

// 点连接器工具 chip：新开一个会话 tab 让 agent 触发该工具
async function runConnectorTool(tool, online){
  if(!online){ showToast("该连接器离线，无法调用。请先启动对应 cmx 服务。"); return; }
  const prompt = tool.startsWith("flow") ? "列出所有流程定义" : tool.startsWith("onto") ? "列出所有对象类型" : "列出所有报表";
  const id="task-"+Date.now();
  await openSession(id, prompt);
}

// ── 人在环审批（X4）：卡片点「允许/拒绝/本对话全部允许」→ 发 approve 命令唤醒挂起的回合 ──
async function approveTool(callId, approved, all){
  if(!callId) return;
  // 取该卡片所属会话 id（「本对话全部允许」需按会话授权）；兜底用当前 active 会话。
  const card=[...document.querySelectorAll(".tool.approval")].find(c=>c.dataset.callid===callId);
  const sid=(card&&card.dataset.sid)||CURRENT||"";
  // 立即禁用按钮 + 置「处理中」，避免重复点击
  document.querySelectorAll(".tool.approval").forEach(c=>{ if(c.dataset.callid===callId){
    const row=c.querySelector(".aprow"); if(row) row.innerHTML='<span class="appending">'+(all?'已允许，本对话后续不再询问…':'已提交，处理中…')+'</span>';
  }});
  try{ await call({cmd:"approve", call_id:callId, approved, all:!!all, session_id:sid}); }catch(e){}
}

// ── 会话历史：大会话只渲染最近一屏，更早的按需加载（减少解析/渲染/DOM，切换更快）──
const HIST_PAGE = 300;
// 把一段事件渲染进临时容器再整体搬入目标（一次性插入，避免逐条重排）。
function renderInto(target, sid, events){
  const tmp=document.createElement('div'); tmp._sb=null;
  // 历史回放无视 _skipUser（那是本地乐观渲染专用标志，残留会误吞历史首条用户消息）。
  events.forEach(ev=>{ const s=tmp._skipUser; tmp._skipUser=false; renderEvent(tmp, ev, sid); tmp._skipUser=s; (target._seqs=target._seqs||new Set()).add(ev.seq); });
  while(tmp.firstChild) target.append(tmp.firstChild);
}
function makeLoadMore(log, sid, start, total){
  const b=el("loadmore","↑ 加载更早的对话（共 "+total+" 条，已显示最近 "+(total-start)+" 条）");
  b.dataset.sid=sid; b.dataset.before=start;
  b.onclick=()=>loadEarlier(log, b);
  return b;
}function renderHistory(log, sid, events, start, total){
  const frag=document.createDocumentFragment();
  if(start>0) frag.append(makeLoadMore(log, sid, start, total));   // 上方还有更早的
  renderInto(frag, sid, events);
  log.innerHTML=""; log.append(frag);
}
async function loadEarlier(log, btn){
  const sid=btn.dataset.sid, before=parseInt(btn.dataset.before||"0",10);
  btn.textContent="加载中…";
  const resp=await call({cmd:"get_events",session_id:sid,limit:HIST_PAGE,before});
  const d=(resp&&resp.ok&&resp.data)||{}; const evs=d.events||[];
  const prevH=log.scrollHeight, prevTop=log.scrollTop;   // 锚点：插入更早内容后维持视口
  const frag=document.createDocumentFragment();
  renderInto(frag, sid, evs);
  btn.after(frag);
  if((d.start||0)>0){ btn.dataset.before=d.start; btn.textContent="↑ 加载更早的对话（共 "+d.total+" 条）"; }
  else btn.remove();
  log.scrollTop=prevTop+(log.scrollHeight-prevH);
}

// ── 会话：作为一个 tab 打开（每 tab 独立 log+composer）──
async function openSession(sessionId, autoPrompt){
  const tabId="s:"+sessionId;
  let t=findTab(tabId);
  if(!t){
    t=openTab({id:tabId, kind:"session", title:sessionId, ico:"💬", sessionId});
    const node=document.getElementById("tpl-session").content.cloneNode(true);
    t.view.className="tabview";
    t.view.append(node);
    const inp=t.view.querySelector(".inp2");
    inp.addEventListener("keydown",e=>{ if(e.key==="Enter"&&!e.shiftKey){e.preventDefault();sendChatTab(t);} });
    inp.addEventListener("input",e=>autoGrow(e.target));
    t.view.querySelector(".tab-send").onclick=()=>sendChatTab(t);
    // 载入历史：只取最近一屏（大会话不再解析/渲染全量）；标题随响应返回，免再查 list_sessions。
    const log=t.view.querySelector(".log"); log.innerHTML="";
    const resp=await call({cmd:"get_events",session_id:sessionId,limit:HIST_PAGE});
    const d=(resp&&resp.ok&&resp.data)||{};
    renderHistory(log, sessionId, d.events||[], d.start||0, d.total||0);
    if(d.title) t.title=d.title||sessionId;
  }
  activateTab(tabId);
  // 打开会话后自动滚到最新消息（底部）：历史是在 tab 隐藏时渲染的，激活后才完成布局，
  // 故延到下一帧再滚（双 rAF 兜底图表/字体导致的二次布局）。
  const _log=t.view.querySelector(".log");
  if(_log) requestAnimationFrame(()=>{ _log.scrollTop=_log.scrollHeight;
    requestAnimationFrame(()=>{ _log.scrollTop=_log.scrollHeight; }); });
  refreshTasks();
  if(autoPrompt){ await doSendTab(t, autoPrompt); }
  else { const inp=t.view.querySelector(".inp2"); if(inp) inp.focus(); }
}

async function doSendTab(t, text){
  const log=t.view.querySelector(".log");
  // 1) 立即乐观渲染用户气泡，并让流里的 user_message 事件跳过一次，避免重复。
  renderEvent(log, {kind:"user_message", text}, t.sessionId);
  log._skipUser = true;
  // 2) 打字等待指示器：立即出现，覆盖「发送→首个 token」的等待。
  showTyping(log);
  await streamSend(t.sessionId, text, (ev)=>{
    if(ev.kind==="stream_done"){ hideTyping(log); return; }
    if(ev.kind==="stream_error"){ hideTyping(log); renderEvent(log,{kind:"model_message",text:"⚠ "+(ev.message||"错误")}, t.sessionId); return; }
    // 有可见输出（文字流/工具卡/模型消息）到达 → 先撤等待动画，再渲染。
    if(ev.kind==="text_delta" || ev.kind==="tool_invoked" || ev.kind==="model_message") hideTyping(log);
    renderEvent(log, ev, t.sessionId);
    // 工具刚出结果 → 模型将继续思考，重新显示等待动画（这是之前缺失的多步等待提示）。
    if(ev.kind==="tool_result") showTyping(log);
    else if(ev.kind==="turn_ended" || ev.kind==="approval_requested") hideTyping(log);
  });
  hideTyping(log);
  const m=(await call({cmd:"list_sessions"})).data.sessions.find(x=>x.id===t.sessionId);
  if(m){ t.title=m.title||t.sessionId; renderTabs(); }
  refreshTasks();
}
async function sendChatTab(t){
  const box=t.view.querySelector(".inp2"); const text=box.value.trim(); if(!text) return;
  box.value=""; autoGrow(box); await doSendTab(t, text);
}
async function startFromHome(){
  const box=document.getElementById("inp");
  const text=box.value.trim(); if(!text) return;
  box.value=""; autoGrow(box);
  const id="task-"+Date.now();
  await openSession(id, text);
}
// 文本域自动增高（随内容 26→200px）
function autoGrow(el){ el.style.height="auto"; el.style.height=Math.min(el.scrollHeight,200)+"px"; }
// 语音输入（Web Speech API：Chrome / WKWebView 支持；把语音转文本填入当前输入框）
function toggleVoice(btn){
  if(!btn) return;
  if(window._rec){ try{ window._rec.stop(); }catch(e){} window._rec=null; btn.classList.remove("recording"); return; }
  const SR = window.SpeechRecognition || window.webkitSpeechRecognition;
  if(!SR){ showToast("此环境不支持语音识别（需 Web Speech API）"); return; }
  const input = document.querySelector(".tabview.active .inp2") || document.getElementById("inp");
  if(!input){ return; }
  const base = input.value;
  const rec = new SR(); window._rec=rec;
  rec.lang="zh-CN"; rec.interimResults=true; rec.continuous=false;
  rec.onresult=(e)=>{ let t=""; for(let i=0;i<e.results.length;i++) t+=e.results[i][0].transcript;
    input.value=(base?base+" ":"")+t; input.dispatchEvent(new Event("input")); };
  rec.onerror=(e)=>{ btn.classList.remove("recording"); window._rec=null; showToast("语音识别出错："+((e&&e.error)||"")); };
  rec.onend=()=>{ btn.classList.remove("recording"); window._rec=null; };
  try{ rec.start(); btn.classList.add("recording"); showToast("🎙 正在聆听…（再点停止）"); }
  catch(err){ btn.classList.remove("recording"); window._rec=null; showToast("无法启动麦克风"); }
}
document.getElementById("inp").addEventListener("keydown",e=>{ if(e.key==="Enter"&&!e.shiftKey){e.preventDefault();startFromHome();} });
document.getElementById("inp").addEventListener("input",e=>autoGrow(e.target));
