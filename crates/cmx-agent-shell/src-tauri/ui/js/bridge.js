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
// ── 默认权限（两旋钮）：🛡 下拉切换沙箱能力 × 审批许可（前门 set_policy，立即生效）。
// 选择记在 localStorage，启动时恢复（后端默认 workspace-write/on-request，由前端补投上次选择）──
document.addEventListener('change', async e => {
  const sel = e.target;
  if (!sel || sel.id !== 'perm') return;
  const [sandbox, approval] = sel.value.split('/');
  try { localStorage.setItem('cmx-perm', sel.value); } catch (_) {}
  const r = await call({cmd:'set_policy', sandbox, approval});
  if (!r || r.ok === false) showToast('切换权限失败：' + (r && r.error ? r.error.message : '未知错误'));
});
(function restorePerm(){
  const sel = document.getElementById('perm');
  let saved = null;
  try { saved = localStorage.getItem('cmx-perm'); } catch (_) {}
  if (!sel || !saved || ![...sel.options].some(o => o.value === saved)) return;
  sel.value = saved;
  const [sandbox, approval] = saved.split('/');
  call({cmd:'set_policy', sandbox, approval}).then(r => {
    if (!r || r.ok === false) console.warn('恢复权限档失败', r);
  });
})();
// ── 流式对话桥（同核多壳）：原生 Tauri 壳走 invoke("send_stream")+事件；Web 壳走 POST /api/stream 的 SSE。──
// onEvent 收到每个会话事件（含终止标记 {kind:"stream_done"} / {kind:"stream_error",message}）。
async function streamSend(sessionId, text, onEvent){
  if (window.__TAURI__ && window.__TAURI__.core){
    STREAMING.add(sessionId);   // 标记本地流式中 → session_event 监听跳过，避免双重渲染
    const chan = "agentstream-" + Date.now() + "-" + Math.random().toString(36).slice(2,8);
    const un = await window.__TAURI__.event.listen(chan, e => onEvent(e.payload));
    try { await window.__TAURI__.core.invoke("send_stream", { sessionId, text, channel: chan }); }
    finally { un(); STREAMING.delete(sessionId); }
    return;
  }
  // Web：fetch + ReadableStream 手动解析 SSE（\n\n 分帧，取 data: 行）
  const r = await fetch("/api/stream", {method:"POST", headers:{"Content-Type":"application/json"}, body: JSON.stringify({session_id:sessionId, text})});
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
}
function esc(s){ return String(s).replace(/[&<>]/g,c=>({"&":"&amp;","<":"&lt;",">":"&gt;"}[c])); }
function el(cls,html){ const d=document.createElement("div"); d.className=cls; if(html!=null)d.innerHTML=html; return d; }
