// ── 事件委托：一个监听器处理所有 [data-act] 点击。
// ── 标题栏控件：侧栏开合 / 主题切换 / 检查更新 ──
function toggleSidebar(){
  document.body.classList.toggle("sidebar-collapsed");
  const on = document.body.classList.contains("sidebar-collapsed");
  document.getElementById("tb-side-ico").textContent = on ? "⇥" : "⇤";
}
function applyTheme(t){
  document.documentElement.setAttribute("data-theme", t);
  document.getElementById("tb-theme-ico").textContent = t === "light" ? "☾" : "☀";
  try{ localStorage.setItem("cmx-theme", t); }catch(e){}
}
function toggleTheme(){
  const cur = document.documentElement.getAttribute("data-theme")==="light" ? "light" : "dark";
  applyTheme(cur === "light" ? "dark" : "light");
}
let APP_VERSION = "M2 · 0.1.0";
// 桌面壳：真实版本号从壳注入（get_app_version），覆盖静态兜底（关于分区 / 更新提示共用）。
(async()=>{
  if(window.__TAURI__ && window.__TAURI__.core){
    try{ APP_VERSION = "M2 · " + await window.__TAURI__.core.invoke("get_app_version"); }catch(e){}
  }
})();
function closeMenu(){
  document.getElementById("modelmenu").classList.remove("on");
  document.getElementById("tab-dropdown").classList.remove("on");
}
let CURRENT_USER = null;   // {user_id, username, nickname, roles} 或 null
// 拉取当前登录用户（前门 current_user），维护设置中心账户分区 + 未登录红点。未登录则显示占位。
async function refreshUser(){
  try{
    const r = await call({cmd:"current_user"});
    CURRENT_USER = (r.ok && r.data && r.data.user) || null;
  }catch(e){ CURRENT_USER = null; }
  // 原生壳未登录：隐藏主窗、聚焦登录窗（每次启动先见登录门，与登出行为对齐）。
  if(!CURRENT_USER){ location.hash = "#/login"; showLoginView(); } // SPA：未认证切登录视图
  const u = CURRENT_USER;
  // 未登录红点（设置中心 logo 入口）：未登录点亮，登录后熄灭。
  const dot = document.getElementById("logo-dot");
  if(dot) dot.style.display = u ? "none" : "";
  // 设置中心账户分区若正开着，同步刷新头像/昵称卡。
  if(typeof renderAccountSection === "function") renderAccountSection();
  // 初始密码强制改密：must_change_password=true 期间改密框常驻（无关闭入口），改密/退出前不能操作；
  // 弹框若被意外关闭（正常没有入口）这里兜底再弹起。登出（u=null）解除强制态。
  if(u && u.must_change_password){
    _pwdForced = true;
    if(document.getElementById("pwd-overlay").classList.contains("hidden")) openPwdDialog(true);
  }
  if(!u){ _pwdForced = false; }
}
function logoutCancel(){ document.getElementById("logout-overlay").classList.add("hidden"); }
async function logoutConfirm(){
  logoutCancel();
  // 换号清场：先中断所有会话在途回合（后端取消含打断审批等待 + 前端停流 + 清排队），
  // 防旧账号发起的回合在登录新账号后继续推进/回灌到界面。IM 侧由后端登出钩子停桥兜底。
  try{
    TABS.filter(t=>t.kind==="session").forEach(t=>{
      try{ call({cmd:"cancel_session",session_id:t.sessionId}); }catch(e){}
      try{ if(t._streamCancel) t._streamCancel(); }catch(e){}
      t._queue=[];
    });
  }catch(e){}
  try{ await call({cmd:"logout"}); }catch(e){}
  CURRENT_USER = null;
  // 原生壳：回到登录窗（隐藏主窗、显示登录窗）；Web 壳：跳登录页。
  location.hash = "#/login"; showLoginView(); // SPA：切登录视图（两壳统一）
}

// ── 结果操作按钮（复制/分享/点赞/差评）：仅图标 + 原生 tooltip（title）。
// 挂载时机在 turn_ended（render.js）：一回合唯一一份，挂在回合最底（工具行/报错行之后），
// 不再塞进回复气泡——气泡后面还有工具行时按钮行会把时间线拦腰截断（用户多次反馈）。
const TOOL_ACT_BTNS =
  `<button class="tcact" data-act="toolCopy" title="复制" aria-label="复制"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg></button>`+
  `<button class="tcact" data-act="toolShare" title="分享" aria-label="分享"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" y1="13.51" x2="15.42" y2="17.49"/><line x1="15.41" y1="6.51" x2="8.59" y2="10.49"/></svg></button>`+
  `<button class="tcact" data-act="toolLike" title="点赞" aria-label="点赞"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 9V5a3 3 0 0 0-3-3l-4 9v11h11.28a2 2 0 0 0 2-1.7l1.38-9a2 2 0 0 0-2-2.3zM7 22H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2h3"/></svg></button>`+
  `<button class="tcact" data-act="toolDislike" title="差评" aria-label="差评"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 15v4a3 3 0 0 0 3 3l4-9V2H5.72a2 2 0 0 0-2 1.7l-1.38 9a2 2 0 0 0 2 2.3zm7-13h2.67A2.31 2.31 0 0 1 22 4v7a2.31 2.31 0 0 1-2.33 2H17"/></svg></button>`;
// 回合操作行：挂回合元素最底（唯一落款）；_for 回指末条回复气泡，复制/分享取正文用它
// （按钮行本体不在气泡里，closest(".bubble") 够不到）。
function addTurnActions(turn, tail){
  if(!turn || !tail || !tail.innerText.trim()) return;
  turn.querySelectorAll(":scope > .tcacts").forEach(a=>a.remove());   // 幂等
  const acts=document.createElement("div"); acts.className="tcacts bubble-acts";
  acts.innerHTML=TOOL_ACT_BTNS; acts._for=tail;
  turn.appendChild(acts);
}
// 复制/分享取正文：回合操作行经 _for 回指末条回复气泡；工具卡取正文块，气泡取全文
function panelText(box){
  if(!box) return "";
  const clone=box.cloneNode(true); clone.querySelectorAll(".tcacts").forEach(a=>a.remove());
  const parts=[...clone.querySelectorAll(".tterm,.tcode,.tcsub,.tcempty")].map(e=>e.innerText.trim()).filter(Boolean);
  if(parts.length) return parts.join("\n\n");
  const tbl=clone.querySelector(".tdata"); if(tbl) return tbl.innerText.trim();
  return clone.innerText.trim();
}
async function copyText(txt){
  try{ await navigator.clipboard.writeText(txt); return true; }
  catch(e){ try{ const ta=document.createElement("textarea"); ta.value=txt; document.body.appendChild(ta); ta.select(); document.execCommand("copy"); ta.remove(); return true; }catch(_){ return false; } }
}
async function toolCopy(btn){ const ok=await copyText(panelText(btn.closest(".tcacts")?._for || btn.closest(".tool,.bubble"))); showToast(ok?"已复制到剪贴板":"复制失败"); }
async function toolShare(btn){
  const txt=panelText(btn.closest(".tcacts")?._for || btn.closest(".tool,.bubble"));
  if(navigator.share){ try{ await navigator.share({text:txt}); return; }catch(e){ if(e&&e.name==="AbortError") return; } }
  const ok=await copyText(txt); showToast(ok?"已复制，可粘贴分享":"分享失败");
}
function toolLike(btn){
  const box=btn.closest(".tool,.bubble"); const dis=box.querySelector('[data-act="toolDislike"]');
  const on=btn.classList.toggle("liked"); if(on&&dis) dis.classList.remove("disliked");
  showToast(on?"已点赞 👍":"已取消点赞");
}
function toolDislike(btn){
  const box=btn.closest(".tool,.bubble"); const lk=box.querySelector('[data-act="toolLike"]');
  const on=btn.classList.toggle("disliked"); if(on&&lk) lk.classList.remove("liked");
  showToast(on?"已记录反馈，谢谢":"已取消差评");
}
function showToast(msg){
  let t=document.getElementById("cmx-toast");
  if(!t){ t=document.createElement("div"); t.id="cmx-toast"; t.className="cmx-toast"; document.body.appendChild(t); }
  t.textContent=msg; t.classList.add("on");
  clearTimeout(showToast._t); showToast._t=setTimeout(()=>t.classList.remove("on"),1600);
}
// 敏感字段掩码切换（ZCode 同款：password 圆点 + 线性眼睛图标，用户 2026-09-17 定稿）：
// 切换同容器 input 的 type；脏检测不受影响——非空且 ≠ 回填脱敏值才 set（dataset.masked）。
const ICON_EYE=`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>`;
const ICON_EYE_OFF=`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>`;
function secretToggle(btn){
  // 通用约定：eye-btn 与 input 是同容器直接子级（.mcfg-key-wrap / .pwd-wrap 均满足）。
  const inp=btn.parentElement.querySelector("input");
  if(!inp) return;
  const show=inp.type==="password";
  inp.type=show?"text":"password";
  btn.innerHTML=show?ICON_EYE_OFF:ICON_EYE;
  btn.title=show?"隐藏":"显示";
}
// 所有 .eye-btn 启动时落默认图标（模型 API Key / IM Secret / 登录注册改密码——同一机制，零重复）。
document.querySelectorAll(".eye-btn").forEach(b=>{ b.innerHTML=ICON_EYE; });

// 内联 onclick 属性全被拦截。用委托（监听在 document 上，由 nonce'd 脚本注册）在两壳都工作。
// 重构：toggleMenu/toggleUserMenu（原两浮层菜单）删除；menuSettings/menuAbout/userProfile/userSwitch/
// userWorkspace/userLogout 收拢进设置中心（js/settings.js 的 openSettings/toggleSettings + acc* 动作）。
const ACTIONS = { newTask, openConnectors, openAssistant, startFromHome, toggleSidebar, toggleTheme, applyUpdate: updateBtnClick, toggleSettings, openSettingsSection, settingsSection, setThemeCard, accChangePassword, accLogout, aboutCheckUpdate, toggleVoice,
  toggleTabOverflow, tabCtxClose, tabCtxCloseOthers, tabCtxCloseRight, tabCtxCloseAll, winMinimize, winToggleMaximize, winClose,
  closeSettings, scfgQqLogin, scfgSelect: scfgSelectChannel, scfgWechatLogin, imcfgSave,
  mcfgClose: closeModelConfig, mcfgSave: saveModelConfig,
  mcfgNew: mcfgNew, mcfgDelete: deleteModelProvider,
  // 模型分区（ZCode 化 P1）：供应商目录 / 清单行内管理 + 模型编辑弹窗
  // （底部「测试连接」按钮已删——每行烧瓶单测已覆盖，2026-09-17）
  mcfgCatClose: mcfgCatalogClose, mcfgCatPick: mcfgCatalogPick,
  mcfgModelAdd, mcfgModelDel, mcfgModelEdit, mcfgModelTest,
  medClose, medSave, medLevelAdd,
  // agentsSelect 收 data-name（调度器统一传 el，包装转换——直接传 el 会让 agentsFind 永不命中，列表点击整体失效）
  agentsSelect: el=>agentsSelect(el.dataset.name),
  agentsNew, agentsSave, agentsDelete,
  userChangePassword, pwdClose, pwdExit, pwdSave, logoutCancel, logoutConfirm };
document.addEventListener("click", e=>{
  const el = e.target.closest("[data-act]");
  // 菜单切换类动作自己管开/关，不提前关（否则 openModelMenu 刚开就被关掉）
  const _menuToggle = ["openModelMenu","toggleSettings","toggleTabOverflow"];
  if(!el || !_menuToggle.includes(el.dataset.act)){ closeMenu(); closeTabCtx(); }
  if(!el) return;
  const act = el.dataset.act;
  if(act === "runConnectorTool"){
    runConnectorTool(el.dataset.tool, el.dataset.online === "1");
  } else if(act === "approveTool"){
    approveTool(el.dataset.callid, el.dataset.ok === "1", el.dataset.all === "1");
  } else if(act === "answerQuestion"){
    answerQuestion(el);
  } else if(act === "toolCopy"){ toolCopy(el);
  } else if(act === "copyBody"){
    const b=el.closest(".tc-body");
    if(b){ const c=b.cloneNode(true); c.querySelectorAll(".tc-copy").forEach(n=>n.remove());
      copyText(c.innerText).then(ok=>showToast(ok?"已复制到剪贴板":"复制失败")); }
  } else if(act === "toolShare"){ toolShare(el);
  } else if(act === "toolLike"){ toolLike(el);
  } else if(act === "toolDislike"){ toolDislike(el);
  } else if(act === "pluginSelect"){ pluginSelect(el);
  } else if(act === "pluginOpenHomepage"){ pluginOpenHomepage(el);
  } else if(act === "secretToggle"){ secretToggle(el);
  } else if(act === "pluginInstall"){ pluginInstall(el);
  } else if(act === "pluginUninstall"){ pluginUninstall(el);
  } else if(act === "pluginToggle"){ pluginToggle(el);
  } else if(act === "openModelMenu"){ openModelMenu(el);
  } else if(act === "setModelChoice"){ setModelChoice(el);
  } else if(act === "setProviderChoice"){ setProviderChoice(el);
  } else if(act === "providerSelect"){ mcfgSelectProvider(el);
  } else if(act === "toggleVoice"){
    toggleVoice(el);
  } else if(act === "showPanel"){
    showPanel(el.dataset.panel);
  } else if(act === "scfgSelect"){ scfgSelectChannel(el);
  } else if(act === "switchCat"){
    el.closest(".cats").querySelectorAll(".cat").forEach(b=>b.classList.remove("active"));
    el.classList.add("active");
  } else if(ACTIONS[act]){
    // 统一传 el：ACTIONS 里多数处理器不用参数（无害），分区跳转/主题卡/开关类需要 data-* 参数
    ACTIONS[act](el);
  }
});

// 设置分区切换：scfg-kind 是旧设置面板遗留元素（现已不在 HTML 中）——必须判空，否则此处 TypeError 会
// 中断 main.js 后续全部启动逻辑（主题恢复 / refreshUser / refreshTasks / SPA 回放兜底）。
{
  const _scfgKind = document.getElementById("scfg-kind");
  if(_scfgKind) _scfgKind.addEventListener("change", scfgSwitchKind);
}
// 模型分区：模型清单行内编辑写回（行动态渲染，容器委托）/ 添加框回车（内联 on* 被两壳 CSP
// 拦截，显式监听）。遮罩关闭由设置中心统一接管（js/settings.js 的 #settings-overlay 遮罩点击/Esc）；
// 供应商目录弹层点击遮罩空白关闭。
{
  const modelsBox = document.getElementById("mcfg-models");
  modelsBox.addEventListener("change", e=>{
    const row=e.target.closest(".ml-row"); if(!row) return;
    const m=_mcfgModels[+row.dataset.i]; if(!m) return;
    if(e.target.type==="checkbox") m.enabled=e.target.checked;
  });
  // API 格式下拉：当前仅 openai 可选（其余为禁用占位），选择即写回保存载荷。
  document.getElementById("mcfg-kind").addEventListener("change", e=>{ _mcfgKind=e.target.value; });
  const catOv = document.getElementById("mcfg-cat-overlay");
  catOv.addEventListener("click", e=>{ if(e.target===catOv) mcfgCatalogClose(); });
  const medOv = document.getElementById("med-overlay");
  medOv.addEventListener("click", e=>{ if(e.target===medOv) medClose(); });
}
// 修改密码框：三个输入框回车即提交（改密弹框不点遮罩关闭——强制提醒场景避免误关）。
["pwd-old","pwd-new","pwd-confirm"].forEach(id=>document.getElementById(id).addEventListener("keydown", e=>{
  if(e.key==="Enter") pwdSave();
}));
// 退出登录确认框：点遮罩 = 取消。
{
  const _lo = document.getElementById("logout-overlay");
  _lo.addEventListener("click", e => { if(e.target === _lo) logoutCancel(); });
}

// 初始化主题（读上次选择，缺省深色）
applyTheme((()=>{ try{ return localStorage.getItem("cmx-theme")||"dark"; }catch(e){ return "dark"; } })());

// 左侧栏宽度可调：拖动 .splitter，宽度存 localStorage，clamp [200,480]
(function initSplitter(){
  const sp=document.getElementById("splitter"), root=document.documentElement;
  try{ const w=localStorage.getItem("cmx-aside-w"); if(w) root.style.setProperty("--aside-w", w+"px"); }catch(e){}
  let startX=0, startW=0, dragging=false;
  const curW=()=>parseInt(getComputedStyle(root).getPropertyValue("--aside-w"))||270;
  sp.addEventListener("pointerdown", e=>{
    dragging=true; startX=e.clientX; startW=curW();
    sp.classList.add("dragging"); sp.setPointerCapture(e.pointerId);
    document.body.style.userSelect="none";
  });
  sp.addEventListener("pointermove", e=>{
    if(!dragging) return;
    const w=Math.max(200,Math.min(480, startW+(e.clientX-startX)));
    root.style.setProperty("--aside-w", w+"px");
  });
  const end=()=>{ if(!dragging) return; dragging=false; sp.classList.remove("dragging"); document.body.style.userSelect="";
    try{ localStorage.setItem("cmx-aside-w", curW()); }catch(e){} };
  sp.addEventListener("pointerup", end); sp.addEventListener("pointercancel", end);
})();

// 启动：初始化 tab 条（空态显示首页）、更新按钮（默认隐藏）、当前用户，聚焦输入框
(async()=>{ renderUpdateBtn(); refreshUser(); refreshModelLabel(); await refreshTasks(); renderTabs(); document.getElementById("inp").focus(); })();

// ── SPA 会话回放兜底：webview JS 跑在 try_restore_session 完成之前（竞态），
// 首查 whoami 失败跳到登录页；2s 后再查一次，恢复成功则切回主视图。 ──
setTimeout(async () => {
  if (location.hash !== "#/login" && location.hash !== "") return;
  try {
    const r = await call({cmd:"current_user"});
    if (r && r.ok && r.data && r.data.user) {
      location.hash = "#/";
      showMainView();
      refreshUser();
      refreshTasks();
      refreshModelLabel();
    }
  } catch(e) {}
}, 2000);

// 登录后主窗口被显示时刷新用户信息：原生壳监听 Tauri "logged-in" 事件；并兜底监听窗口 focus。
if(window.__TAURI__ && window.__TAURI__.event){ window.__TAURI__.event.listen("logged-in", ()=>{ location.hash = "#/"; showMainView(); refreshUser(); refreshTasks(); refreshModelLabel(); }); }
// 兜底监听窗口 focus 刷新用户信息（3s debounce，避免每次点内元素都发 current_user 请求）
let _refreshUserTimer=null;
window.addEventListener("focus", ()=>{ if(_refreshUserTimer) return; _refreshUserTimer=setTimeout(()=>{ _refreshUserTimer=null; refreshUser(); }, 3000); });

// 会话事件总线实时通道（U16）：任意来源（本地 / IM 桥）的会话事件广播给前端，按 session_id 分流渲染。
// 这是「飞书发消息实时显示到对话界面」的最后一公里——IM 无关，微信/钉钉接入后走同一条路。
if(window.__TAURI__ && window.__TAURI__.event){
  window.__TAURI__.event.listen("session_event", e => {
    try { console.log("[session_event] recv", JSON.stringify(e && e.payload)); } catch(err){ console.log("[session_event] recv (unserializable)", e && e.payload); }
    onSessionEvent(e.payload);
  });
  // 总线丢帧重同步（红蓝审查 P3-5）：壳在 event bus Lagged 时广播，前端标脏+重拉落库事件。
  window.__TAURI__.event.listen("session_resync", () => { if(typeof resyncOpenSessions==="function") resyncOpenSessions(); });
  console.log("[session_event] 监听已注册");
} else {
  console.log("[session_event] 未注册：window.__TAURI__.event 不可用");
  // Web 壳实时通路（U16 落地，红蓝审查 P1-2）：EventSource 订阅 /api/subscribe，同一
  // onSessionEvent 分流——后台子任务回执 / IM 会话事件实时上屏，不再「落库才可见」。
  // 本地回合由 STREAMING 标记（bridge.js web 分支）交还给 streamSend 流式通道，不重复渲染。
  // 封装为可重建：未登录时建连会被 401 打死（EventSource 对 401 不自动重连），
  // 登录/注册成功后经 showMainView() → initEventBus() 重挂。
  window.initEventBus = function(){
    try{
      if (window.__es) { try{ window.__es.close(); }catch(_){} }
      const es = new EventSource("/api/subscribe");
      es.onmessage = (m)=>{ try{ onSessionEvent(JSON.parse(m.data)); }catch(e){} };
      es.onerror = ()=>{ /* 网络断线 EventSource 自动重连；401（未登录）会停止——登录成功后由 showMainView 重建 */ };
      window.__es = es;
      console.log("[session_event] EventSource(/api/subscribe) 已注册");
    }catch(e){ console.warn("[session_event] EventSource 不可用", e); }
  };
  initEventBus();
}
