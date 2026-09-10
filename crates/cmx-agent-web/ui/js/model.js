// —— 模型选择器（composer 的 ◎ 模型按钮）——
// 当前模型显示名（供 openSession 克隆模板后同步新 tab 的 .mlabel；模板是惰性 DOM，querySelectorAll 不到）
let _modelLabel = "";
function setModelLabel(name){
  const short = !name ? "未配置" : name==="demo" ? "Demo" : name.replace(/^deepseek-/,"").replace(/^gpt-/,"gpt-");
  _modelLabel = short;
  document.querySelectorAll(".model .mlabel").forEach(s=>s.textContent=short);
}
async function refreshModelLabel(){
  try{ const r=await call({cmd:"list_models"}); if(r&&r.ok) setModelLabel(r.data.current); }catch(e){}
}
async function openModelMenu(el){
  const menu=document.getElementById("modelmenu");
  if(menu.classList.contains("on")){ menu.classList.remove("on"); return; }
  closeMenu();
  const r=await call({cmd:"list_providers"});
  if(!r||!r.ok){ showToast("取模型列表失败"); return; }
  const d=r.data;
  // 多 provider 分组：每个 provider 一行（点击整体切换），激活条目下缩进列候选模型（点击换模型）。
  const parts=(d.providers||[]).map(p=>{
    const on=!!p.active;
    let html=`<div class="mi mm-head" data-act="setProviderChoice" data-id="${esc(p.id)}" style="cursor:pointer">`
      +`<span>${on?'✓ ':'&nbsp;&nbsp;&nbsp;'}${esc(p.name||p.id)}</span>`
      +`<span class="mm-b">${esc((p.model||"—")+(p.configured_key?"":" · 未填Key"))}</span></div>`;
    if(on){
      html+=(p.candidates||[]).map(c=>
        `<div class="mi mm-item" style="padding-left:26px" data-act="setModelChoice" data-model="${esc(c.model)}" data-pid="${esc(p.id)}"><span class="mm-ck">${c.model===p.model?'✓':''}</span> ${esc(c.label||c.model)}</div>`
      ).join("");
    }
    return html;
  }).join("");
  menu.innerHTML=parts
    +`<div class="mi mm-item" style="margin-top:4px;border-top:1px solid var(--border);padding-top:6px" data-act="openModelConfig">⚙ 配置模型…</div>`;
  // 锚到按钮上方
  const rect=el.getBoundingClientRect();
  menu.style.left=Math.max(8,rect.left)+"px";
  menu.style.bottom=(window.innerHeight-rect.top+6)+"px";
  menu.style.top="auto";
  menu.classList.add("on");
}
async function setProviderChoice(el){
  const id=el.dataset.id;
  document.getElementById("modelmenu").classList.remove("on");
  const r=await call({cmd:"set_active_provider", id});
  if(r&&r.ok){ showToast((r.data&&r.data.note)||"已切换"); refreshModelLabel(); }
  else { showToast("切换失败："+((r&&r.error&&r.error.message)||"未知")); }
}
async function setModelChoice(el){
  const model=el.dataset.model, pid=el.dataset.pid;
  document.getElementById("modelmenu").classList.remove("on");
  setModelLabel(model);
  const payload={cmd:"set_model", model};
  if(pid) payload.provider_id=pid;
  const r=await call(payload);
  if(r&&r.ok){ showToast("已切换模型 · "+((r.data&&r.data.note)||model)); }
  else { showToast("切换失败："+((r&&r.error&&r.error.message)||"未知")); refreshModelLabel(); }
}

// —— 模型配置面板（多 provider：列表+详情）——
const MCFG_PRESETS = {
  mlamp:    "https://llmgw-bz.mlamp.cn/v1",
};
const MCFG_CANDIDATES = {
  mlamp:    ["mlamp/deepseek-v4-flash","mlamp/qwen3-coder-next-fp8","mlamp/deepseek-v4-pro","mlamp/glm-5.2","mlamp/kimi-k3","mlamp/qwen3.8-27b","mlamp/minimax-h3"],
};
let _mcfgKeyUnlocked = false;
let _mcfgProviders = [];      // list_providers 快照（面板期间）
let _mcfgSelId = null;        // 当前选中条目 id；null = 新建态
let _mcfgIsNew = false;       // 「＋ 新增」后为 true，保存前选中项视为新条目

function mcfgLockKey(){
  _mcfgKeyUnlocked = false;
  const keyInput = document.getElementById("mcfg-api-key");
  const lockBtn  = document.getElementById("mcfg-key-lock-btn");
  keyInput.readOnly = true; keyInput.type = "password";
  lockBtn.textContent = "🔒";
}
async function openModelConfig(){
  closeMenu();
  mcfgLockKey();
  const r = await call({cmd:"list_providers"});
  if(!r||!r.ok){ showToast("读取配置失败"); return; }
  _mcfgProviders = r.data.providers || [];
  // 默认选中激活条目；无激活选第一个。
  _mcfgSelId = (_mcfgProviders.find(p=>p.active) || _mcfgProviders[0] || {}).id || null;
  _mcfgIsNew = false;
  mcfgRenderList();
  await mcfgFillDetail(_mcfgSelId);
  document.getElementById("mcfg-overlay").classList.remove("hidden");
}
function closeModelConfig(){
  document.getElementById("mcfg-overlay").classList.add("hidden");
}
// 左列渲染：provider 名 + （激活圆点 / 内置徽标）+ 当前模型副行；底部「＋ 新增」。
function mcfgRenderList(){
  const list=document.getElementById("mcfg-list");
  const items=_mcfgProviders.map(p=>{
    const on=p.id===_mcfgSelId && !_mcfgIsNew;
    return `<div class="mcfg-list-item${on?' on':''}" data-act="providerSelect" data-id="${esc(p.id)}">`
      +`<div style="min-width:0"><div class="lp-name">${esc(p.name||p.id)}</div>`
      +`<div class="lp-sub">${esc(p.model||"—")}</div></div>`
      +(p.active?'<span class="lp-dot" title="当前激活"></span>':"")
      +(p.builtin?'<span class="lp-badge">内置</span>':"")
      +`</div>`;
  }).join("");
  list.innerHTML=items+`<div class="mcfg-list-new" data-act="mcfgNew">＋ 新增 Provider</div>`;
}
// 右侧详情回填（切选中 / 重开面板共用）。id 为空 = 新建态清空表单。
async function mcfgFillDetail(id){
  mcfgLockKey();
  const nameInput=document.getElementById("mcfg-name");
  const hint=document.getElementById("mcfg-hint");
  const delBtn=document.getElementById("mcfg-del-btn");
  if(!id){
    nameInput.value=""; document.getElementById("mcfg-base-url").value="";
    document.getElementById("mcfg-api-key").value="";
    document.getElementById("mcfg-model").value="";
    document.getElementById("mcfg-temp").value=0.2;
    document.getElementById("mcfg-temp-val").textContent="0.20";
    document.getElementById("mcfg-timeout").value=60000;
    document.getElementById("mcfg-preset").value="";
    mcfgUpdateCandidates([]);
    delBtn.disabled=true;
    hint.textContent="新条目保存后不会自动激活，用上方 ◎ 菜单切换。";
    return;
  }
  const r=await call({cmd:"get_model_config", id});
  if(!r||!r.ok){ showToast("读取配置失败"); return; }
  const d=r.data;
  nameInput.value=d.name||"";
  document.getElementById("mcfg-base-url").value  = d.base_url || "";
  document.getElementById("mcfg-api-key").value    = d.api_key_masked || "";
  document.getElementById("mcfg-model").value      = d.model || "";
  const temp = typeof d.temperature === "number" ? d.temperature : 0.2;
  document.getElementById("mcfg-temp").value       = temp;
  document.getElementById("mcfg-temp-val").textContent = temp.toFixed(2);
  document.getElementById("mcfg-timeout").value    = d.timeout_ms || 60000;
  mcfgSetPreset(d.base_url || "");
  mcfgUpdateCandidates(d.candidates || []);
  delBtn.disabled = !!d.builtin;   // 内置不可删
  hint.textContent = d.builtin ? "内置预设：可填 Key / 换模型，不可删除。"
    : (d.active ? "当前激活。" : "未激活，保存后用 ◎ 菜单切换。");
}
// 切换左列选中：丢弃未保存的表单改动（与「取消」同语义，刻意不弹确认）。
async function mcfgSelectProvider(el){
  const id=el.dataset.id;
  if(id===_mcfgSelId && !_mcfgIsNew) return;
  _mcfgSelId=id; _mcfgIsNew=false;
  mcfgRenderList();
  await mcfgFillDetail(id);
}
// ＋ 新增：清空表单进入新建态（预设下拉可用于快速填充）。
function mcfgNew(){
  _mcfgIsNew=true; _mcfgSelId=null;
  mcfgRenderList();
  mcfgFillDetail(null);
  document.getElementById("mcfg-name").focus();
}
function mcfgToggleKeyLock(){
  const input = document.getElementById("mcfg-api-key");
  const btn   = document.getElementById("mcfg-key-lock-btn");
  if(_mcfgKeyUnlocked){
    mcfgLockKey();
    input.value = ""; // 锁回后清空，防止明文残留
  } else {
    input.readOnly = false; input.type = "text"; input.value = "";
    btn.textContent = "🔓"; _mcfgKeyUnlocked = true;
    input.focus();
  }
}
function mcfgApplyPreset(key){
  if(!key) return;
  document.getElementById("mcfg-base-url").value = MCFG_PRESETS[key] || "";
  mcfgUpdateCandidates(MCFG_CANDIDATES[key] || []);
}
function mcfgSetPreset(url){
  const sel = document.getElementById("mcfg-preset");
  const match = Object.entries(MCFG_PRESETS).find(([,v])=>url.includes(v.replace(/^https?:\/\//,"").split("/")[0]));
  sel.value = match ? match[0] : "";
}
function mcfgRefreshCandidates(){
  const url = document.getElementById("mcfg-base-url").value.toLowerCase();
  const key = Object.keys(MCFG_CANDIDATES).find(k=>url.includes(k));
  mcfgUpdateCandidates(key ? MCFG_CANDIDATES[key] : []);
}
function mcfgUpdateCandidates(list){
  const dl = document.getElementById("mcfg-model-list");
  dl.innerHTML = list.map(m=>`<option value="${esc(m)}">`).join("");
}
async function saveModelConfig(){
  const name    = document.getElementById("mcfg-name").value.trim();
  const baseUrl = document.getElementById("mcfg-base-url").value.trim();
  const model   = document.getElementById("mcfg-model").value.trim();
  const temp    = parseFloat(document.getElementById("mcfg-temp").value);
  const timeout = parseInt(document.getElementById("mcfg-timeout").value, 10);
  if(_mcfgIsNew && !name){ showToast("名称不能为空"); return; }
  if(!baseUrl){ showToast("Base URL 不能为空"); return; }
  if(!model){   showToast("模型名不能为空");   return; }
  const payload = {cmd:"set_model_config", base_url:baseUrl, model, temperature:temp, timeout_ms:timeout};
  if(_mcfgIsNew || name) payload.name = name;
  if(!_mcfgIsNew && _mcfgSelId) payload.id = _mcfgSelId;
  if(_mcfgKeyUnlocked){
    const key = document.getElementById("mcfg-api-key").value.trim();
    payload.api_key_action = "set";
    payload.api_key_value  = key;
  } else {
    payload.api_key_action = "keep";
  }
  const r = await call(payload);
  if(r&&r.ok){
    showToast("已保存 · "+((r.data&&r.data.note)||model));
    refreshModelLabel();
    // 保存后回到该条目（新建则选中新 id），刷新列表。
    _mcfgSelId = (r.data&&r.data.id) || _mcfgSelId;
    _mcfgIsNew = false;
    const lr = await call({cmd:"list_providers"});
    if(lr&&lr.ok) _mcfgProviders = lr.data.providers || [];
    mcfgRenderList();
    await mcfgFillDetail(_mcfgSelId);
  } else {
    showToast("保存失败："+((r&&r.error&&r.error.message)||"未知"));
  }
}
// 删除（仅自定义条目；内置按钮已禁用双保险）。
async function deleteModelProvider(){
  if(!_mcfgSelId || _mcfgIsNew) return;
  const p=_mcfgProviders.find(x=>x.id===_mcfgSelId);
  if(!p || p.builtin) return;
  const r=await call({cmd:"delete_provider", id:_mcfgSelId});
  if(r&&r.ok){
    showToast((r.data&&r.data.note)||"已删除");
    refreshModelLabel();
    const lr=await call({cmd:"list_providers"});
    if(lr&&lr.ok) _mcfgProviders = lr.data.providers || [];
    _mcfgSelId = (_mcfgProviders.find(x=>x.active) || _mcfgProviders[0] || {}).id || null;
    mcfgRenderList();
    await mcfgFillDetail(_mcfgSelId);
  } else {
    showToast("删除失败："+((r&&r.error&&r.error.message)||"未知"));
  }
}

// 状态图标（替代 exit 码文字）：成功 ✓ / 失败 ✕
const ICON_CHECK=`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>`;
const ICON_X=`<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><line x1="18" y1="6" x2="6" y2="18"/><line x1="6" y1="6" x2="18" y2="18"/></svg>`;
