// ── 更新检查 / 更新安装（标题栏更新按钮 + 自绘弹窗 + 自动检查）──
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
    if(!silent){
    const msg=String(err);
    if(msg.includes("fallback platforms")){ showToast("更新服务暂未配置当前平台的安装包"); }
    else{ showToast("检查更新失败："+msg); }
  }
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
