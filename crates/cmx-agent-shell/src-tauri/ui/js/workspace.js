// ── 输入区：工作空间选择、@ 文件、/ 技能 ──
// 交互对齐 opencode/codex：触发符只在光标 token 内生效；悬浮列表支持↑↓/⏎/Tab/Esc；
// 选择后替换触发词并保留其余输入。工作空间真源在后端 workspaces.json。
let WORKSPACE_STATE = { current:null, workspaces:[] };

async function refreshWorkspaces(){
  try{
    const r=await call({cmd:"list_workspaces"});
    if(r.ok) WORKSPACE_STATE=r.data;
  }catch(e){ WORKSPACE_STATE={current:null,workspaces:[]}; }
  renderWorkspaceState();
}

function renderWorkspaceState(){
  const label=document.getElementById("workspace-label"), btn=document.getElementById("workspace-btn");
  if(!label||!btn) return;
  const c=WORKSPACE_STATE.current;
  // default 托管空间即「任务模式」：不使用工作空间 = 用默认空间（后端 select(null) 真切到 default）。
  const isTask=!c||c.id==="default";
  label.textContent=isTask?"工作空间：不使用":("工作空间："+(c.kind==="local"?"本地 · "+c.name:c.name));
  btn.title=isTask
    ? "任务模式：使用内置默认空间（"+((c&&c.path)||"")+"）"
    : ("工作空间根："+c.path);
}

function closeWorkspaceMenu(){
  const m=document.getElementById("workspace-menu");
  if(m){ m.hidden=true; m.classList.remove("on"); }
}

function openWorkspaceMenu(){
  const btn=document.getElementById("workspace-btn"), m=document.getElementById("workspace-menu");
  if(!btn||!m) return;
  const r=btn.getBoundingClientRect();
  m.style.left=Math.max(12,Math.min(r.left,window.innerWidth-370))+"px";
  m.style.bottom=(window.innerHeight-r.top+8)+"px";
  m.style.top="auto";
  m.hidden=false; m.classList.add("on");
  const input=document.getElementById("workspace-search"); input.value=""; renderWorkspaceList("");
  // WORKSPACE_STATE 只在加载时拉一次，之后侧栏/其他入口新增的空间它看不见（曾表现为
  // 侧栏有空间、下拉却「没有工作空间」）——每次展开都强制重拉，先照旧渲染再就绪后重画。
  refreshWorkspaces().then(()=>{ if(!m.hidden) renderWorkspaceList(input.value); });
  setTimeout(()=>input.focus(),0);
}

function renderWorkspaceList(query){
  const box=document.getElementById("workspace-list"); if(!box) return;
  box.innerHTML="";
  const q=query.trim().toLowerCase();
  // default 托管空间即任务模式，不进空间列表（入口是下方「不使用工作空间」）。
  const items=WORKSPACE_STATE.workspaces
    .filter(w=>w.id!=="default")
    .filter(w=>!q||w.name.toLowerCase().includes(q)||w.path.toLowerCase().includes(q));
  if(!items.length) box.append(el("fm-empty","没有工作空间——新建或打开本地文件夹"));
  items.forEach(w=>{
    const item=el("fm-item"+(WORKSPACE_STATE.current&&WORKSPACE_STATE.current.id===w.id?" selected":""));
    item.innerHTML=`<span class="wi">${w.kind==="local"?"📂":"🗂"}</span><span class="fm-main"><span class="fm-title">${esc(w.name)}</span><span class="fm-sub">${esc(w.path)}</span></span><span class="check">✓</span>`;
    item.onclick=async()=>{ await selectWorkspace(w.id); closeWorkspaceMenu(); };
    box.append(item);
  });
}

async function selectWorkspace(id){
  try{
    const r=await call({cmd:"select_workspace",id});
    if(r.ok){ WORKSPACE_STATE=r.data; renderWorkspaceState(); }
    else showToast("切换工作空间失败："+(r.error?.message||"未知错误"));
  }catch(e){ showToast("切换工作空间失败："+(e&&e.message||e)); }
}

// 「打开本地文件夹」两壳都走系统原生选择器：Tauri 壳 invoke 自己的命令；
// Web 壳（浏览器）由本机后端进程调起系统选择器（协议命令 pick_local_directory），
// 选完拿绝对路径仍走 add_local_workspace 校验，与手填路径表单同一落点。
async function pickLocalWorkspaceFolder(){
  const invoke=window.__TAURI__?.core?.invoke;
  const picked = invoke
    ? await invoke("pick_local_directory")
    : await webPickLocalDirectory();
  if(!picked) return true; // 用户取消属于“已处理”，不应再弹出路径表单。
  return await addLocalWorkspace(picked, null);
}

async function webPickLocalDirectory(){
  const r=await call({cmd:"pick_local_directory"});
  if(!r.ok) throw new Error(r.error?.message||"打开文件夹选择器失败");
  return r.data?.picked ?? null;
}

async function addLocalWorkspace(path, name){
  try{
    const r=await call({cmd:"add_local_workspace",path,name:name||null});
    if(r.ok){
      WORKSPACE_STATE=r.data; renderWorkspaceState();
      showToast("已使用本地文件夹"); closeWorkspaceMenu();
      return true;
    }
    showToast(r.error?.message||"添加本地文件夹失败");
    return false;
  }catch(e){ showToast("添加本地文件夹失败："+(e&&e.message||e)); return false; }
}

function showWorkspaceForm(kind){
  const f=document.getElementById("workspace-form"); if(!f) return;
  f.hidden=false;
  f.innerHTML=kind==="create"
    ? `<label>工作空间名称</label><input id="ws-name" placeholder="例如：季度报表"><div class="form-ops"><button data-ws-cancel>取消</button><button class="primary" data-ws-submit>创建</button></div>`
    : `<label>本地文件夹路径</label><input id="ws-path" placeholder="例如：E:\\projects\\demo"><label class="mt">显示名称（可选）</label><input id="ws-name" placeholder="默认取文件夹名"><div class="form-ops"><button data-ws-cancel>取消</button><button class="primary" data-ws-submit>使用此文件夹</button></div>`;
  f.dataset.kind=kind;
  f.querySelector("[data-ws-cancel]").onclick=()=>{ f.hidden=true; };
  f.querySelector("[data-ws-submit]").onclick=async()=>{
    const name=f.querySelector("#ws-name").value.trim();
    if(kind==="create"){
      try{
        const r=await call({cmd:"create_workspace",name});
        if(r.ok){ WORKSPACE_STATE=r.data; renderWorkspaceState(); f.hidden=true; showToast("已创建工作空间"); closeWorkspaceMenu(); }
        else showToast(r.error?.message||"创建工作空间失败");
      }catch(e){ showToast("创建工作空间失败："+(e&&e.message||e)); }
    }else{
      if(await addLocalWorkspace(f.querySelector("#ws-path").value.trim(), name||null)) f.hidden=true;
    }
  };
  (f.querySelector(kind==="create"?"#ws-name":"#ws-path")).focus();
}

function initWorkspaceUI(){
  const btn=document.getElementById("workspace-btn");
  if(btn) btn.onclick=e=>{ e.stopPropagation(); const m=document.getElementById("workspace-menu"); (m.hidden?openWorkspaceMenu():closeWorkspaceMenu()); };
  const search=document.getElementById("workspace-search");
  if(search) search.addEventListener("input",()=>renderWorkspaceList(search.value));
  document.querySelectorAll("[data-ws-action]").forEach(b=>{
    b.onclick=async()=>{
      const a=b.dataset.wsAction;
      if(a==="create") showWorkspaceForm("create");
      else if(a==="local"){
        let handled=false;
        try{ handled=await pickLocalWorkspaceFolder(); }
        catch(e){ showToast("打开文件夹选择器失败："+(e&&e.message||e)); }
        if(!handled) showWorkspaceForm("local");
      }
      else { await selectWorkspace(null); closeWorkspaceMenu(); }
    };
  });
  document.addEventListener("click",e=>{
    const m=document.getElementById("workspace-menu");
    if(m&&!m.hidden&&!e.target.closest("#workspace-menu,#workspace-btn")) closeWorkspaceMenu();
  });
  document.addEventListener("keydown",e=>{ if(e.key==="Escape") closeWorkspaceMenu(); });
  refreshWorkspaces();
}

// ── @ / 悬浮选择（复刻 opencode PromptPopover）──
// @ 文件行 = 目录灰字 + 文件名亮字单行；/ 技能行 = /名称 + 描述 + 「技能」badge。
// 键盘 ↑↓/⏎/Tab/Esc；指针悬停即设 active（类切换，不重渲列表）。
let POP={open:false,type:null,items:[],active:0,inp:null,start:0,query:"",loading:false,token:0};
let MENTION_TIMER=null;

function detectTrigger(inp){
  const cur=inp.selectionStart||0, before=inp.value.slice(0,cur);
  const at=before.match(/(^|\s)(@)([^\s@]*)$/);
  if(at) return {type:"at",trigger:at[2],query:at[3],start:cur-at[3].length-1};
  // / 与 @ 同款规则（2026-09-18 反馈）：行首或空白字符之后都弹提示窗；紧贴文字的「和/或」、
  // 路径式「/a/b」（前一段非空白）不弹。start=「/」所在下标，selectPopoverItem 按它替换。
  const slash=before.match(/(^|\s)\/([^\s/]*)$/);
  if(slash){
    const atPos=(slash.index||0)+slash[1].length;
    return {type:"slash",trigger:"/",query:slash[2],start:atPos};
  }
  return null;
}

function ensurePopover(inp){
  let p=document.getElementById("input-popover");
  if(!p){
    p=el("floating-menu input-popover"); p.id="input-popover"; p.hidden=true;
    p.addEventListener("mousedown",e=>e.preventDefault());   // 点选项不抢输入框焦点
    document.body.append(p);
  }
  const r=inp.getBoundingClientRect();
  p.style.left=r.left+"px"; p.style.width=Math.max(r.width,360)+"px";
  p.style.bottom=(window.innerHeight-r.top+8)+"px"; p.style.top="auto";
  return p;
}

function closePopover(){
  const p=document.getElementById("input-popover");
  if(p){ p.hidden=true; p.classList.remove("on"); }   // .on 控制 opacity/pointer-events，缺它浮窗恒透明
  POP.open=false;
}

function scrollPopActive(){
  const p=document.getElementById("input-popover"); if(!p) return;
  const act=p.querySelector(".fm-item.active");
  if(act) act.scrollIntoView({block:"nearest"});
}
// 悬停/键盘共用：只切类，不重渲（避免 DOM 重建闪烁）
function setPopActive(i){
  if(i===POP.active) return;
  POP.active=i;
  const p=document.getElementById("input-popover"); if(!p) return;
  p.querySelectorAll(".fm-item").forEach((elm,idx)=>elm.classList.toggle("active",idx===i));
  scrollPopActive();
}

// @ 行：把 path 拆成「目录灰字 + 文件名亮字」（对齐 opencode file 行）
function splitPath(path){
  const cut=Math.max(path.lastIndexOf("/"),path.lastIndexOf("\\"));
  return cut>=0 ? {dir:path.slice(0,cut+1), leaf:path.slice(cut+1)||path} : {dir:"", leaf:path};
}

function renderPopover(){
  const p=ensurePopover(POP.inp);
  p.innerHTML="";
  if(!POP.items.length){
    const message=POP.loading?"正在搜索文件…"
      :POP.type==="at"?(POP.query?"没有匹配文件"
        :(WORKSPACE_STATE.current?"工作空间根目录——输入文件名关键字深搜":"先在下方 🗂 选择工作空间，即可浏览 / 搜索文件"))
      :"没有匹配技能";
    p.append(el("fm-empty",message));
  }
  let lastGroup=null;
  POP.items.forEach((item,i)=>{
    if(item.group && item.group!==lastGroup){
      lastGroup=item.group;
      p.append(el("fm-group",esc(item.group)));
    }
    const row=el("fm-item"+(i===POP.active?" active":""));
    if(item.invalid) row.title=item.invalid;
    if(POP.type==="at"){
      const isDir=item.type==="dir";
      const seg=isDir?{dir:"",leaf:item.name||item.path}:splitPath(item.path||item.name||"");
      row.innerHTML=`<span class="wi">${isDir?"📂":"📄"}</span>`
        +`<span class="fm-line">${seg.dir?`<span class="fm-dir">${esc(seg.dir)}</span>`:""}<span class="fm-leaf">${esc(seg.leaf)}</span></span>`
        +(item.description?`<span class="fm-desc">${esc(item.description)}</span>`:"");
    }else{
      const icon=item.icon||(item.group==="子智能体"?"🤖":item.group==="命令"?"⌘":"⚡");
      row.innerHTML=`<span class="wi">${icon}</span>`
        +`<span class="fm-line"><span class="fm-leaf${item.invalid?" invalid":""}">/${esc(item.name)}</span></span>`
        +`<span class="fm-desc">${esc(item.description||item.title||"")}</span>`
        +`<span class="fm-badge">${esc(item.group||"技能")}</span>`;
    }
    row.addEventListener("pointermove",()=>setPopActive(i));
    row.onclick=()=>selectPopoverItem(item);
    p.append(row);
  });
  p.hidden=false; p.classList.add("on"); POP.open=true;   // floating-menu 基类默认透明，必须加 .on 才可见
  scrollPopActive();
}

function selectPopoverItem(item){
  const inp=POP.inp, cur=inp.selectionStart||0;
  const prefix=inp.value.slice(0,POP.start), suffix=inp.value.slice(cur);
  // 文件用相对路径；技能条目只有 name（list_skills 不发 path）
  const token=(POP.type==="at"?item.path:item.name)||"";
  const insert=(POP.type==="at"?"@":"/")+token+" ";
  inp.value=prefix+insert+suffix;
  const pos=(prefix+insert).length;
  inp.setSelectionRange(pos,pos); inp.focus(); closePopover(); inp.dispatchEvent(new Event("input"));
}

async function queryTrigger(inp,tr){
  const token=++POP.token;
  POP.type=tr.type; POP.inp=inp; POP.start=tr.start; POP.query=tr.query;
  POP.active=0; POP.items=[];
  if(tr.type==="at"){
    // 空 query 也发后端：列出工作空间根目录一层（opencode 空 query 出最近/根文件的同款体验）
    if(!WORKSPACE_STATE.current){ POP.loading=false; renderPopover(); return; }
    POP.loading=true; renderPopover();
    try{
      // 带上活动会话 id：后端按「会话所属空间」检索（跨空间任务的 @ 提示不能串到别的空间）；
      // 首页输入框 CURRENT 为空 → 后端用当前空间，与新建任务的归属一致。
      const r=await call({cmd:"search_workspace_files",query:tr.query,limit:50,session_id:CURRENT||null});
      if(token!==POP.token) return;
      POP.items=(r.ok&&r.data.files)||[];
      if(!r.ok) showToast(r.error?.message||"文件检索失败");
    }catch(e){ if(token===POP.token) showToast("文件检索失败："+(e&&e.message||e)); }
    finally{ if(token===POP.token) POP.loading=false; }
  }else{
    // 斜杠菜单三分（姊妹方案 §2.3/§2.4）：命令/技能/子智能体一次拉全，前端只做跨组过滤。
    const r=await call({cmd:"list_slash"});
    const d=(r.ok&&r.data)||{};
    const all=[
      ...(d.commands||[]).map(c=>({group:"命令",icon:"⌘",name:c.name,description:c.description})),
      ...(d.skills||[]).map(s=>({group:"技能",icon:"⚡",name:s.name,description:s.description,invalid:s.invalid})),
      ...(d.subagents||[]).map(a=>({group:"子智能体",icon:"🤖",name:a.name,description:a.description||a.title})),
    ];
    const q=tr.query.toLowerCase();
    POP.items=all.filter(s=>!q||s.name.toLowerCase().includes(q)||(s.description||"").toLowerCase().includes(q));
  }
  renderPopover();
}

function attachComposer(inp){
  if(!inp||inp._composerAttached) return;
  inp._composerAttached=true;
  inp.addEventListener("keydown",e=>{
    if(!POP.open) return;
    if(e.key==="ArrowDown"||e.key==="ArrowUp"){
      e.preventDefault(); e.stopImmediatePropagation();
      if(POP.items.length) setPopActive((POP.active+(e.key==="ArrowDown"?1:-1)+POP.items.length)%POP.items.length);
    }else if(e.key==="Enter"||e.key==="Tab"){
      if(POP.items.length){ e.preventDefault(); e.stopImmediatePropagation(); selectPopoverItem(POP.items[POP.active]); }
      else closePopover();
    }else if(e.key==="Escape"){ e.preventDefault(); e.stopImmediatePropagation(); closePopover(); }
  },true);
  inp.addEventListener("input",()=>{
    clearTimeout(MENTION_TIMER);
    const tr=detectTrigger(inp);
    if(!tr){ closePopover(); } 
    syncComposerChip(inp);
    if(!tr) return;
    MENTION_TIMER=setTimeout(()=>queryTrigger(inp,tr),110);
  });
  inp.addEventListener("blur",()=>setTimeout(()=>closePopover(),120));
  inp.addEventListener("click",()=>{ const tr=detectTrigger(inp); if(!tr) closePopover(); });
}

function bindComposerButtons(){
  document.querySelectorAll(".composer .tool-ic").forEach(btn=>{
    const title=btn.getAttribute("title")||"";
    if(!title.includes("引用文件")&&!title.includes("调用技能")) return;
    if(btn._mentionBound) return; btn._mentionBound=true;
    btn.onclick=()=>{
      const input=btn.closest(".composer")?.querySelector("textarea")||document.querySelector(".tabview.active .inp2")||document.getElementById("inp");
      if(!input) return;
      input.focus();
      const cur=input.selectionStart||input.value.length;
      input.value=input.value.slice(0,cur)+(title.includes("引用文件")?"@":"/")+input.value.slice(cur);
      input.setSelectionRange(cur+1,cur+1); input.dispatchEvent(new Event("input"));
    };
  });
}

// ── 斜杠命令 chip（ZCode 图二 2026-09-18）：输入框文本是「/命令 [+参数]」且命令在 list_slash
// 清单里时，整体变「⌘ 命令 ✕ | 参数」chip 形态；textarea 隐藏但 value 持续同步，发送链路不变。
let SLASH_NAMES=null;
async function slashNames(){
  if(SLASH_NAMES) return SLASH_NAMES;
  SLASH_NAMES={};
  try{
    const r=await call({cmd:"list_slash"}); const d=(r.ok&&r.data)||{};
    (d.commands||[]).forEach(c=>{ SLASH_NAMES[c.name]={icon:c.icon||"⌘"}; });
    (d.skills||[]).forEach(s=>{ if(!s.invalid) SLASH_NAMES[s.name]={icon:"⚡"}; });
    (d.subagents||[]).forEach(a=>{ SLASH_NAMES[a.name]={icon:"🤖"}; });
  }catch(e){ /* 清单取不到就不变 chip，按普通文本发送 */ }
  return SLASH_NAMES;
}
function syncComposerChip(inp){
  const m=(inp.value||"").match(/^\/([^\s/]+)(?:\s([\s\S]*))?$/);
  if(inp._chip){
    // 已在 chip 态：命令名被改掉/文本不再匹配 → 拆回输入框
    if(!m||!SLASH_NAMES||!SLASH_NAMES[m[1]]) exitChipMode(inp,false);
    return;
  }
  if(!m) return;
  const name=m[1];
  slashNames().then(names=>{
    if(inp._chip||!document.contains(inp)||!names[name]) return;
    if(document.activeElement===inp) enterChipMode(inp,name);   // 失焦状态下不抢焦点变形
  });
}
function enterChipMode(inp,name){
  inp._chip={name};
  inp.style.display="none";
  let row=inp.parentNode.querySelector(".cmd-chip-row");
  if(!row){ row=document.createElement("div"); row.className="cmd-chip-row"; inp.parentNode.insertBefore(row,inp); }
  const meta=SLASH_NAMES[name]||{};
  row.innerHTML=`<span class="cmd-chip"><span class="ci">${esc(meta.icon||"⌘")}</span><span class="cn">${esc(name)}</span>`
    +`<span class="cx" title="取消命令（Esc）">✕</span></span><input class="chip-inp" placeholder="参数，可留空">`;
  const args=row.querySelector(".chip-inp");
  args.addEventListener("input",()=>{ inp.value="/"+name+(args.value?" "+args.value:""); });
  args.addEventListener("keydown",e=>{
    if(e.key==="Backspace"&&!args.value){ e.preventDefault(); exitChipMode(inp,true); }
    else if(e.key==="Escape"){ e.preventDefault(); exitChipMode(inp,true); }
    else if(e.key==="Enter"&&!e.shiftKey){
      e.preventDefault();
      // 复用输入框既有的 Enter 发送绑定（会话页/首页各自已挂 keydown）
      inp.dispatchEvent(new KeyboardEvent("keydown",{key:"Enter",cancelable:true}));
    }
  });
  row.querySelector(".cx").addEventListener("click",()=>{
    inp.value="";                       // ✕ = 删除命令（含参数），不是退回编辑态（Esc/退格才是）
    exitChipMode(inp,true);
  });
  args.focus();
}
function exitChipMode(inp,focus){
  inp._chip=null;
  const row=inp.parentNode&&inp.parentNode.querySelector(".cmd-chip-row");
  if(row) row.remove();
  inp.style.display="";
  if(focus){ inp.focus(); const n=inp.value.length; try{ inp.setSelectionRange(n,n); }catch(e){} }
}

document.addEventListener("DOMContentLoaded",()=>{ initWorkspaceUI(); attachComposer(document.getElementById("inp")); bindComposerButtons(); });

// ── 上下文用量圆环 + 明细卡（压缩方案 §4.4.2，ZCode 同款；2026-09-18 拍板）──
// 入口=会话 composer 工具行模型按钮左侧 18px SVG 圆环（<80% 蓝 / ≥80% 金 / ≥95% 红）；
// 点开明细卡：容量（万单位）+ 分类分段条 + 占比行 + 缓存命中率（网关未回传整行隐藏）。
// 明细卡沿用 cmx-dd 定稿「挂 body + position:fixed」模式，杜绝 overflow 裁剪；
// scroll（捕获）/resize/点空白统一收起。数据源=get_context_usage（真实 usage 优先，
// 无则后端投影估算；UI 不标注来源——09-18 拍板）。取不到数据一律按 0% 显示
// （09-18 反馈：占位「—」与默认整圈蓝都不对）。
const RING_C = 2 * Math.PI * 7;

function ensureUsageRings(root){
  (root || document).querySelectorAll(".composer .crow").forEach(crow => {
    if (crow.querySelector(".ring-btn")) return;
    const model = crow.querySelector(".model");
    if (!model) return;
    const btn = document.createElement("button");
    btn.className = "ring-btn";
    btn.innerHTML = '<svg viewBox="0 0 18 18" width="18" height="18" aria-hidden="true">'
      + '<circle class="rb-bg" cx="9" cy="9" r="7"></circle>'
      + '<circle class="rb-fg" cx="9" cy="9" r="7"></circle></svg>'
      + '<span class="tip">上下文 0%</span>';
    crow.insertBefore(btn, model);
    btn.addEventListener("click", e => { e.stopPropagation(); toggleUsagePop(btn); });
  });
}

function fmtWan(n){
  if (n == null) return "—";
  if (n >= 10000) {
    const v = n / 10000;
    return (v >= 100 ? Math.round(v) : Math.round(v * 10) / 10) + "万";
  }
  return String(n);
}

const USAGE_CATS = [
  ["messages", "消息", "--blue"],
  ["tool_results", "工具结果", "--green"],
  ["tool_defs", "工具定义", "--violet"],
  ["system", "系统提示词", "--gold"],
  ["other", "其他", "--muted"],
];

let _usagePop = null;

function usagePop(){
  if (_usagePop) return _usagePop;
  _usagePop = el("usage-pop");
  _usagePop.id = "usage-pop";
  _usagePop.hidden = true;
  document.body.append(_usagePop);
  document.addEventListener("scroll", hideUsagePop, true);
  window.addEventListener("resize", hideUsagePop);
  document.addEventListener("click", e => {
    if (_usagePop && !_usagePop.hidden && !_usagePop.contains(e.target)
      && !(e.target.closest && e.target.closest(".ring-btn"))) hideUsagePop();
  });
  return _usagePop;
}

function hideUsagePop(){ if (_usagePop) { _usagePop.hidden = true; _usagePop._btn = null; } }

function toggleUsagePop(btn){
  const p = usagePop();
  if (!p.hidden && p._btn === btn) { hideUsagePop(); return; }
  p._btn = btn;
  p.hidden = false;
  p.innerHTML = '<div class="up-loading">读取中…</div>';
  const r = btn.getBoundingClientRect();
  const pw = 264;
  // top=底缘（CSS translateY(-100%)）：内容变高向上生长，永不下探盖住 composer
  p.style.top = Math.max((p.offsetHeight || 60) + 8, r.top - 8) + "px";
  p.style.left = Math.max(8, Math.min(window.innerWidth - pw - 8, r.right - pw)) + "px";
  refreshUsage(CURRENT);
}

async function refreshUsage(sid){
  let d = null;
  if (sid) {
    try {
      const r = await call({ cmd: "get_context_usage", session_id: sid });
      if (r && r.ok && r.data) d = r.data;
    } catch (e) { /* 取不到按 0 显示 */ }
  }
  paintUsageRings(d);
  if (d && _usagePop && !_usagePop.hidden) paintUsagePopBody(d);
}

function paintUsageRings(d){
  const pct = d ? Math.max(0, Math.min(100, d.pct || 0)) : 0;
  document.querySelectorAll(".ring-btn").forEach(btn => {
    const fg = btn.querySelector(".rb-fg");
    if (fg) {
      fg.style.strokeDasharray = RING_C.toFixed(2);
      fg.style.strokeDashoffset = (RING_C * (1 - pct / 100)).toFixed(2);
    }
    btn.classList.toggle("warn", pct >= 80 && pct < 95);
    btn.classList.toggle("crit", pct >= 95);
    const tip = btn.querySelector(".tip");
    if (tip) tip.textContent = "上下文 " + pct + "%";
  });
}

function paintUsagePopBody(d){
  const p = _usagePop; if (!p) return;
  const total = d.breakdown && d.breakdown.total || 1;
  // 分段条按「占容量」铺宽（ZCode 同款）：彩色总宽=used/容量，剩余留灰底轨；
  // 下方占比行仍是各分类占 used 的份额，两者语义不同。
  const cap = d.usable || d.window || total;
  const segs = USAGE_CATS.map(([k, , color]) => {
    const v = (d.breakdown && d.breakdown[k]) || 0;
    const w = v / cap * 100;
    return `<i style="width:${w}%;background:var(${color})"></i>`;
  }).join("");
  const rows = USAGE_CATS.map(([k, label, color]) => {
    const v = (d.breakdown && d.breakdown[k]) || 0;
    const w = total > 0 ? Math.round(v / total * 1000) / 10 : 0;
    return `<div class="ucat"><span class="dot" style="background:var(${color})"></span>`
      + `<span>${label}</span><b>${w}%</b></div>`;
  }).join("");
  const cache = d.cached_input != null
    ? (() => {
        const rate = d.used > 0 ? Math.round(d.cached_input / d.used * 1000) / 10 : 0;
        return `<div class="cache"><span>平均缓存命中率</span><b>${rate}%</b></div>`;
      })()
    : "";
  p.innerHTML = `<div class="cap"><h5>上下文容量</h5>`
    + `<span class="num">${fmtWan(d.used)} / ${fmtWan(d.window)}（${d.pct}%）</span></div>`
    + `<div class="segbar">${segs}</div>${rows}${cache}`;
}
