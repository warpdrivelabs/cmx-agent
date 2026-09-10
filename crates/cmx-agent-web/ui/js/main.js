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
// 桌面壳：真实版本号从壳注入（get_app_version），覆盖静态兜底（关于菜单 / 更新提示共用）。
(async()=>{
  if(window.__TAURI__ && window.__TAURI__.core){
    try{ APP_VERSION = "M2 · " + await window.__TAURI__.core.invoke("get_app_version"); }catch(e){}
  }
})();
function toggleMenu(){ document.getElementById("usermenu").classList.remove("on"); document.getElementById("cmxmenu").classList.toggle("on"); }
function closeMenu(){
  document.getElementById("cmxmenu").classList.remove("on");
  document.getElementById("usermenu").classList.remove("on");
  document.getElementById("modelmenu").classList.remove("on");
  document.getElementById("tab-dropdown").classList.remove("on");
}
let CURRENT_USER = null;   // {user_id, username, nickname, roles} 或 null
// 拉取当前登录用户（前门 current_user），填充活动栏头像 + 用户菜单。未登录则显示占位。
async function refreshUser(){
  try{
    const r = await call({cmd:"current_user"});
    CURRENT_USER = (r.ok && r.data && r.data.user) || null;
  }catch(e){ CURRENT_USER = null; }
  // 原生壳未登录：隐藏主窗、聚焦登录窗（每次启动先见登录门，与登出行为对齐）。
  if(!CURRENT_USER){ location.hash = "#/login"; showLoginView(); } // SPA：未认证切登录视图
  const u = CURRENT_USER;
  const name = u ? (u.nickname || u.username) : "未登录";
  const initial = name && name !== "未登录" ? name.trim().charAt(0).toUpperCase() : "👤";
  document.getElementById("uic").textContent = initial;
  document.getElementById("user-act-btn").title = name;
  document.getElementById("um-av").textContent = initial;
  document.getElementById("um-name").textContent = name;
  document.getElementById("um-mail").textContent = u
    ? ("@"+u.username + (u.roles && u.roles.length ? " · "+u.roles.join("/") : ""))
    : "点此登录 cmx 门户";
  // 初始密码强制改密：must_change_password=true 期间改密框常驻（无关闭入口），改密/退出前不能操作；
  // 弹框若被意外关闭（正常没有入口）这里兜底再弹起。登出（u=null）解除强制态。
  if(u && u.must_change_password){
    _pwdForced = true;
    if(document.getElementById("pwd-overlay").classList.contains("hidden")) openPwdDialog(true);
  }
  if(!u){ _pwdForced = false; }
}
function toggleUserMenu(){ document.getElementById("cmxmenu").classList.remove("on"); document.getElementById("usermenu").classList.toggle("on"); }
function userProfile(){ closeMenu();
  const u = CURRENT_USER;
  if(!u){ showToast("尚未登录。"); return; }
  infoDialog("账户信息", (u.nickname||u.username)+"（@"+u.username+"）\n用户 ID："+u.user_id+"\n角色："+((u.roles||[]).join("、")||"—")+"\n\n对接 cmx 门户统一认证（/api/auth）。"); }
function userSwitch(){ closeMenu(); infoDialog("切换账户", "退出后重新登录即可切换账户/租户。"); }
function userWorkspace(){ closeMenu(); infoDialog("工作空间（占位）", "管理沙箱工作区根目录与数据目录。"); }

function userLogout(){ closeMenu();
  // 自制确认框替代原生 confirm（与主题一致）：取消/点遮罩关闭，确认走 logoutConfirm。
  document.getElementById("logout-overlay").classList.remove("hidden");
}
function logoutCancel(){ document.getElementById("logout-overlay").classList.add("hidden"); }
async function logoutConfirm(){
  logoutCancel();
  try{ await call({cmd:"logout"}); }catch(e){}
  CURRENT_USER = null;
  // 原生壳：回到登录窗（隐藏主窗、显示登录窗）；Web 壳：跳登录页。
  location.hash = "#/login"; showLoginView(); // SPA：切登录视图（两壳统一）
}

// ── 结果面板操作按钮（复制/分享/点赞/差评）：仅图标 + 原生 tooltip（title），注入到 .tchead 右侧 ──
const TOOL_ACT_BTNS =
  `<button class="tcact" data-act="toolCopy" title="复制" aria-label="复制"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="9" y="9" width="13" height="13" rx="2" ry="2"/><path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"/></svg></button>`+
  `<button class="tcact" data-act="toolShare" title="分享" aria-label="分享"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="18" cy="5" r="3"/><circle cx="6" cy="12" r="3"/><circle cx="18" cy="19" r="3"/><line x1="8.59" y1="13.51" x2="15.42" y2="17.49"/><line x1="15.41" y1="6.51" x2="8.59" y2="10.49"/></svg></button>`+
  `<button class="tcact" data-act="toolLike" title="点赞" aria-label="点赞"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M14 9V5a3 3 0 0 0-3-3l-4 9v11h11.28a2 2 0 0 0 2-1.7l1.38-9a2 2 0 0 0-2-2.3zM7 22H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2h3"/></svg></button>`+
  `<button class="tcact" data-act="toolDislike" title="差评" aria-label="差评"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 15v4a3 3 0 0 0 3 3l4-9V2H5.72a2 2 0 0 0-2 1.7l-1.38 9a2 2 0 0 0 2 2.3zm7-13h2.67A2.31 2.31 0 0 1 22 4v7a2.31 2.31 0 0 1-2.33 2H17"/></svg></button>`;
function addToolActions(card){
  const head=card.querySelector(".tchead");
  if(!head || head.querySelector(".tcacts")) return;
  const acts=document.createElement("span"); acts.className="tcacts"; acts.innerHTML=TOOL_ACT_BTNS;
  const meta=head.querySelector(".tcmeta");
  if(meta){ meta.after(acts); }                              // 跟在「exit N / N 步」右侧
  else { acts.style.marginLeft="auto"; head.appendChild(acts); }  // 无 meta 时按钮组自己靠右
}
// AI 文本气泡：把同款操作按钮作为底部行追加（气泡无表头，放底部）
function addBubbleActions(bubble){
  if(!bubble || !bubble.innerText.trim() || bubble.querySelector(":scope > .tcacts")) return;
  const acts=document.createElement("div"); acts.className="tcacts bubble-acts"; acts.innerHTML=TOOL_ACT_BTNS;
  bubble.appendChild(acts);
}
// 复制/分享取正文文本（去掉操作按钮本身）；工具卡取正文块，气泡取全文
function panelText(box){
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
async function toolCopy(btn){ const ok=await copyText(panelText(btn.closest(".tool,.bubble"))); showToast(ok?"已复制到剪贴板":"复制失败"); }
async function toolShare(btn){
  const txt=panelText(btn.closest(".tool,.bubble"));
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

// 内联 onclick 属性全被拦截。用委托（监听在 document 上，由 nonce'd 脚本注册）在两壳都工作。
const ACTIONS = { newTask, openConnectors, startFromHome, toggleSidebar, toggleTheme, checkUpdate, applyUpdate: updateBtnClick, toggleMenu, menuSettings, menuAbout, toggleVoice, toggleUserMenu, userProfile, userSwitch, userWorkspace, userLogout,
  toggleTabOverflow, tabCtxClose, tabCtxCloseOthers, tabCtxCloseRight, tabCtxCloseAll, winMinimize, winToggleMaximize, winClose,
  closeSettings, scfgSecretLock: scfgToggleSecretLock, scfgTgLock: scfgToggleTgLock, scfgQqLock: scfgToggleQqLock, scfgSelect: scfgSelectChannel, imcfgSave,
  mcfgClose: closeModelConfig, mcfgKeyLock: mcfgToggleKeyLock, mcfgSave: saveModelConfig,
  mcfgNew: mcfgNew, mcfgDelete: deleteModelProvider,
  userChangePassword, pwdClose, pwdExit, pwdSave, logoutCancel, logoutConfirm };
document.addEventListener("click", e=>{
  const el = e.target.closest("[data-act]");
  // 菜单切换类动作自己管开/关，不提前关（否则 openModelMenu 刚开就被关掉）
  const _menuToggle = ["openModelMenu","toggleMenu","toggleUserMenu","toggleTabOverflow"];
  if(!el || !_menuToggle.includes(el.dataset.act)){ closeMenu(); closeTabCtx(); }
  if(!el) return;
  const act = el.dataset.act;
  if(act === "runConnectorTool"){
    runConnectorTool(el.dataset.tool, el.dataset.online === "1");
  } else if(act === "approveTool"){
    approveTool(el.dataset.callid, el.dataset.ok === "1", el.dataset.all === "1");
  } else if(act === "toolCopy"){ toolCopy(el);
  } else if(act === "toolShare"){ toolShare(el);
  } else if(act === "toolLike"){ toolLike(el);
  } else if(act === "toolDislike"){ toolDislike(el);
  } else if(act === "pluginSelect"){ pluginSelect(el);
  } else if(act === "pluginOpenHomepage"){ pluginOpenHomepage(el);
  } else if(act === "pluginInstall"){ pluginInstall(el);
  } else if(act === "pluginUninstall"){ pluginUninstall(el);
  } else if(act === "pluginToggle"){ pluginToggle(el);
  } else if(act === "openModelMenu"){ openModelMenu(el);
  } else if(act === "setModelChoice"){ setModelChoice(el);
  } else if(act === "setProviderChoice"){ setProviderChoice(el);
  } else if(act === "providerSelect"){ mcfgSelectProvider(el);
  } else if(act === "openModelConfig"){ closeMenu(); openModelConfig();
  } else if(act === "toggleVoice"){
    toggleVoice(el);
  } else if(act === "showPanel"){
    showPanel(el.dataset.panel);
  } else if(act === "scfgSelect"){ scfgSelectChannel(el);
  } else if(act === "switchCat"){
    el.closest(".cats").querySelectorAll(".cat").forEach(b=>b.classList.remove("active"));
    el.classList.add("active");
  } else if(ACTIONS[act]){
    ACTIONS[act]();
  }
});

// 设置面板：平台下拉的 change 委托管不了（click-only）→ 显式监听；点遮罩空白处关闭（target
// 必须是遮罩自身，避免点进表单误关）。同样绕开被拦的内联 on* 属性。
document.getElementById("scfg-kind").addEventListener("change", scfgSwitchKind);
{
  const _scfgOverlay = document.getElementById("settings-overlay");
  _scfgOverlay.addEventListener("click", e => { if(e.target === _scfgOverlay) closeSettings(); });
}
// 模型面板：preset 下拉 / base-url 输入 / 温度滑杆的事件（内联 on* 被两壳 CSP 拦截，显式监听）；
// 点遮罩空白处关闭（target 必须是遮罩自身，避免点进表单误关）。
document.getElementById("mcfg-preset").addEventListener("change", e => mcfgApplyPreset(e.target.value));
document.getElementById("mcfg-base-url").addEventListener("input", mcfgRefreshCandidates);
document.getElementById("mcfg-temp").addEventListener("input", e => {
  document.getElementById("mcfg-temp-val").textContent = parseFloat(e.target.value).toFixed(2);
});
{
  const _mcfgOverlay = document.getElementById("mcfg-overlay");
  _mcfgOverlay.addEventListener("click", e => { if(e.target === _mcfgOverlay) closeModelConfig(); });
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
  console.log("[session_event] 监听已注册");
} else {
  console.log("[session_event] 未注册：window.__TAURI__.event 不可用");
}
