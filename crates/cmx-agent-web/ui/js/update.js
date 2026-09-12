// ── 更新检查 / 更新安装（标题栏更新按钮 + 自绘弹窗 + 自动检查 + 重试闭环）──
// P1+P2（下载重试与断点续传方案 §2.2）：壳命令 download_and_install 已自持化——
// 外环 4 轮退避重试 + 断点下载（.part 续传）+ 回环安装全在壳内；本层只负责：
// 确认窗 / 重试窗 / agreed 标记 / 30 分钟自动再试的闭环。
let UPDATE_INFO = null;   // {version, notes, force} 或 null（无可用更新）
let _updating = false;    // 防重入：下载安装进行中不再触发（壳内 INSTALLING 双保险）
let _lastAutoTry = 0;     // 上次自动再试时刻（回前台再试的 5 分钟门槛）
let _autoTimer = null;    // 30 分钟自动再试 interval 句柄（只挂一次）

// 「已同意」标记（R14）：点「立即更新」即写入；成功/撤回清除、失败保留（自动再试依据）。
// 存 localStorage：SPA 刷新 / 登录视图切换不丢；壳命令自持化，仅凭标记即可恢复重试（T12）。
function setUpdateAgreed(v){
  try{ v ? localStorage.setItem("cmx-update-agreed", JSON.stringify({version:v, ts:Date.now()}))
         : localStorage.removeItem("cmx-update-agreed"); }catch(e){}
}
function getUpdateAgreed(){
  try{ return JSON.parse(localStorage.getItem("cmx-update-agreed")||"null"); }catch(e){ return null; }
}
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
// 下载 + 安装（壳内 4 轮重试 + 断点续传 + 回环安装，这里只等结果）。
// 进度：壳 emit 的 update_progress {received, total, attempt}（received 已含续传偏移）→ 按钮内联百分比；
// attempt>1 时文案标「重试N」（新一轮断点续传，百分比从上次断点起跳）。
// auto=true（自动再试）：失败只 toast 不弹窗（防打扰循环）；手动路径失败弹重试窗。
// 结果闭环（R14）：invoke Ok = 撤回（安装成功壳直接 restart，前端收不到返回）→ 清标记+按钮；
// Err = 4 轮全败 → 标记保留（自动再试兜底）。
async function applyUpdate(auto){
  if(_updating) return;
  if(!auto && !UPDATE_INFO) return;
  _updating = true;
  const label=document.getElementById("tb-update-label");
  label.textContent = auto ? "自动重试中…" : "下载中…";
  (async()=>{
    let unlisten=null;
    if(window.__TAURI__ && window.__TAURI__.event){
      try{ unlisten = await window.__TAURI__.event.listen("update_progress", (e)=>{
        const p=e.payload||{};
        const att=p.attempt||1;
        const n = p.total ? Math.round(p.received/p.total*100)+"%"
                          : Math.round(p.received/1024)+"KB";
        label.textContent = att>1 ? "重试"+att+" "+n : "下载 "+n;
      }); }catch(err){}
    }
    try{
      await window.__TAURI__.core.invoke("download_and_install");
      // Ok = 版本撤回（204）：清 agreed + 按钮（update_withdrawn 事件监听会 toast，这里静默）。
      setUpdateAgreed(null); UPDATE_INFO=null; _updating=false; if(unlisten) unlisten(); renderUpdateBtn();
    }catch(err){
      // 4 轮重试全败：agreed 保留（30 分钟自动再试兜底）；手动弹重试窗，自动只 toast。
      _updating=false; if(unlisten) unlisten(); renderUpdateBtn();
      if(auto){ showToast("自动更新重试失败："+err); }
      else{ showRetryDialog(String(err)); }
    }
  })();
}
// 更新提示自绘弹窗（浏览器原生 alert/confirm 在 WebView 里带「网页说」前缀且样式出戏，禁用）。
// force（强制更新）：无「稍后」、点遮罩不关——数据通路已通，服务端 force 随版本走。
function showUpdateDialog(info){
  const old=document.getElementById("cmx-update-modal"); if(old) old.remove();
  // 弹出即记「已提示」：30 分钟内静默检查不重复弹同一版本（登录视图→主视图跨切防重，不依赖用户点「稍后」；
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
    if(act && act.dataset.x==="now"){
      close(); renderUpdateBtn();
      setUpdateAgreed(info.version);   // R14：用户已同意安装——之后失败由 30 分钟自动再试兜底
      applyUpdate(false);
    }
    else if(act && act.dataset.x==="later"){ close(); }   // snooze 标记已在弹出时写入（agreed 不受影响）
    else if(!info.force && e.target===mask){ close(); }
  });
}
// 更新失败重试窗（方案 §2.2：最终失败不再只 toast）：重试=再调壳命令（外环重新走）；
// 稍后=关窗（agreed 已保留，30 分钟自动再试兜底；标题栏按钮常驻可手动重试）。
function showRetryDialog(reason){
  const old=document.getElementById("cmx-retry-modal"); if(old) old.remove();
  const esc=(s)=>String(s??"").replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c]));
  const mask=document.createElement("div");
  mask.id="cmx-retry-modal"; mask.className="cmx-modal-mask";
  mask.innerHTML='<div class="cmx-modal" role="dialog" aria-modal="true">'
    +'<div class="cmx-modal-title"><span class="dot"></span>更新下载失败</div>'
    +'<div class="cmx-modal-body">已自动重试 4 次仍未成功：<br>'+esc(reason)+'</div>'
    +'<div class="cmx-modal-ops">'
    +'<button class="cmx-btn" data-x="later">稍后</button>'
    +'<button class="cmx-btn cmx-btn-primary" data-x="retry">重试</button>'
    +'</div></div>';
  document.body.appendChild(mask);
  requestAnimationFrame(()=>mask.classList.add("on"));
  const close=()=>{ mask.classList.remove("on"); setTimeout(()=>mask.remove(),200); };
  mask.addEventListener("click",(e)=>{
    const act=e.target.closest("[data-x]");
    if(act && act.dataset.x==="retry"){ close(); renderUpdateBtn(); applyUpdate(false); }
    else if(act && act.dataset.x==="later"){ close(); }
    else if(e.target===mask){ close(); }
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
// 「已提示」标记查询（写入口：showUpdateDialog，弹出即写）
function updateSnoozed(v){
  try{
    const s=JSON.parse(localStorage.getItem("cmx-update-snooze")||"null");
    return !!(s && s.version===v && Date.now()-s.ts<30*60*1000);
  }catch(e){ return false; }
}
// 检查更新（cmx 菜单项 / 启动自动检查）：有新版 → 自绘弹窗（用户确认才装）+ 标题栏按钮。
// silent=true：启动 5s 后自动检查——失败无感（fail-closed），有新版同样弹自绘窗；无新版零打扰。
// 登录视图已提示过的同版本（30 分钟内），静默检查只出标题栏按钮不再弹（手动菜单检查 = 用户点名，照弹）。
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
// 启动自动检查（登录视图是首查点，此处兜底：首查时未发布、登录过程中才发布的窗口期）。
// 登录未完成（主视图隐藏）时不查不弹——弹窗挂在不可见界面里，用户会在登录后看到"重复弹出"；
// 等首次可见（登录完成亮窗）再兜底查，此时「已提示」标记已落，同一版本不会重弹。
let _updChecked=false;
function autoUpdateCheck(){
  if(_updChecked) return;
  if(!(window.__TAURI__ && window.__TAURI__.core)) return;
  if(document.visibilityState==="hidden") return;
  _updChecked=true;
  checkUpdate(true);
}
setTimeout(autoUpdateCheck, 5000);                              // 已登录直开主视图：5s 例行检查
document.addEventListener("visibilitychange", autoUpdateCheck); // 登录完成亮窗：兜底检查

// 版本撤回事件（壳外环 check 拿 204 时 emit）：清 agreed + 按钮 + toast（R13/R14 闭环）。
(async()=>{
  if(window.__TAURI__ && window.__TAURI__.event){
    try{
      await window.__TAURI__.event.listen("update_withdrawn", ()=>{
        setUpdateAgreed(null); UPDATE_INFO=null; renderUpdateBtn();
        showToast("更新已撤回");
      });
    }catch(e){}
  }
})();

// 已同意版本的自动再试（P1）：30 分钟 interval + 回前台（距上次尝试 >5 分钟）。
// 只认 agreed 标记——壳命令自持化（自己 check），无需 UPDATE_INFO；SPA 刷新后标记仍在即恢复重试。
// 再次最终失败只 toast（applyUpdate(auto=true)），标题栏按钮常驻可手动重试。
function tryAutoRetry(){
  const a=getUpdateAgreed();
  if(!a || _updating) return;
  _lastAutoTry=Date.now();
  applyUpdate(true);
}
function ensureAutoRetryLoop(){
  if(_autoTimer) return;
  _autoTimer=setInterval(()=>{ if(getUpdateAgreed() && !_updating) tryAutoRetry(); }, 30*60*1000);
}
document.addEventListener("visibilitychange", ()=>{
  if(document.visibilityState!=="visible") return;
  if(getUpdateAgreed() && !_updating && Date.now()-_lastAutoTry>5*60*1000) tryAutoRetry();
});
ensureAutoRetryLoop();

// cmx 底部菜单：开合 / 设置 / 关于
