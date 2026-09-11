// ── 侧栏会话列表（从 tabs.js 挪入）──
async function refreshTasks(){
  const resp = await call({cmd:"list_sessions"});
  const list = (resp.ok && resp.data.sessions) || [];
  document.getElementById("taskcount").textContent = "("+list.length+")";
  const box = document.getElementById("tasklist"); box.innerHTML="";
  if(list.length===0){ box.append(el("empty-tasks","还没有任务。<br>点上方「新建任务」或在首页直接下达指令。")); return; }
  list.forEach(m=>{
    const t=el("task"+(m.id===CURRENT?" active":""));
    t.innerHTML = `<span class="tt">${esc(m.title||m.id)}</span><span class="tm">${ago(m.updated_at)}</span><span class="del" title="删除">✕</span>`;
    t.addEventListener("click", (e)=>{ if(e.target.closest(".del")) return; openSession(m.id); });
    t.querySelector(".del").onclick = async (e)=>{ e.stopPropagation(); await call({cmd:"delete_session",session_id:m.id});
      if(findTab("s:"+m.id)) closeTab("s:"+m.id); refreshTasks(); };
    box.append(t);
  });
}
function ago(iso){ const d=(Date.now()-new Date(iso).getTime())/86400000;
  if(d<1) return "今天"; if(d<2) return "昨天"; return Math.floor(d)+"天前"; }

// 每个回合一个 timeline item；后续事件都挂到当前卡片里。
function ensureTurn(log){
  // 历史分片还没挂到 document 时 isConnected 为 false；这时只看引用，避免一次回放被拆散。
  if(!log._turn || (!log._history && !log._turn.isConnected)){ log._turn=el("turn"); log.append(log._turn); }
  return log._turn;
}
// 会话事件渲染（renderEvent）真源在 render.js（opencode 式工具卡/思考行/上下文组）。
// ⚠ 本文件脚本晚于 render.js 加载——**禁止**在此重复定义 renderEvent，否则会静默覆盖新实现
// （曾因此历史回放调用已删除的 renderToolInvoke 抛异常，表现为点开任务页面卡死）。

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
  // 模板里的 .mlabel 是硬编码"未配置"（惰性 DOM，querySelectorAll 不到）→ 克隆后用当前值覆盖
  t.view.querySelectorAll(".model .mlabel").forEach(s=>s.textContent=_modelLabel);
  const inp=t.view.querySelector(".inp2");
  attachComposer(inp);
  bindComposerButtons();
  inp.addEventListener("keydown",e=>{ if(e.key==="Enter"&&!e.shiftKey){e.preventDefault();sendChatTab(t);} });
  inp.addEventListener("input",e=>autoGrow(e.target));
  t.view.querySelector(".tab-send").onclick=()=>sendChatTab(t);
  const log=t.view.querySelector(".log"); log.innerHTML=""; log._seqs=new Set(); log._sb=null;
  activateTab(tabId);
  scheduleRefreshTasks();
  // 后台补历史：把未渲染过的事件 prepend 到 log 顶部（历史在上、实时新事件在下），不擦除。
  call({cmd:"get_events",session_id:sessionId,limit:HIST_PAGE}).then(resp=>{
    const d=(resp&&resp.ok&&resp.data)||{}; const evs=d.events||[];
    const frag=document.createDocumentFragment();
    if((d.start||0)>0) frag.append(makeLoadMore(log, sessionId, d.start, d.total||0));
    const tmp=document.createElement('div'); tmp._sb=null; tmp._history=true;
    evs.forEach(ev=>{
      if(log._seqs.has(ev.seq)) return;
      const s=tmp._skipUser; tmp._skipUser=false; renderEvent(tmp, ev, sessionId); tmp._skipUser=s;
      while(tmp.firstChild) frag.append(tmp.firstChild);
      log._seqs.add(ev.seq);
    });
    settlePendingCards(frag);
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
  tmp._history=true;
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
  settlePendingCards(frag);              // 回放结束仍 pending 的卡=中断/截断，收敛避免永久转圈
  log.innerHTML=""; log._tools=new Map(); log._turn=null; log._typing=null;
  log._sb=null; log._raw=""; log._raf=null; log._rsb=null; log._rraw=""; log._rraf=null;
  log._rcard=null; log._ctx=null; log._intMarked=false;   // 全量渲染前清状态，防旧引用串场
  stopWorkDurTick(log); log._durRow=null; log._durTick=null; log._turnStartTs=null;  // 实时计时行一并清（防旧 interval 改新 DOM）
  log._closed=false;
  log.append(frag);
}
async function loadEarlier(log, btn){
  const sid=btn.dataset.sid, before=parseInt(btn.dataset.before||"0",10);
  btn.textContent="加载中…";
  const resp=await call({cmd:"get_events",session_id:sid,limit:HIST_PAGE,before});
  const d=(resp&&resp.ok&&resp.data)||{}; const evs=d.events||[];
  const prevH=log.scrollHeight, prevTop=log.scrollTop;   // 锚点：插入更早内容后维持视口
  const frag=document.createDocumentFragment();
  renderInto(frag, sid, evs);
  settlePendingCards(frag);
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
    t.view.querySelectorAll(".model .mlabel").forEach(s=>s.textContent=_modelLabel);
  const inp=t.view.querySelector(".inp2");
  attachComposer(inp);
  bindComposerButtons();
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
  scheduleRefreshTasks();
  if(autoPrompt){ await doSendTab(t, autoPrompt); }
  else { const inp=t.view.querySelector(".inp2"); if(inp) inp.focus(); }
}

async function doSendTab(t, text){
  const log=t.view.querySelector(".log");
  // 同会话等待队列：当前回合未结束时，后发消息按顺序排队，不并发写同一会话。
  if(t._busy){
    t._queue=t._queue||[];
    t._queue.push(text);
    queueNote(log,"⏳ 已加入会话等待队列（第 "+t._queue.length+" 条）");
    return;
  }
  t._busy=true;
  if(STREAMING && STREAMING.add) STREAMING.add(t.sessionId);
  setSessionBusy(t,true);
  // 取消标记：tab 关闭后不再渲染事件（streamSend 回调检查此 flag）
  let _cancelled = false;
  t._streamCancel = () => {
    _cancelled = true;
    if(t._streamAbort) t._streamAbort.abort();
  };
  // 1) 立即乐观渲染用户气泡，并让流里的 user_message 事件跳过一次，避免重复。
  renderEvent(log, {kind:"user_message", text}, t.sessionId);
  log._skipUser = true;
  // 2) 打字等待指示器：立即出现，覆盖「发送→首个 token」的等待。
  showTyping(log);
  t._streamAbort=new AbortController();
  try {
    await streamSend(t.sessionId, text, (ev)=>{
      if(_cancelled){
        // 已点中断：丢弃中途事件，但 turn_ended（后端权威收尾）照渲——负责「已中断」分隔条与关回合。
        if(ev.kind==="turn_ended") renderEvent(log, ev, t.sessionId);
        return;
      }
      if(ev.kind==="stream_done"){ hideTyping(log); return; }
      if(ev.kind==="stream_error"){ hideTyping(log); renderEvent(log,{kind:"model_message",text:"⚠ "+(ev.message||"错误")}, t.sessionId); return; }
      // 落库事件按 seq 去重并注册：与 session_event 通道共用 _seqs。流结束 STREAMING 放行后，
      // 总线迟到重播的同一条事件（如 reasoning 回执）若不在此注册，会被再渲一遍（重复思考卡）。
      if(ev.seq != null){
        log._seqs = log._seqs || new Set();
        if(log._seqs.has(ev.seq)) return;
        log._seqs.add(ev.seq);
      }
      // 有可见输出（文字流/工具卡/模型消息）到达 → 先撤等待动画，再渲染。
      if(ev.kind==="text_delta" || ev.kind==="tool_invoked" || ev.kind==="model_message") hideTyping(log);
      renderEvent(log, ev, t.sessionId);
      // 工具刚出结果 → 模型将继续思考，重新显示等待行（多步等待提示）。
      if(ev.kind==="tool_result") showTyping(log);
      else if(ev.kind==="turn_ended" || ev.kind==="approval_requested") hideTyping(log);
    });
  } catch(e) {
    if(!_cancelled){ closeCtxGroup(log); renderEvent(log,{kind:"note",text:"⚠ 连接中断："+(e&&e.message||e)},t.sessionId); }
  } finally {
    hideTyping(log);
    t._busy=false; setSessionBusy(t,false);
    if(STREAMING && STREAMING.delete) STREAMING.delete(t.sessionId);
  }
  const m=(await call({cmd:"list_sessions"})).data.sessions.find(x=>x.id===t.sessionId);
  if(m){ t.title=m.title||t.sessionId; renderTabs(); }
  scheduleRefreshTasks();
  const next=(t._queue||[]).shift();
  if(next && !_cancelled) doSendTab(t,next);
  else if(_cancelled && (t._queue||[]).length){
    t._queue=[]; queueNote(log,"🛑 已中断，等待队列已清空。");
  }
}
// 直接挂到 log 的提示行（回合已关闭时不新开一张空回合卡）
function queueNote(log, text){
  const host=log._turn||log;
  host.append(el("meta note",esc(text)));
  log.scrollTop=1e9;
}
async function sendChatTab(t){
  const box=t.view.querySelector(".inp2"); const text=box.value.trim(); if(!text) return;
  box.value=""; autoGrow(box); await doSendTab(t, text);
}
function setSessionBusy(t,busy){
  const btn=t.view.querySelector(".tab-send");
  if(!btn) return;
  btn.classList.toggle("stop",busy);
  // 停止方块用几何绘制：■ 字形在字体 em-box 内基线偏移，flex 居不住（视觉偏离按钮中心）
  btn.innerHTML=busy?'<span class="stop-ico"></span>':"<span>↑</span>";
  btn.title=busy?((t._queue&&t._queue.length)?("已排队 "+t._queue.length+" 条 · "):"")+"中断 ⎋":"发送 ⏎";
  btn.onclick=busy?()=>stopSession(t):()=>sendChatTab(t);
}
// 立即中断反馈：撤等待行、折叠思考、关上下文组、当前回合补「⎋ 已中断」分隔条并关闭。
// 后端随后到达的 turn_ended(stopped) 经 _intMarked 去重，不会画第二条。
function closeInterruptedTurn(log){
  hideTyping(log);
  if(log._raf){ cancelAnimationFrame(log._raf); log._raf=null; }
  if(log._rraf){ cancelAnimationFrame(log._rraf); log._rraf=null; }
  stopWorkDurTick(log);                                  // 停实时计时；turn_ended 后到时不重复定格
  if(log._durRow){ const t=log._durRow.querySelector(".wd-t"); if(t) t.textContent="已中断"; log._durRow=null; }
  log._closed=true; log._turnStartTs=null;               // 关回合：后到事件不再拉起新的计时行/回合卡
  closeCtxGroup(log);
  closeReasoning(log);
  log._sb=null;
  if(log._turn){
    log._turn.append(el("turn-divider int","⎋ 已中断"));
    log._intMarked=true;
    log._turn=null;
  }
}
async function stopSession(t){
  if(t._streamCancel) t._streamCancel();
  const log=t.view.querySelector(".log");
  if(log) closeInterruptedTurn(log);
  try{ await call({cmd:"cancel_session",session_id:t.sessionId}); showToast("已请求中断"); }
  catch(e){ showToast("中断请求失败："+(e&&e.message||e)); }
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

// Esc 中断当前会话（对齐 codex「esc to interrupt」）：浮窗/弹层/输入弹层开着时让位，只在会话 tab 忙时触发。
document.addEventListener("keydown",e=>{
  if(e.key!=="Escape") return;
  const pop=document.getElementById("input-popover");
  if(pop&&!pop.hidden) return;                                       // 输入弹层先关（attachComposer 已处理）
  if(document.querySelector(".floating-menu:not([hidden])")) return; // 工作空间菜单先关
  if(document.querySelector(".mcfg-overlay:not(.hidden)")) return;   // 模态框先关
  if(document.querySelector(".cmxmenu.on")) return;                  // 下拉菜单先关
  const t=TABS.find(x=>x.kind==="session"&&x.view.classList.contains("active"));
  if(t&&t._busy){ e.preventDefault(); stopSession(t); }
});

