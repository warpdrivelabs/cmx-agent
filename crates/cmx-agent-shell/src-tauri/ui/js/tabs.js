// ══════════════ 多 tab 系统 ══════════════
// 每个 tab 保留独立 DOM 状态（像浏览器）。TABS 里存 {id,kind,title,ico,view,extra}
// kind: "session"（会话，view 含独立 log+composer）/ "connectors"（连接器卡片）
let TABS = [];
let ACTIVE = null;      // 当前 active tab 的 id
let CURRENT = null;     // 当前 active 的会话 id（供任务列表高亮/发送；非会话 tab 时为 null）
// 本地正在流式的会话 id 集合。全局 session_event 监听据此跳过本地回合，避免与 streamSend 双重渲染。
// IM 来源会话永不在其中 → 走 session_event 实时通道。
let STREAMING = new Set();
const tabviews = ()=>document.getElementById("tabviews");
const homeEl = ()=>document.getElementById("home");

function findTab(id){ return TABS.find(t=>t.id===id); }

// 渲染顶部 tab 条
function renderTabs(){
  const strip=document.getElementById("tabstrip"); strip.innerHTML="";
  TABS.forEach(t=>{
    const el=document.createElement("div");
    el.className="tab"+(t.id===ACTIVE?" active":"");
    el.dataset.tabid=t.id;
    el.innerHTML=`<span class="tab-ico">${t.ico}</span><span class="tab-t">${esc(t.title)}</span><span class="tab-x" title="关闭">✕</span>`;
    el.onclick=(e)=>{ if(e.target.classList.contains("tab-x")){ closeTab(t.id); } else { activateTab(t.id); } };
    el.oncontextmenu=(e)=>{ e.preventDefault(); openTabCtx(e, t.id); };
    strip.append(el);
  });
  // 显示逻辑：home(新建任务) 与 tabviews 按「是否有激活 tab」切换；tab 条只要有 tab 就显示。
  // 关键：ACTIVE==null 表示「新建任务」态——即使已有 tab，也显示 home 内容（tab 条仍在，便于切回）。
  const hasTabs=TABS.length>0;
  const inTab=ACTIVE!==null;
  homeEl().classList.toggle("off", inTab);
  tabviews().classList.toggle("on", inTab);
  document.getElementById("tabbar").style.display = hasTabs ? "" : "none";
}

// 激活某 tab：只显示它的 view
function activateTab(id){
  ACTIVE=id;
  TABS.forEach(t=>t.view.classList.toggle("active", t.id===id));
  const t=findTab(id);
  CURRENT = (t && t.kind==="session") ? t.sessionId : null;
  renderTabs();
  if(t && t.kind==="session"){ const inp=t.view.querySelector(".inp2"); if(inp) inp.focus(); }
}

// 打开或聚焦一个 tab
/** 打开或聚焦 tab。@param opts {{id,kind,title,ico?,sessionId?}} */
function openTab({id, kind, title, ico, sessionId}){
  let t=findTab(id);
  if(t){ activateTab(id); return t; }
  const view=document.createElement("div"); // 会被模板内容替换
  t={id, kind, title, ico, sessionId, view};
  TABS.push(t);
  tabviews().append(view);
  return t;
}

// 关闭一个 tab
function closeTab(id){
  const idx=TABS.findIndex(t=>t.id===id); if(idx<0) return;
  const t=TABS[idx]; t.view.remove(); TABS.splice(idx,1);
  if(ACTIVE===id){
    // 激活相邻 tab
    const next=TABS[idx] || TABS[idx-1];
    if(next) activateTab(next.id); else { ACTIVE=null; CURRENT=null; renderTabs(); document.getElementById("inp").focus(); }
  } else { renderTabs(); }
  refreshTasks();
}
function closeOthers(id){ TABS.filter(t=>t.id!==id).map(t=>t.id).forEach(closeTab); activateTab(id); }
function closeRight(id){ const idx=TABS.findIndex(t=>t.id===id); TABS.slice(idx+1).map(t=>t.id).forEach(closeTab); }
function closeAll(){ TABS.map(t=>t.id).forEach(closeTab); }

// tab 溢出下拉
function toggleTabOverflow(){
  const dd=document.getElementById("tab-dropdown");
  if(dd.classList.contains("on")){ dd.classList.remove("on"); return; }
  dd.innerHTML="";
  TABS.forEach(t=>{
    const it=el("tab-dd-item"+(t.id===ACTIVE?" active":""));
    it.innerHTML=`<span>${t.ico}</span><span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${esc(t.title)}</span><span class="ddx">✕</span>`;
    it.onclick=(e)=>{ if(e.target.classList.contains("ddx")){ closeTab(t.id); toggleTabOverflow(); } else { activateTab(t.id); dd.classList.remove("on"); } };
    dd.append(it);
  });
  if(TABS.length===0) dd.append(el("tab-dd-item","（没有打开的标签页）"));
  dd.classList.add("on");
}

// tab 右键上下文菜单
let CTX_TAB=null;
function openTabCtx(e, id){
  CTX_TAB=id;
  const m=document.getElementById("tabctx");
  m.style.left=e.clientX+"px"; m.style.top=e.clientY+"px";
  m.classList.add("on");
}
function closeTabCtx(){ document.getElementById("tabctx").classList.remove("on"); }
function tabCtxClose(){ closeTabCtx(); if(CTX_TAB) closeTab(CTX_TAB); }
function tabCtxCloseOthers(){ closeTabCtx(); if(CTX_TAB) closeOthers(CTX_TAB); }
function tabCtxCloseRight(){ closeTabCtx(); if(CTX_TAB) closeRight(CTX_TAB); }
function tabCtxCloseAll(){ closeTabCtx(); closeAll(); }

// 快捷动作（点击=预填输入框）。占位示例，贴合 WorkBuddy 品类。
const QUICK = ["📄 文档处理","💳 金融服务","📊 数据分析及可视化","🧰 个人工作台","🔍 深度研究"];
const quickEl = document.getElementById("quick");
QUICK.forEach(q=>{ const b=el("button","<span>"+q+"</span>"); b.className="qa"; b.title="点击预填指令（占位示例）";
  b.onclick=()=>{ document.getElementById("inp").value = q.replace(/^\S+\s/,"")+"："; document.getElementById("inp").focus(); };
  quickEl.append(b); });

// ── 侧栏会话列表（真实：list_sessions）──
