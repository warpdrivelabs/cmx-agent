// ── 活动栏面板切换（Agent / 即时通讯 / 插件管理）：切换左侧操作面板；右侧为共享 tab 区 ──
function showPanel(name){
  ["agent","im","plugins"].forEach(p=>{
    document.getElementById("panel-"+p).classList.toggle("on", p===name);
    document.getElementById("act-"+p).classList.toggle("active", p===name);
  });
  if(name==="im"){ renderIM(); openIM(); }
  if(name==="plugins") loadPluginSidebar();
}

// 即时通讯侧栏：Telegram 式会话列表（占位数据）
const IM_DATA = [
  {nm:"财务审批群",     av:"💰", c:"#eb6834", msg:"李经理：付款单已提交，请审批", tm:"09:42", unread:3},
  {nm:"cmx-flow 助手",  av:"🔀", c:"#2a78d6", msg:"采购付款审批流程已完成", tm:"09:15", unread:0},
  {nm:"运维告警",       av:"🚨", c:"#e34948", msg:"onto 服务延迟正常（4ms）", tm:"昨天", unread:0},
  {nm:"张伟",           av:"张", c:"#1baf7a", msg:"报表数据我核对过了，没问题", tm:"昨天", unread:1},
  {nm:"产品讨论组",     av:"💡", c:"#9085e9", msg:"你：下一版加语音输入", tm:"周一", unread:0},
  {nm:"本体建模组",     av:"◈", c:"#4a3aa7", msg:"新增了订单头对象类型", tm:"周一", unread:0},
];
function renderIM(){
  const box=document.getElementById("im-list"); if(box.dataset.done) return; box.dataset.done="1";
  box.innerHTML="";
  IM_DATA.forEach(m=>{
    const it=el("im-item");
    it.innerHTML=`<div class="av" style="background:${m.c}">${esc(m.av)}</div>
      <div class="mid"><div class="r1"><span class="nm">${esc(m.nm)}</span><span class="tm">${esc(m.tm)}</span></div>
      <div class="r2"><span class="msg">${esc(m.msg)}</span>${m.unread?`<span class="unread">${m.unread}</span>`:""}</div></div>`;
    box.append(it);
  });
}

// 即时通讯：点活动栏 💬 在主内容区开 tab，正中显示联信连接提示（真实联信接入前的占位）
function openIM(){
  const t=openTab({id:"im", kind:"im", title:"即时通讯", ico:"💬"});
  if(!t.view.dataset.built){
    t.view.dataset.built="1";
    t.view.className="tabview im-connecting";
    t.view.innerHTML=`<div class="im-center">
      <div class="ico">💬</div>
      <div class="txt">智能版联信正在连接中...</div>
    </div>`;
  }
  activateTab("im");
}

// 插件管理：VSCode 式已装/市场列表（占位数据）
// 市场演示目录（后端 list_plugins 的 market 为空时兜底；条目形状对齐后端 MarketEntry 摘要，
// 均带 homepage(HTML 信息页) + manifest(内嵌清单，供「安装」真实写入)）。
const PLUG_MARKET_DEMO = [
  {name:"cmx-rules", kind:"http", version:"1.0.0", author:"cmx", description:"决策表 / FEEL 规则评估连接器", icon:"⚖", homepage:"https://feel.cmx.dev/", installable:true,
   manifest:{name:"cmx-rules", kind:"http", version:"1.0.0", description:"决策表 / FEEL 规则评估连接器", icon:"⚖", author:"cmx", homepage:"https://feel.cmx.dev/", base_url:"http://127.0.0.1:8094", method:"POST", path:"/api/rules/v1/evaluate", permissions:["net:fetch"]}},
  {name:"doc-tools", kind:"command", version:"0.9.0", author:"社区", description:"Office / PDF 解析与生成（命令载体示例）", icon:"📄", homepage:"https://example.com/", installable:true,
   manifest:{name:"doc-tools", kind:"command", version:"0.9.0", description:"Office / PDF 解析（命令载体示例）", icon:"📄", author:"社区", homepage:"https://example.com/", command:"echo", args:["doc {file}"], requires_approval:true, permissions:["exec"]}},
  {name:"mcp-bridge", kind:"mcp", version:"1.2.0", author:"cmx", description:"接入任意 Model Context Protocol 服务", icon:"🔌", homepage:"https://modelcontextprotocol.io/", installable:true,
   manifest:{name:"mcp-bridge", kind:"mcp", version:"1.2.0", description:"接入任意 MCP 服务", icon:"🔌", author:"cmx", homepage:"https://modelcontextprotocol.io/", command:"my-mcp-server", args:["--stdio"]}},
];
// kind → 图标兜底 / 底色 / 中文名
const KIND_ICON={http:"🌐",command:"⌘",wasm:"🧱",mcp:"🔌",connector:"🧩"};
const KIND_COLOR={http:"#2a78d6",command:"#4a3aa7",wasm:"#1baf7a",mcp:"#eda100",connector:"#eb6834"};
const KIND_LABEL={http:"HTTP 端点",command:"Shell 命令",wasm:"WebAssembly",mcp:"MCP 服务",connector:"连接器"};
function plugIcon(p){ return p.icon || KIND_ICON[p.kind] || "🧩"; }
function plugColor(p){ return KIND_COLOR[p.kind] || "#6b7280"; }
function plugMeta(p,installed){
  const state = installed ? (p.enabled===false ? "已禁用" : "已启用") : null;
  const parts=[p.author||"cmx", p.version?("v"+p.version):null, KIND_LABEL[p.kind]||p.kind, state].filter(Boolean);
  return parts.join(" · ");
}

// 插件状态缓存（列表在侧栏 master，详情在主区 tab，两处 DOM 树不同 → 用模块级状态共享）
let PLUGIN_STATE={installed:[], market:[]};

// 侧栏插件列表（master，保持在原侧栏位置）：点活动栏 🧩 由 showPanel 触发。
async function loadPluginSidebar(){
  const list=document.getElementById("pg-list");
  if(!list) return;
  // 首次绑定搜索/刷新
  const search=document.getElementById("pg-search-input");
  if(search && !search.dataset.bound){ search.dataset.bound="1"; search.oninput=()=>filterPluginSidebar(search.value); }
  const rf=document.getElementById("pg-refresh");
  if(rf && !rf.dataset.bound){ rf.dataset.bound="1"; rf.onclick=()=>loadPluginSidebar(); }
  list.innerHTML='<div class="pg-loading">正在加载插件…</div>';
  const resp=await call({cmd:"list_plugins"});
  const data=(resp&&resp.ok&&resp.data)||{};
  const installed=data.installed||[];
  let market=data.market||[];
  if(!market.length) market=PLUG_MARKET_DEMO;   // 后端未配市场 URL → 演示目录兜底
  PLUGIN_STATE={installed, market};
  renderPluginSidebar(installed, market);
}

function renderPluginSidebar(installed, market){
  const list=document.getElementById("pg-list");
  list.innerHTML="";
  const sec=(t)=>{ const s=el("pg-sec"); s.textContent=t; list.append(s); };
  const row=(p, isInstalled)=>{
    const off = isInstalled && p.enabled===false;
    const it=el("plug-item pg-item"+(off?" pg-off":""));
    it.dataset.act="pluginSelect"; it.dataset.pkey=p.name; it.dataset.pinstalled=isInstalled?"1":"0";
    it.innerHTML=`<div class="pic" style="background:${plugColor(p)}">${esc(plugIcon(p))}</div>
      <div class="pm"><div class="pn">${esc(p.name||"")}</div><div class="pd">${esc(p.description||"")}</div><div class="pmeta">${esc(plugMeta(p,isInstalled))}</div></div>
      <div class="pbtn ${isInstalled?'installed':''}">${isInstalled?(off?'已禁用':'已安装'):'安装'}</div>`;
    list.append(it);
  };
  sec("已安装 ("+installed.length+")");
  if(installed.length) installed.forEach(p=>row(p,true));
  else { const e=el("pg-hint"); e.textContent="（暂无已安装插件；从市场安装后重启生效）"; list.append(e); }
  sec("市场 ("+market.length+")");
  market.forEach(p=>row(p,false));
}

function filterPluginSidebar(q){
  q=(q||"").trim().toLowerCase();
  document.querySelectorAll("#pg-list .pg-item").forEach(it=>{
    const hit=!q || it.textContent.toLowerCase().includes(q);
    it.style.display=hit?"":"none";
  });
}

// 点侧栏某插件 → 在右侧主内容区打开/复用同一「插件详情」tab（不每插件一个 tab）。
function openPluginDetail(p, installed){
  const t=openTab({id:"plugins", kind:"plugins", title:"插件", ico:"🧩"});
  if(!t.view.dataset.built){
    t.view.dataset.built="1";
    t.view.className="tabview";
    t.view.append(document.getElementById("tpl-plugins").content.cloneNode(true));
  }
  activateTab("plugins");
  renderPluginDetail(t.view, p, installed);
}

// VSCode 式详情：头部 + 能力/权限 + 信息页（HTML 链接 iframe 内嵌 + 外开兜底）。已装/未装差异化。
function renderPluginDetail(view, p, installed){
  const d=view.querySelector(".pg-detail");
  const enabled = p.enabled!==false;   // 已装默认启用；后端 enabled:false 表示禁用
  const badge=installed
    ? (enabled ? `<span class="pg-badge installed">✓ 已安装 · 已启用</span>`
               : `<span class="pg-badge disabled">⊘ 已安装 · 已禁用</span>`)
    : `<span class="pg-badge market">☁ 未安装</span>`;
  const actions=installed
    ? `<button class="pg-actbtn" data-act="pluginUninstall" data-pkey="${esc(p.name)}">卸载</button>
       <button class="pg-actbtn ghost" data-act="pluginToggle" data-pkey="${esc(p.name)}" data-enabled="${enabled?'1':'0'}">${enabled?'禁用':'启用'}</button>`
    : `<button class="pg-actbtn primary" data-act="pluginInstall" data-pkey="${esc(p.name)}">安装</button>`;
  const openBtn=p.homepage?`<button class="pg-actbtn ghost" data-act="pluginOpenHomepage" data-url="${esc(p.homepage)}">⧉ 在浏览器打开</button>`:"";
  // 能力/权限 chips + 载体特有
  const chips=[];
  chips.push(`<span class="pg-chip">${esc(KIND_LABEL[p.kind]||p.kind||"插件")}</span>`);
  (p.permissions||[]).forEach(pm=>chips.push(`<span class="pg-chip perm">🔒 ${esc(pm)}</span>`));
  if(p.requiresApproval) chips.push(`<span class="pg-chip warn">需审批</span>`);
  const carrier=[];
  const kv=(k,v)=>{ if(v!=null&&v!=="") carrier.push(`<div class="pg-kv"><span class="k">${esc(k)}</span><span class="v">${esc(String(v))}</span></div>`); };
  if(p.kind==="http"){ kv("base_url",p.baseUrl); kv("method",p.method); kv("path",p.path); }
  else if(p.kind==="command"){ kv("command",p.command); if((p.args||[]).length) kv("args",(p.args||[]).join(" ")); }
  else if(p.kind==="wasm"){ kv("module",p.module); kv("invoke",p.invoke); kv("runtime",p.runtime); }
  else if(p.kind==="mcp"){ kv("command",p.command); if((p.args||[]).length) kv("args",(p.args||[]).join(" ")); }
  // 信息页
  let info;
  if(p.homepage){
    // sandbox 不得带 allow-same-origin：与 allow-scripts 同开 = 沙箱失效，远程信息页可同源调 /api 前门。
    info=`<div class="pg-frame-bar">信息页：<a class="pg-link" data-act="pluginOpenHomepage" data-url="${esc(p.homepage)}">${esc(p.homepage)}</a><span class="pg-frame-hint">（若下方空白说明该页禁止内嵌，请点链接在浏览器打开）</span></div>
      <iframe class="pg-frame" src="${esc(p.homepage)}" referrerpolicy="no-referrer" sandbox="allow-scripts allow-popups allow-forms"></iframe>`;
  } else {
    info=`<div class="pg-empty sm">该插件未提供信息页（cmx-plugin.json 的 homepage 字段）。</div>`;
  }
  d.innerHTML=`<div class="pg-detail-inner ${installed?'is-installed':'is-market'}">
    <div class="pg-topbar"></div>
    <div class="pg-head">
      <div class="pg-ico" style="background:${plugColor(p)}">${esc(plugIcon(p))}</div>
      <div class="pg-h-main">
        <div class="pg-title">${esc(p.name||"")} ${badge}</div>
        <div class="pg-sub">${esc(plugMeta(p,installed))}</div>
        <div class="pg-desc">${esc(p.description||"")}</div>
        <div class="pg-actions">${actions}${openBtn}</div>
      </div>
    </div>
    <div class="pg-sec-h">能力与权限</div>
    <div class="pg-chips">${chips.join("")}</div>
    ${carrier.length?`<div class="pg-kvs">${carrier.join("")}</div>`:""}
    <div class="pg-sec-h">信息</div>
    ${info}
  </div>`;
}

// —— 插件 master-detail 交互（列表在侧栏，详情在主区 tab）——
function pluginSelect(el){
  const key=el.dataset.pkey, installed=el.dataset.pinstalled==="1";
  const pool=installed?PLUGIN_STATE.installed:PLUGIN_STATE.market;
  const p=(pool||[]).find(x=>x.name===key); if(!p) return;
  document.querySelectorAll("#pg-list .pg-item.sel").forEach(x=>x.classList.remove("sel"));
  el.classList.add("sel");
  openPluginDetail(p, installed);
}
function pluginOpenHomepage(el){
  const url=el.dataset.url; if(!url) return;
  try{ window.open(url, "_blank", "noopener"); }catch(e){ showToast("无法打开链接"); }
}
// 安装：从当前选中的市场插件取内嵌 manifest → 前门 install_plugin（点击即人工授权）→ 刷新。
async function pluginInstall(el){
  const key=el.dataset.pkey;
  const p=(PLUGIN_STATE.market||[]).find(x=>x.name===key);
  if(!p){ showToast("未找到该市场插件"); return; }
  if(!p.manifest){ showToast("该市场条目未内嵌清单，无法安装"); return; }
  el.disabled=true; el.textContent="安装中…";
  const r=await call({cmd:"install_plugin", manifest:p.manifest});
  if(r&&r.ok){
    const note=(r.data&&r.data.note)||"已安装";
    showToast("已安装「"+key+"」· "+note);
    await loadPluginSidebar();
    const np=(PLUGIN_STATE.installed||[]).find(x=>x.name===key);
    reopenPluginDetail(np||p, !!np);   // 详情切到「已安装」态
  } else {
    el.disabled=false; el.textContent="安装";
    showToast("安装失败："+((r&&r.error&&r.error.message)||"未知错误"));
  }
}
// 卸载：两段式确认（首点变红「确认卸载」，3s 内再点才执行；两壳都稳，不依赖 window.confirm）。
async function pluginUninstall(el){
  const key=el.dataset.pkey;
  if(el.dataset.armed!=="1"){
    el.dataset.armed="1"; el.classList.add("danger"); const old=el.textContent; el.textContent="确认卸载？";
    el._t=setTimeout(()=>{ el.dataset.armed="0"; el.classList.remove("danger"); el.textContent=old; }, 3000);
    return;
  }
  clearTimeout(el._t); el.dataset.armed="0"; el.disabled=true; el.textContent="卸载中…";
  const r=await call({cmd:"uninstall_plugin", name:key});
  if(r&&r.ok){
    const note=(r.data&&r.data.note)||"已卸载";
    showToast("已卸载「"+key+"」· "+note);
    await loadPluginSidebar();
    const mp=(PLUGIN_STATE.market||[]).find(x=>x.name===key);
    if(mp) reopenPluginDetail(mp, false); else resetPluginDetail();
  } else {
    el.disabled=false; el.textContent="卸载";
    showToast("卸载失败："+((r&&r.error&&r.error.message)||"未知错误"));
  }
}
async function pluginToggle(el){
  const key=el.dataset.pkey, enable = el.dataset.enabled!=="1";  // 当前启用→点击=禁用
  el.disabled=true; el.textContent=enable?"启用中…":"禁用中…";
  const r=await call({cmd:"toggle_plugin", name:key, enabled:enable});
  if(r&&r.ok){
    const note=(r.data&&r.data.note)||(enable?"已启用":"已禁用");
    showToast((enable?"已启用「":"已禁用「")+key+"」· "+note);
    await loadPluginSidebar();
    const np=(PLUGIN_STATE.installed||[]).find(x=>x.name===key);
    if(np) reopenPluginDetail(np, true);
  } else {
    el.disabled=false; el.textContent=enable?"启用":"禁用";
    showToast("操作失败："+((r&&r.error&&r.error.message)||"未知错误"));
  }
}
// 详情复位/重开（安装/卸载后刷新详情面板 + 同步侧栏高亮）
function resetPluginDetail(){
  const t=findTab&&findTab("plugins"); if(t) t.view.querySelector(".pg-detail").innerHTML='<div class="pg-empty">← 在左侧插件列表中选择一个插件查看详情</div>';
}
function reopenPluginDetail(p, installed){
  openPluginDetail(p, installed);
  // 同步侧栏选中高亮到对应行
  document.querySelectorAll("#pg-list .pg-item.sel").forEach(x=>x.classList.remove("sel"));
  const row=document.querySelector('#pg-list .pg-item[data-pkey="'+(window.CSS&&CSS.escape?CSS.escape(p.name):p.name)+'"][data-pinstalled="'+(installed?"1":"0")+'"]');
  if(row) row.classList.add("sel");
}
