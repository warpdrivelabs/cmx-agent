// ── 后端桥：同核多壳。原生 Tauri 壳走 invoke("agent")，Web 壳回退 HTTP POST /api。两者同达 dispatch_json。 ──
/** 双桥 JSON 前门：Tauri invoke agent 或 HTTP POST /api。@param req {{cmd:string,...}} @returns {Promise<{ok,data?,error?}>} */
async function call(req){
  if (window.__TAURI__ && window.__TAURI__.core){
    const raw = await window.__TAURI__.core.invoke("agent", { payload: JSON.stringify(req) });
    return JSON.parse(raw);
  }
  const r = await fetch("/api",{method:"POST",headers:{"Content-Type":"application/json"},body:JSON.stringify(req)});
  return await r.json();
}
// ── 权限三模式（方案 20260914 改造三）：输入框下拉「🛡 变更前确认 / 📋 计划模式 / ⚡ 完全访问」。
// confirm = 工作区可写 + 按需审批（原默认档）；plan = 安全档 + 会话级计划模式（守卫白名单默认拒，
// 比旧「只读」档更强）；full = 完全访问 + 从不打断。切档 = set_policy（全局两旋钮）+ 会话内联动
// set_plan_mode；选择记 localStorage（旧两旋钮值自动迁移），首页选择由 startFromHome 前置到新会话。──
const PERM_MAP = {
  confirm: { sandbox: "workspace-write", approval: "on-request", plan: false },
  plan:    { sandbox: "workspace-write", approval: "on-request", plan: true },
  full:    { sandbox: "danger-full-access", approval: "never", plan: false },
};
let permMode = "confirm";   // 全局当前档（plan 态本身按会话独立，见 _SESSION_META[...].plan_mode）
function permMigrate(v){
  if (v && PERM_MAP[v]) return v;
  if (v === "danger-full-access/never") return "full";
  if (v) return "confirm";            // 旧 workspace-write/on-request / read-only/on-request → confirm
  return null;
}
function permSaved(){ let v=null; try{ v=localStorage.getItem("cmx-perm"); }catch(_){} return permMigrate(v); }
/** 把某实例下拉回填到指定档（不落盘不广播；im-* 会话由调用方先禁 plan 项） */
function permSetSelect(sel, mode){ if (sel && PERM_MAP[mode] && !sel.querySelector("option[value='"+mode+"']:disabled")) sel.value = mode; }
/** 应用一个权限档：全局两旋钮 +（给 sessionId 时）联动会话级计划模式；成功后落盘并广播同步 */
async function applyPermMode(mode, sessionId){
  const m = PERM_MAP[mode]; if (!m) return false;
  const r = await call({cmd:"set_policy", sandbox:m.sandbox, approval:m.approval});
  if (!r || r.ok === false) { showToast("切换权限失败：" + (r && r.error ? r.error.message : "未知错误")); return false; }
  if (sessionId != null){
    const p = await call({cmd:"set_plan_mode", session_id:sessionId, enabled:m.plan});
    if (!p || p.ok === false) { showToast("切换计划模式失败：" + (p && p.error ? p.error.message : "未知错误")); return false; }
    if (_SESSION_META[sessionId]) _SESSION_META[sessionId].plan_mode = m.plan;
  }
  permMode = mode;
  try { localStorage.setItem("cmx-perm", mode); } catch(_){}
  document.dispatchEvent(new CustomEvent("cmx-perm-changed", { detail:{ mode } }));
  return true;
}
document.addEventListener("change", async e => {
  const sel = e.target;
  if (!sel || sel.getAttribute("data-role") !== "perm") return;
  // 用 contains 定位所属 tab（t.view 是 .tabview，select 在其内的 .session-view 里，不能直接比较）
  const t = TABS.find(x => x.view.contains(sel));
  await applyPermMode(sel.value, t ? t.sessionId : undefined);
});
// 各实例下拉同步：非计划态的会话下拉与首页下拉跟随全局档；计划中的会话保持「📋 计划模式」。
document.addEventListener("cmx-perm-changed", () => {
  document.querySelectorAll("select[data-role=perm]").forEach(sel => {
    const t = TABS.find(x => x.view.contains(sel));
    const isPlan = t && _SESSION_META[t.sessionId] && _SESSION_META[t.sessionId].plan_mode;
    if (!isPlan) permSetSelect(sel, permMode);
  });
});
// 启动恢复：上次选择补投后端（后端缺省 confirm 档）；各在册下拉回填。
(function restorePerm(){
  const mode = permSaved() || "confirm";
  permMode = mode;
  document.querySelectorAll("select[data-role=perm]").forEach(sel => permSetSelect(sel, mode));
  const m = PERM_MAP[mode];
  call({cmd:"set_policy", sandbox:m.sandbox, approval:m.approval}).then(r => {
    if (!r || r.ok === false) console.warn("恢复权限档失败", r);
  });
})();
// ── 流式对话桥（同核多壳）：原生 Tauri 壳走 invoke("send_stream")+事件；Web 壳走 POST /api/stream 的 SSE。──
// onEvent 收到每个会话事件（含终止标记 {kind:"stream_done"} / {kind:"stream_error",message}）。
async function streamSend(sessionId, text, onEvent, signal){
  if (window.__TAURI__ && window.__TAURI__.core){
    STREAMING.add(sessionId);   // 标记本地流式中 → session_event 监听跳过，避免双重渲染
    const chan = "agentstream-" + Date.now() + "-" + Math.random().toString(36).slice(2,8);
    const un = await window.__TAURI__.event.listen(chan, e => onEvent(e.payload));
    try { await window.__TAURI__.core.invoke("send_stream", { sessionId, text, channel: chan }); }
    finally { un(); STREAMING.delete(sessionId); }
    return;
  }
  // Web：fetch + ReadableStream 手动解析 SSE（\n\n 分帧，取 data: 行）。
  // 本地回合同样标记 STREAMING（与 Tauri 分支同规）：/api/subscribe 常驻通道会把本回合的
  // 落库事件也广播一遍，不标记会被 onSessionEvent 二次渲染（用户气泡/工具卡重复）。
  STREAMING.add(sessionId);
  try {
    const r = await fetch("/api/stream", {method:"POST", headers:{"Content-Type":"application/json"}, body: JSON.stringify({session_id:sessionId, text}), signal});
    // 必须检查状态：后端 401/500 返回 JSON 错误体时没有任何 data: 帧——旧实现静默结束，
    // 用户消息石沉大海且计时器空转。失败时合成 stream_error 走统一错误渲染。
    if (!r.ok) {
      onEvent({kind:"stream_error", message:"请求失败（HTTP " + r.status + "）"});
      return;
    }
    const reader = r.body.getReader(); const dec = new TextDecoder(); let buf = "";
    while(true){
      const {value, done} = await reader.read(); if(done) break;
      buf += dec.decode(value, {stream:true});
      let idx;
      while((idx = buf.indexOf("\n\n")) >= 0){
        const frame = buf.slice(0, idx); buf = buf.slice(idx+2);
        const dline = frame.split("\n").find(l => l.startsWith("data:"));
        if(!dline) continue;
        try { onEvent(JSON.parse(dline.slice(5).trim())); } catch(e){}
      }
    }
  } finally { STREAMING.delete(sessionId); }
}
function esc(s){ return String(s).replace(/[&<>"']/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"}[c])); }
function el(cls,html){ const d=document.createElement("div"); d.className=cls; if(html!=null)d.innerHTML=html; return d; }

