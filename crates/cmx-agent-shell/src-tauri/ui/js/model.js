// —— 模型选择器（composer 的 ◎ 模型按钮）——
// 单测图标（lucide flask-conical 线描）：跟 ✎/✕ 同为单色 currentColor——彩色 emoji
// 🧪 在单色动作列里太扎眼（用户反馈 2026-09-17）。
const ICON_TEST = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M10 2v7.527a2 2 0 0 1-.211.896L4.72 20.55a1 1 0 0 0 .9 1.45h12.76a1 1 0 0 0 .9-1.45l-5.069-10.127A2 2 0 0 1 14 9.527V2"/><path d="M8.5 2h7"/><path d="M7 16h10"/></svg>`;
// 当前模型显示名（供 openSession 克隆模板后同步新 tab 的 .mlabel；模板是惰性 DOM，querySelectorAll 不到）
let _modelLabel = "";
function setModelLabel(name){
  // 空或 demo 占位（后端无已配置 Provider 时的 DemoModel）统一显示「未配置」。
  const short = !name || name==="demo" ? "未配置" : name.replace(/^deepseek-/,"").replace(/^gpt-/,"gpt-");
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
  // 多 provider 分组：每个 provider 一行（点击整体切换），其下直接展开候选模型（点击换模型
  // 并激活该供应商）——原先只有激活供应商才展开，未配置状态下无从选型（用户反馈 2026-09-17）。
  // ✓ 只标激活供应商的当前模型；思考/视觉用文字 tag（与设置页模型清单一致，不用 emoji）。
  const parts=(d.providers||[]).map(p=>{
    const on=!!p.active;
    let html=`<div class="mi mm-head" data-act="setProviderChoice" data-id="${esc(p.id)}" style="cursor:pointer">`
      +`<span>${on?'✓ ':'&nbsp;&nbsp;&nbsp;'}${esc(p.name||p.id)}</span>`
      +`<span class="mm-b">${esc((p.model||"—")+(p.configured_key?"":" · 未填Key"))}</span></div>`;
    html+=(p.candidates||[]).map(c=>{
      const tags=((c.input_types||[]).includes("image")?`<span class="mm-tag">视觉</span>`:"");   // 只标视觉；思考不展示（用户 2026-09-17）
      return `<div class="mi mm-item" style="padding-left:26px" data-act="setModelChoice" data-model="${esc(c.model)}" data-pid="${esc(p.id)}"><span class="mm-ck">${on&&c.model===p.model?'✓':''}</span> ${esc(c.label||c.model)}${tags}</div>`;
    }).join("");
    return html;
  }).join("");
  menu.innerHTML=parts
    +`<div class="mi mm-item" style="margin-top:4px;border-top:1px solid var(--border);padding-top:6px" data-act="openSettingsSection" data-section="models">⚙ 管理模型…</div>`;
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

// —— 模型配置面板（多 provider：列表+详情；ZCode 化 P1，方案 20260917）——
// 模板真源在后端（list_provider_presets 下发），前端零硬编码；候选模型一律来自 providers.json
// 的 models 清单（base_url 关键词硬编码已下线）。无「主模型」概念：model 字段=当前使用中
// 模型，仅由聊天 ◎ 选择器维护，配置页只管清单（后端在停用/删除当前模型时自动回落启用项）。
let _mcfgProviders = [];      // list_providers 快照（面板期间）
let _mcfgSelId = null;        // 当前选中条目 id；null = 新建态
let _mcfgIsNew = false;       // 「＋ 新增」后为 true，保存前选中项视为新条目
let _mcfgModels = [];         // 表单期模型清单 [{id,enabled,…,reasoning_levels,reasoning_params}]（行内编辑写回，保存提交）
let _mcfgKind = "openai";     // 协议类型（P1 恒 openai，随条目回显）
let _mcfgPreset = "";         // 来源模板 id（目录选择/条目回显）
let _mcfgPresets = null;      // 模板目录缓存（list_provider_presets，懒加载）

async function mcfgLoadPresets(){
  if(_mcfgPresets) return _mcfgPresets;
  const r = await call({cmd:"list_provider_presets"});
  _mcfgPresets = (r&&r.ok&&r.data&&r.data.presets) || [];
  return _mcfgPresets;
}
async function mcfgOpen(){
  const r = await call({cmd:"list_providers"});
  if(!r||!r.ok){ showToast("读取配置失败"); return; }
  _mcfgProviders = r.data.providers || [];
  // 默认选中激活条目；无激活选第一个。
  _mcfgSelId = (_mcfgProviders.find(p=>p.active) || _mcfgProviders[0] || {}).id || null;
  _mcfgIsNew = false;
  mcfgRenderList();
  await mcfgFillDetail(_mcfgSelId);
}
// 关闭模型分区 = 关设置中心（由 js/settings.js 的 closeSettings 统一管遮罩；此处保留别名防旧调用）。
function closeModelConfig(){ closeSettings(); }
// 左列渲染：provider 名 + （激活圆点 / 内置徽标）+ 模型数副行；底部「＋ 新增」。
function mcfgRenderList(){
  const list=document.getElementById("mcfg-list");
  const items=_mcfgProviders.map(p=>{
    const on=p.id===_mcfgSelId && !_mcfgIsNew;
    const n=(p.models||[]).length;
    return `<div class="mcfg-list-item${on?' on':''}" data-act="providerSelect" data-id="${esc(p.id)}">`
      +`<div style="min-width:0"><div class="lp-name">${esc(p.name||p.id)}</div>`
      +`<div class="lp-sub">${esc(n?n+" 个模型":(p.model||"—"))}</div></div>`
      +(p.active?'<span class="lp-dot" title="当前激活"></span>':"")
      +(p.builtin?'<span class="lp-badge">内置</span>':"")
      +`</div>`;
  }).join("");
  list.innerHTML=items+`<div class="mcfg-list-new" data-act="mcfgNew">＋ 新增 Provider</div>`;
}
// 右侧详情回填（切选中 / 重开面板共用）。id 为空 = 新建态清空表单。
// API Key = ZCode 同款密码框：reveal 明文回填（password 掩码成圆点、长度=真实长度），
// 点眼睛看完整字符串；dataset.masked 记回填明文，保存时「非空且 ≠ 回填值」才 set。
async function mcfgFillDetail(id){
  const keyInput=document.getElementById("mcfg-api-key");
  keyInput.dataset.masked = "";
  const nameInput=document.getElementById("mcfg-name");
  const hint=document.getElementById("mcfg-hint");
  const delBtn=document.getElementById("mcfg-del-btn");
  const keyUrl=document.getElementById("mcfg-key-url");
  if(!id){
    nameInput.value=""; document.getElementById("mcfg-base-url").value="";
    keyInput.value="";
    document.getElementById("mcfg-kind").value = _mcfgKind = "openai";
    keyUrl.style.display="none"; keyUrl.dataset.url="";
    _mcfgModels=[]; _mcfgKind="openai"; _mcfgPreset="";
    mcfgRenderModels();
    delBtn.disabled=true;
    hint.textContent="新条目保存后不会自动激活，用 ◎ 菜单切换。";
    return;
  }
  const r=await call({cmd:"get_model_config", id, reveal:true});
  if(!r||!r.ok){ showToast("读取配置失败"); return; }
  const d=r.data;
  nameInput.value=d.name||"";
  document.getElementById("mcfg-base-url").value  = d.base_url || "";
  // 明文回填（用户拍板「点眼睛展示完整字符串」）：password 型平时掩码成圆点（长度=真实
  // 长度），点眼睛即见全串；dataset.masked 基准=明文——未动=keep，编辑=set 新值。
  keyInput.value    = d.api_key || "";
  keyInput.dataset.masked = d.api_key || "";
  _mcfgKind = d.kind || "openai";
  document.getElementById("mcfg-kind").value = _mcfgKind;
  if(d.api_key_url){ keyUrl.style.display=""; keyUrl.dataset.url=d.api_key_url; }
  else { keyUrl.style.display="none"; keyUrl.dataset.url=""; }
  _mcfgModels=(d.models||[]).map(m=>({id:m.id, enabled:m.enabled!==false,
    context_window:m.context_window||null, max_output_tokens:m.max_output_tokens||null,
    input_types:m.input_types||[], capabilities:m.capabilities||[],
    reasoning_levels:m.reasoning_levels||[], reasoning_params:m.reasoning_params||""}));
  _mcfgKind=d.kind||"openai"; _mcfgPreset=d.preset||"";
  mcfgRenderModels();
  delBtn.disabled = !!d.builtin;   // 内置不可删
  hint.textContent = d.builtin ? "内置预设：可填 Key / 管理模型清单，不可删除。"
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
// ＋ 新增：先弹供应商目录（选模板预填；自定义端点 = 空表单，方案 §5.3 新流程）。
async function mcfgNew(){
  _mcfgIsNew=true; _mcfgSelId=null;
  mcfgRenderList();
  mcfgFillDetail(null);
  await mcfgCatalogOpen();
}
// ── 供应商目录弹层 ──
async function mcfgCatalogOpen(){
  const presets=await mcfgLoadPresets();
  const grid=document.getElementById("mcfg-cat-grid");
  grid.innerHTML=presets.map(p=>{
    const n=(p.models||[]).length;
    return `<button type="button" class="cat-card" data-act="mcfgCatPick" data-id="${esc(p.id)}">`
      +`<span class="cat-name">${esc(p.name)}</span>`
      +`<span class="cat-desc">${esc(p.description||"")}</span>`
      +`<span class="cat-meta">${n?esc(n)+" 个预置模型":"手动添加模型"}</span></button>`;
  }).join("");
  document.getElementById("mcfg-cat-overlay").classList.remove("hidden");
}
function mcfgCatalogClose(){ document.getElementById("mcfg-cat-overlay").classList.add("hidden"); }
// 选模板：预填名称（新建态重名时后端会拦，先给唯一候选名）/ 地址 / Key 链接 / 模型清单。
function mcfgCatalogPick(el){
  const p=(_mcfgPresets||[]).find(x=>x.id===el.dataset.id);
  mcfgCatalogClose();
  if(!p) return;
  if(p.id==="custom"){ mcfgFillDetail(null); document.getElementById("mcfg-name").focus(); return; }
  document.getElementById("mcfg-name").value = p.name||"";
  document.getElementById("mcfg-base-url").value = p.base_url||"";
  const keyUrl=document.getElementById("mcfg-key-url");
  if(p.api_key_url){ keyUrl.style.display=""; keyUrl.dataset.url=p.api_key_url; }
  else { keyUrl.style.display="none"; keyUrl.dataset.url=""; }
  _mcfgModels=(p.models||[]).map(m=>({id:m.id, enabled:true,
    input_types:m.input_types||[], reasoning_levels:m.reasoning_levels||[], reasoning_params:""}));   // 目录模板元数据随带
  _mcfgKind="openai"; _mcfgPreset=p.id;
  document.getElementById("mcfg-kind").value = _mcfgKind;
  mcfgRenderModels();
  document.getElementById("mcfg-api-key").focus();
}
// ── 模型清单（ZCode 式：行内只留 ID/单测/编辑/删除/启用开关；能力设置进「编辑」弹窗）──
function mcfgRenderModels(){
  const box=document.getElementById("mcfg-models");
  // 动作列固定宽度逐行对齐（ZCode 式规整）：单测/✎/✕ 各 22px，图标全部单色线描。
  // 视觉 tag（ZCode 同款）：input_types 含 image = 视觉模型，行内小 pill 标识（弹窗里改，即时生效）。
  box.innerHTML=_mcfgModels.map((m,i)=>
    `<div class="ml-row" data-i="${i}">`
    +`<span class="ml-id" title="${esc(m.id)}">${esc(m.id)}</span>`
    +((m.input_types||[]).includes("image")?`<span class="ml-tag" title="支持图片输入（视觉）">视觉</span>`:"")
    +`<button type="button" class="ml-t" data-act="mcfgModelTest" title="单独测试这个模型">${ICON_TEST}</button>`
    +`<button type="button" class="ml-e" data-act="mcfgModelEdit" title="编辑模型设置（上下文 / 输入类型 / 推理等级…）">✎</button>`
    +`<button type="button" class="ml-del" data-act="mcfgModelDel" title="从清单删除">✕</button>`
    +`<label class="ml-en" title="关 = 保留配置但不进聊天选择器"><input type="checkbox" ${m.enabled?"checked":""}>启用</label>`
    +`</div>`).join("")
    ||`<div class="ml-empty">清单为空——点「＋ 新增模型」添加</div>`;
}
function mcfgModelAdd(){
  mcfgEditOpen(-1);   // ＋ 新增模型：直接进编辑弹窗（ZCode 式），保存时才加入清单
}
function mcfgModelDel(el){
  const i=+el.closest(".ml-row").dataset.i;
  _mcfgModels.splice(i,1);
  mcfgRenderModels();
}
function mcfgModelEdit(el){
  mcfgEditOpen(+el.closest(".ml-row").dataset.i);
}
async function mcfgModelTest(el){
  const i=+el.closest(".ml-row").dataset.i;
  const m=_mcfgModels[i];
  if(m) await mcfgTest(m.id, el);
}

// ── 模型编辑弹窗（ZCode 图二设置项：ID / 上下文窗口 / 最大输出 Token / 高级配置（输入类型）
// / 推理等级 / 推理参数映射）。模型能力 chips 已删（用户拍板）：存量值保存时透传，无编辑入口。
// _mcfgEditIdx=-1 = 新增态。──
const MED_INPUT_TYPES=[["text","文本"],["image","图片"],["video","视频"],["pdf","PDF"]];
const MED_REASONING_PRESETS=["low","high","max"];
let _mcfgEditIdx=-1;          // 当前编辑的清单下标；-1 = 新增
let _medLevels=[];            // 推理等级暂存（chips + 自定义）
function medChipRow(boxId, items, sel){
  const box=document.getElementById(boxId);
  box.innerHTML=items.map(([v,label])=>
    `<label class="med-chip${sel.includes(v)?" on":""}"><input type="checkbox" data-v="${v}" ${sel.includes(v)?"checked":""}>${esc(label)}</label>`).join("");
}
function medRenderLevelChips(){
  const box=document.getElementById("med-reasoning-levels");
  const known=new Set(MED_REASONING_PRESETS);
  const custom=_medLevels.filter(l=>!known.has(l));
  box.innerHTML=MED_REASONING_PRESETS.map(l=>
    `<label class="med-chip${_medLevels.includes(l)?" on":""}"><input type="checkbox" data-v="${esc(l)}" ${_medLevels.includes(l)?"checked":""}>${esc(l)}</label>`).join("")
    +custom.map(l=>`<label class="med-chip on"><input type="checkbox" data-v="${esc(l)}" checked>${esc(l)}</label>`).join("")
    +`<input id="med-level-new" type="text" placeholder="自定义…"><button type="button" data-act="medLevelAdd" title="添加自定义等级">＋</button>`;
  box.querySelectorAll('input[type=checkbox]').forEach(cb=>cb.addEventListener("change",()=>{
    const v=cb.dataset.v;
    if(cb.checked){ if(!_medLevels.includes(v)) _medLevels.push(v); }
    else { _medLevels=_medLevels.filter(x=>x!==v); }
    medRenderLevelChips();
  }));
  // 点 chip 标签文本也能切换（checkbox 被视觉隐藏时 label 点击原生生效，无需额外处理；
  // 这里仅阻断双触发）。自定义输入回车 = 添加。
  const inp=box.querySelector("#med-level-new");
  if(inp) inp.addEventListener("keydown",e=>{ if(e.key==="Enter"){ e.preventDefault(); medLevelAdd(); } });
}
function mcfgEditOpen(idx){
  _mcfgEditIdx=idx;
  const m=idx>=0 ? _mcfgModels[idx] : {};
  document.getElementById("med-title").childNodes[0].textContent = idx>=0 ? "编辑模型 " : "新增模型";
  document.getElementById("med-id").value = m.id||"";
  document.getElementById("med-ctx").value = m.context_window||"";
  document.getElementById("med-maxout").value = m.max_output_tokens||"";
  medChipRow("med-input-types", MED_INPUT_TYPES, (m.input_types&&m.input_types.length)?m.input_types:["text"]);
  // 推理等级：优先 reasoning_levels；旧数据只有 reasoning 布尔时给占位 high。
  _medLevels=(m.reasoning_levels&&m.reasoning_levels.length)?m.reasoning_levels.slice():(m.reasoning?["high"]:[]);
  medRenderLevelChips();
  document.getElementById("med-params").value = m.reasoning_params||"";
  document.getElementById("med-hint").textContent="";
  document.getElementById("med-overlay").classList.remove("hidden");
  document.getElementById("med-id").focus();
}
function medClose(){ document.getElementById("med-overlay").classList.add("hidden"); }
function medLevelAdd(){
  const inp=document.getElementById("med-level-new");
  const v=(inp.value||"").trim();
  if(!v) return;
  if(!_medLevels.includes(v)) _medLevels.push(v);
  medRenderLevelChips();
}
function medSave(){
  const id=document.getElementById("med-id").value.trim();
  if(!id){ showToast("模型 ID 不能为空"); return; }
  const dup=_mcfgModels.findIndex((m,i)=>m.id===id && i!==_mcfgEditIdx);
  if(dup>=0){ showToast("模型「"+id+"」已在清单中"); return; }
  const params=document.getElementById("med-params").value.trim();
  if(params){ try{ JSON.parse(params); }catch(e){ showToast("推理参数映射不是合法 JSON"); return; } }
  const pickChips=(boxId)=>[...document.querySelectorAll("#"+boxId+" input[type=checkbox]:checked")].map(c=>c.dataset.v);
  const entry={
    id,
    enabled: _mcfgEditIdx>=0 ? _mcfgModels[_mcfgEditIdx].enabled : true,   // 启用开关在清单行，弹窗不碰
    reasoning: _medLevels.length>0,
    context_window: parseInt(document.getElementById("med-ctx").value,10)||null,
    max_output_tokens: parseInt(document.getElementById("med-maxout").value,10)||null,
    input_types: pickChips("med-input-types"),
    capabilities: _mcfgEditIdx>=0 ? (_mcfgModels[_mcfgEditIdx].capabilities||[]) : [],   // 能力无编辑入口：存量透传
    reasoning_levels: _medLevels.slice(),
    reasoning_params: params,
  };
  if(_mcfgEditIdx>=0) _mcfgModels[_mcfgEditIdx]=entry;
  else _mcfgModels.push(entry);
  medClose();
  mcfgRenderModels();
}
// ── 测试连接（后端 test_model_config：真实发 max_tokens=1，同时验 key/URL/模型名）──
// modelId 传入 = 清单行单测。进行中/结果走悬浮条（ZCode 同款）；行内按钮只把烧瓶换成
// 同尺寸转圈——原先 textContent 改「测试中…」会在 22px 定宽按钮里竖排撑高行，且完成后
// textContent 回填会把 SVG 图标抹掉（按钮"消失"，2026-09-17）。
let _floatTimer=null;
function mcfgFloatShow(html){
  const f=document.getElementById("mcfg-float"); if(!f) return;
  if(_floatTimer){ clearTimeout(_floatTimer); _floatTimer=null; }
  // 整条 innerHTML 重建：进行中=转圈+文案；结果=✓/✗+文案（转圈只在进行中出现）。
  // 点悬浮条任意位置立即关闭。
  f.innerHTML=html;
  if(!f._bound){ f._bound=true; f.addEventListener("click",()=>mcfgFloatHide(0)); }
  f.hidden=false;
}
function mcfgFloatHide(delayMs){
  if(_floatTimer){ clearTimeout(_floatTimer); _floatTimer=null; }
  _floatTimer=setTimeout(()=>{ const f=document.getElementById("mcfg-float"); if(f) f.hidden=true; }, delayMs||0);
}
async function mcfgTest(modelId, btn){
  const baseUrl=document.getElementById("mcfg-base-url").value.trim();
  if(!baseUrl){ showToast("先填 Base URL"); return; }
  const keyVal=document.getElementById("mcfg-api-key").value.trim();
  const keyOrig=(document.getElementById("mcfg-api-key").dataset.masked||"").trim();
  const payload={cmd:"test_model_config", base_url:baseUrl};
  const m = modelId || (_mcfgModels.find(x=>x.enabled) || _mcfgModels[0] || {}).id;
  if(m) payload.model=m;
  // key 语义与保存一致：改动了（非空且 ≠ 回填脱敏值）= 用新值测；否则用已存 key 测（需条目已存在）。
  if(keyVal && keyVal!==keyOrig){ payload.api_key_action="set"; payload.api_key_value=keyVal; }
  else if(!_mcfgIsNew && _mcfgSelId){ payload.id=_mcfgSelId; payload.api_key_action="keep"; }
  const pname=document.getElementById("mcfg-name").value.trim()||"当前供应商";
  mcfgFloatShow(`<span class="tc-spin"></span><span>正在测试 ${esc(pname)} / ${esc(m||"…")}…</span>`);
  let btnSvg="";
  if(btn){ btn.disabled=true; btnSvg=btn.innerHTML; btn.innerHTML=`<span class="tc-spin"></span>`; }
  let r;
  try{ r=await call(payload); } finally { if(btn){ btn.disabled=false; btn.innerHTML=btnSvg; } }
  if(r&&r.ok&&r.data&&r.data.ok){
    mcfgFloatShow(`<span class="mf-ico ok">✓</span><span>连接成功（${((r.data.latency_ms/1000)).toFixed(1)} 秒）${m?" · "+esc(m):""}</span>`);
    mcfgFloatHide(4000);
  } else {
    let msg=(r&&r.data&&r.data.message)||(r&&r.error&&r.error.message)||"测试失败";
    mcfgFloatShow(`<span class="mf-ico bad">✗</span><span>${esc(String(msg).split("（原文：")[0].trim()||"测试失败")}</span>`);
    mcfgFloatHide(8000);
  }
}
async function saveModelConfig(){
  const name    = document.getElementById("mcfg-name").value.trim();
  const baseUrl = document.getElementById("mcfg-base-url").value.trim();
  if(_mcfgIsNew && !name){ showToast("名称不能为空"); return; }
  if(!baseUrl){ showToast("Base URL 不能为空"); return; }
  if(!_mcfgModels.length){ showToast("至少添加一个模型"); return; }
  // 温度/超时不再从 UI 传（「高级」区已移除）：后端对更新沿用旧值、新建用默认。
  const payload = {cmd:"set_model_config", base_url:baseUrl,
    kind:_mcfgKind, preset:_mcfgPreset,
    api_key_url:(document.getElementById("mcfg-key-url").dataset.url||"").trim(),
    models:_mcfgModels.map(m=>({id:m.id, enabled:!!m.enabled,
      context_window:m.context_window||null, max_output_tokens:m.max_output_tokens||null,
      input_types:m.input_types||[], capabilities:m.capabilities||[],
      reasoning_levels:m.reasoning_levels||[], reasoning_params:m.reasoning_params||""}))};
  if(_mcfgIsNew || name) payload.name = name;
  if(!_mcfgIsNew && _mcfgSelId) payload.id = _mcfgSelId;
  // API Key 密码框：回填的脱敏串未动（或清空）= keep；用户输入了新值 = set。
  const keyInput = document.getElementById("mcfg-api-key");
  const keyVal = keyInput.value.trim();
  const keyOrig = (keyInput.dataset.masked || "").trim();
  if(keyVal && keyVal !== keyOrig){
    payload.api_key_action = "set";
    payload.api_key_value  = keyVal;
  } else {
    payload.api_key_action = "keep";
  }
  const r = await call(payload);
  if(r&&r.ok){
    showToast("已保存 · "+((r.data&&r.data.note)||""));
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
