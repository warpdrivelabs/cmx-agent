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
// ── 更新：标题栏「更新」按钮仅当检测到新版本时才显示 ──
let UPDATE_INFO = null;   // {version, notes, force} 或 null（无可用更新）
let _updating = false;    // 防重入：下载安装进行中不再触发
function renderUpdateBtn(){
  const btn=document.getElementById("tb-update-btn");
  if(UPDATE_INFO){ btn.style.display=""; document.getElementById("tb-update-label").textContent="更新 "+UPDATE_INFO.version; }
  else { btn.style.display="none"; }
}
// 供 Tauri updater / 调试注入：window.__cmxUpdate("0.2.0") 让按钮出现；__cmxUpdate(null) 隐藏。
window.__cmxUpdate=(version)=>{ UPDATE_INFO=version?{version}:null; renderUpdateBtn(); };
// 标题栏「更新」按钮：不直接下载，先弹更新确认窗（复用检查更新的自绘弹窗），用户点「立即更新」才装。
function updateBtnClick(){
  if(!UPDATE_INFO) return;
  showUpdateDialog(UPDATE_INFO);
}
// 点标题栏「更新」按钮 = 下载 + 验签 + 安装 + 自动重启（Tauri updater 插件，壳内 update_common.rs）。
// 进度：壳 emit 的 update_progress 事件（received/total 累计字节）→ 按钮内联百分比（不做弹窗）。
function applyUpdate(){
  if(!UPDATE_INFO || _updating) return;
  _updating = true;
  const label=document.getElementById("tb-update-label");
  label.textContent="下载中…";
  (async()=>{
    let unlisten=null;
    if(window.__TAURI__ && window.__TAURI__.event){
      try{ unlisten = await window.__TAURI__.event.listen("update_progress", (e)=>{
        const p=e.payload||{};
        label.textContent = p.total ? "下载 "+Math.round(p.received/p.total*100)+"%"
                                    : "下载 "+Math.round(p.received/1024)+"KB";
      }); }catch(err){}
    }
    try{
      await window.__TAURI__.core.invoke("download_and_install");
      label.textContent="即将重启…";   // 壳内 install 后立刻 restart，通常见不到这一帧
    }catch(err){
      showToast("更新失败："+err);      // fail-closed：只 toast，主功能不受影响，可重试
      _updating=false; if(unlisten) unlisten(); renderUpdateBtn();
    }
  })();
}
// 更新提示自绘弹窗（浏览器原生 alert/confirm 在 WebView 里带「网页说」前缀且样式出戏，禁用）。
// force（强制更新）：无「稍后」、点遮罩不关——数据通路已通，服务端 force 随版本走。
function showUpdateDialog(info){
  const old=document.getElementById("cmx-update-modal"); if(old) old.remove();
  // 弹出即记「已提示」：30 分钟内静默检查不重复弹同一版本（登录页→主窗跨窗防重，不依赖用户点「稍后」；
  // 手动菜单检查不受限；force 不受限）
  try{ localStorage.setItem("cmx-update-snooze",JSON.stringify({version:info.version,ts:Date.now()})); }catch(e){}
  const esc=(s)=>String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const mask=document.createElement("div");
  mask.id="cmx-update-modal"; mask.className="cmx-modal-mask";
  mask.innerHTML='<div class="cmx-modal" role="dialog" aria-modal="true">'
    +'<div class="cmx-modal-title"><span class="dot"></span>发现新版本 '+esc(info.version)+'</div>'
    +'<div class="cmx-modal-body">'+(esc(info.notes)||"暂无更新说明")+'</div>'
    +'<div class="cmx-modal-ops">'
    +(info.force?"":'<button class="cmx-btn" data-x="later">稍后</button>')
    +'<button class="cmx-btn cmx-btn-primary" data-x="now">立即更新</button>'
    +'</div></div>';
  document.body.appendChild(mask);
  requestAnimationFrame(()=>mask.classList.add("on"));
  const close=()=>{ mask.classList.remove("on"); setTimeout(()=>mask.remove(),200); };
  mask.addEventListener("click",(e)=>{
    const act=e.target.closest("[data-x]");
    if(act && act.dataset.x==="now"){ close(); renderUpdateBtn(); applyUpdate(); }
    else if(act && act.dataset.x==="later"){ close(); }   // 标记已在弹出时写入
    else if(!info.force && e.target===mask){ close(); }
  });
}
// 通用信息弹窗（浏览器原生 alert 在 WebView 里带「网页说」前缀且样式出戏，全站禁用；复用更新弹窗的模态样式）
function infoDialog(title, body){
  const old=document.getElementById("cmx-info-modal"); if(old) old.remove();
  const esc=(s)=>String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const mask=document.createElement("div");
  mask.id="cmx-info-modal"; mask.className="cmx-modal-mask";
  mask.innerHTML='<div class="cmx-modal" role="dialog" aria-modal="true">'
    +'<div class="cmx-modal-title"><span class="dot"></span>'+esc(title)+'</div>'
    +'<div class="cmx-modal-body">'+esc(body)+'</div>'
    +'<div class="cmx-modal-ops"><button class="cmx-btn cmx-btn-primary" data-x="ok">知道了</button></div></div>';
  document.body.appendChild(mask);
  requestAnimationFrame(()=>mask.classList.add("on"));
  const close=()=>{ mask.classList.remove("on"); setTimeout(()=>mask.remove(),200); };
  mask.addEventListener("click",(e)=>{
    const act=e.target.closest("[data-x]");
    if((act && act.dataset.x==="ok") || e.target===mask) close();
  });
}
// 「已提示」标记查询（写入口：showUpdateDialog / 登录页 showUpd，弹出即写）
function updateSnoozed(v){
  try{
    const s=JSON.parse(localStorage.getItem("cmx-update-snooze")||"null");
    return !!(s && s.version===v && Date.now()-s.ts<30*60*1000);
  }catch(e){ return false; }
}
// 检查更新（cmx 菜单项 / 启动自动检查）：有新版 → 自绘弹窗（用户确认才装）+ 标题栏按钮。
// silent=true：启动 5s 后自动检查——失败无感（fail-closed），有新版同样弹自绘窗；无新版零打扰。
// 登录页已提示过的同版本（30 分钟内），静默检查只出标题栏按钮不再弹（手动菜单检查 = 用户点名，照弹）。
async function checkUpdate(silent){
  if(!silent) closeMenu();
  if(!(window.__TAURI__ && window.__TAURI__.core)){ if(!silent) showToast("Web 壳无需自动更新"); return; }
  try{
    const r = JSON.parse(await window.__TAURI__.core.invoke("check_update"));
    if(r && r.ok && r.data && r.data.update_available){
      UPDATE_INFO = {version:r.data.version, notes:r.data.notes, force:r.data.force};
      renderUpdateBtn();
      if(UPDATE_INFO.force || !silent || !updateSnoozed(UPDATE_INFO.version)) showUpdateDialog(UPDATE_INFO);
    } else {
      UPDATE_INFO = null; renderUpdateBtn();
      if(!silent) showToast("当前已是最新版本（"+APP_VERSION+"）");
    }
  }catch(err){
    UPDATE_INFO = null; renderUpdateBtn();
    if(!silent) showToast("检查更新失败："+err);   // 静默检查失败：无感（fail-closed）
  }
}
// 启动自动检查（登录页是首查点，此处兜底：登录页查时未发布、登录过程中才发布的窗口期）。
// 主窗隐藏（登录未完成）时不查不弹——弹窗挂在不可见窗里，用户会在登录后看到"重复弹出"；
// 等本窗首次可见（登录完成亮窗）再兜底查，此时登录页的「已提示」标记已落，同一版本不会重弹。
let _updChecked=false;
function autoUpdateCheck(){
  if(_updChecked) return;
  if(!(window.__TAURI__ && window.__TAURI__.core)) return;
  if(document.visibilityState==="hidden") return;
  _updChecked=true;
  checkUpdate(true);
}
setTimeout(autoUpdateCheck, 5000);                              // 已登录直开主窗：5s 例行检查
document.addEventListener("visibilitychange", autoUpdateCheck); // 登录完成亮窗：兜底检查

// cmx 底部菜单：开合 / 设置 / 关于
function toggleMenu(){ document.getElementById("usermenu").classList.remove("on"); document.getElementById("cmxmenu").classList.toggle("on"); }
function closeMenu(){ document.getElementById("cmxmenu").classList.remove("on"); document.getElementById("usermenu").classList.remove("on"); }
// ── Windows/Linux 窗口控件（最小化/最大化/关闭）+ 无边框缩放 ──
// macOS 用系统红绿灯（Overlay 标题栏），不建这些；仅非 macOS 且在 Tauri 壳内才生效（Web 壳无 __TAURI__.window）。
function _tauriWin(){
  return (window.__TAURI__ && window.__TAURI__.window) ? window.__TAURI__.window.getCurrentWindow() : null;
}
async function winMinimize(){ const w=_tauriWin(); if(w) await w.minimize(); }
async function winToggleMaximize(){
  const w=_tauriWin(); if(!w) return;
  await w.toggleMaximize();
  try{
    const maxed = await w.isMaximized();
    document.getElementById("win-max-btn").querySelector(".ico").textContent = maxed ? "❐" : "□";
  }catch(e){}
}
async function winClose(){ const w=_tauriWin(); if(w) await w.close(); }
// 无边框缩放热区：pointerdown 触发 Tauri 原生 startResizeDragging（比手搓 CSS resize 在各平台更可靠）。
// macOS 用系统边框缩放（decorations=Overlay 保留系统 resize），resize-handle 在 mac 上通过 CSS 隐藏，不干扰。
document.querySelectorAll(".resize-handle").forEach(h=>{
  h.addEventListener("pointerdown", async e=>{
    e.preventDefault();
    const w=_tauriWin(); if(!w) return;
    try{ await w.startResizeDragging(h.dataset.resize); }catch(e){}
  });
});
// 平台探测：非 macOS 才显示自绘窗口控件 + 缩放热区（decorations:false 仅在 tauri.windows/linux.conf.json 生效）。
// 用自定义 `platform` 命令（= std::env::consts::OS）而非 tauri-plugin-os：省一份插件依赖/首次联网编译。
(async function initPlatformChrome(){
  if(!(window.__TAURI__ && window.__TAURI__.core)) return; // Web 壳：系统浏览器窗口，不处理
  try{
    const platform = await window.__TAURI__.core.invoke("platform"); // "macos" | "windows" | "linux"
    if(platform !== "macos"){
      document.body.classList.add("no-native-decor");
      document.getElementById("win-ctrls").style.display = "flex";
      // login 视图窗口三键 + 拖拽条（同条件显示）
      const lwc = document.getElementById("login-win-ctrls");
      if(lwc) lwc.style.display = "flex";
      const lds = document.querySelector(".login-drag-strip");
      if(lds) lds.style.display = "block";
    }
  }catch(e){}
})();

// 用户菜单
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
  if(!el){ closeMenu(); closeTabCtx(); document.getElementById("modelmenu").classList.remove("on"); document.getElementById("tab-dropdown").classList.remove("on"); return; }
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
    }
  } catch(e) {}
}, 2000);

// 登录后主窗口被显示时刷新用户信息：原生壳监听 Tauri "logged-in" 事件；并兜底监听窗口 focus。
if(window.__TAURI__ && window.__TAURI__.event){ window.__TAURI__.event.listen("logged-in", ()=>{ location.hash = "#/"; showMainView(); refreshUser(); refreshTasks(); }); }
window.addEventListener("focus", ()=>{ refreshUser(); });

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
