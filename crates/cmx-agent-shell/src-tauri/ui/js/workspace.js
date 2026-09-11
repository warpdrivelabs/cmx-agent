// ── 输入区：工作空间选择、@ 文件、/ 技能 ──
// 交互对齐 opencode/codex：触发符只在光标 token 内生效；悬浮列表支持↑↓/⏎/Tab/Esc；
// 选择后替换触发词并保留其余输入。工作空间真源在后端 workspaces.json。
let WORKSPACE_STATE = { current:null, workspaces:[] };
let SKILL_CACHE = null;

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
  label.textContent=c?(c.kind==="local"?"本地 · "+c.name:c.name):"不使用工作空间";
  btn.title=c?("工作空间根："+c.path):"当前为普通任务模式，不绑定文件工作区";
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
  const input=document.getElementById("workspace-search"); input.value=""; renderWorkspaceList(""); setTimeout(()=>input.focus(),0);
}

function renderWorkspaceList(query){
  const box=document.getElementById("workspace-list"); if(!box) return;
  box.innerHTML="";
  const q=query.trim().toLowerCase();
  const items=WORKSPACE_STATE.workspaces.filter(w=>!q||w.name.toLowerCase().includes(q)||w.path.toLowerCase().includes(q));
  if(!items.length) box.append(el("fm-empty","没有匹配的工作空间"));
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

function nativeFolderPickerAvailable(){
  return Boolean(window.__TAURI__ && window.__TAURI__.core && window.__TAURI__.core.invoke);
}

// Tauri 壳调用系统原生文件夹选择器；浏览器/Web 壳受安全限制拿不到绝对路径，继续使用显式路径表单。
async function pickLocalWorkspaceFolder(){
  const invoke=window.__TAURI__?.core?.invoke;
  const picked=await invoke("pick_local_directory");
  if(!picked) return true; // 用户取消属于“已处理”，不应再弹出路径表单。
  return await addLocalWorkspace(picked, null);
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
        if(nativeFolderPickerAvailable()){
          let handled=false;
          try{ handled=await pickLocalWorkspaceFolder(); }
          catch(e){ showToast("打开文件夹选择器失败："+(e&&e.message||e)); }
          if(!handled) showWorkspaceForm("local");
        }else showWorkspaceForm("local");
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
  // / 只在输入框开头触发（对齐 opencode ^\/）：避免「cd /usr」「和/或」这类路径/文本误触；@ 行中空白后即可
  const slash=before.match(/^\/([^\s/]*)$/);
  if(slash) return {type:"slash",trigger:"/",query:slash[1],start:0};
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
  POP.items.forEach((item,i)=>{
    const row=el("fm-item"+(i===POP.active?" active":""));
    if(POP.type==="at"){
      const isDir=item.type==="dir";
      const seg=isDir?{dir:"",leaf:item.name||item.path}:splitPath(item.path||item.name||"");
      row.innerHTML=`<span class="wi">${isDir?"📂":"📄"}</span>`
        +`<span class="fm-line">${seg.dir?`<span class="fm-dir">${esc(seg.dir)}</span>`:""}<span class="fm-leaf">${esc(seg.leaf)}</span></span>`
        +(item.description?`<span class="fm-desc">${esc(item.description)}</span>`:"");
    }else{
      row.innerHTML=`<span class="wi">⚡</span>`
        +`<span class="fm-line"><span class="fm-leaf">/${esc(item.name)}</span></span>`
        +`<span class="fm-desc">${esc(item.description||item.title||"技能工具")}</span>`
        +`<span class="fm-badge">技能</span>`;
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
      const r=await call({cmd:"search_workspace_files",query:tr.query,limit:50});
      if(token!==POP.token) return;
      POP.items=(r.ok&&r.data.files)||[];
      if(!r.ok) showToast(r.error?.message||"文件检索失败");
    }catch(e){ if(token===POP.token) showToast("文件检索失败："+(e&&e.message||e)); }
    finally{ if(token===POP.token) POP.loading=false; }
  }else{
    if(!SKILL_CACHE){
      const r=await call({cmd:"list_skills"});
      SKILL_CACHE=(r.ok&&r.data.skills)||[];
    }
    const q=tr.query.toLowerCase();
    POP.items=SKILL_CACHE.filter(s=>!q||s.name.toLowerCase().includes(q)||(s.description||"").toLowerCase().includes(q));
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
    if(!tr){ closePopover(); return; }
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

document.addEventListener("DOMContentLoaded",()=>{ initWorkspaceUI(); attachComposer(document.getElementById("inp")); bindComposerButtons(); });
