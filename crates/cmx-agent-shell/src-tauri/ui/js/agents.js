// ── 设置中心「子智能体」分区（阶段一，方案 20260914 §6.7）──
// 列表：内置（不可删，只开放模型/启用）+ 自定义（可增删改）。
// 编辑器：名称 / 显示名 / 描述 / 工具（全部 | 白名单多选，选项来自 ListSkills=当前已注册工具）/
//         系统提示词 / 模型（继承默认 + ListProviders）/ 启用。
// 保存走 save_agent（内置=覆盖项 upsert；自定义=name upsert），保存即热生效（hot:true）。
let AGENTS_STATE = { builtin: [], custom: [], skills: [], providers: [], current: null, isNew: false };

async function agentsOpen(){
  try{
    const [a, s, p] = await Promise.all([
      call({cmd:"list_agents"}),
      call({cmd:"list_skills"}),
      call({cmd:"list_providers"}),
    ]);
    const d=(a&&a.ok&&a.data)||{};
    AGENTS_STATE.builtin = d.builtin||[];
    AGENTS_STATE.custom  = d.custom||[];
    AGENTS_STATE.skills  = ((s&&s.ok&&s.data)||{}).skills||[];
    AGENTS_STATE.providers = ((p&&p.ok&&p.data)||{}).providers||[];
  }catch(e){ showToast("加载子智能体失败："+(e&&e.message||e)); return; }
  agentsRenderList();
  // 缺省选中内置第一条（general-purpose）
  if(!AGENTS_STATE.current){
    const first = AGENTS_STATE.builtin[0];
    if(first) agentsSelect(first.name, true);
  } else {
    agentsSelect(AGENTS_STATE.current, true);
  }
}

function agentsRenderList(){
  const box=document.getElementById("agents-list"); if(!box) return;
  const item=(a,builtin)=>{
    const on = AGENTS_STATE.current===a.name ? " on":"";
    return `<div class="mcfg-list-item${on}" data-act="agentsSelect" data-name="${esc(a.name)}"${a.enabled?"":" style=\"opacity:.5\""}>`
      +`<div style="min-width:0"><div class="lp-name">${esc(a.title||a.name)}</div>`
      +`<div class="lp-sub">${esc(a.name)}</div></div>`
      +(builtin?'<span class="lp-badge">内置</span>':"")
      +`</div>`;
  };
  box.innerHTML =
    AGENTS_STATE.builtin.map(a=>item(a,true)).join("")
    + AGENTS_STATE.custom.map(a=>item(a,false)).join("")
    + `<div class="mcfg-list-new" data-act="agentsNew">＋ 新建子智能体</div>`;
}

function agentsFind(name){
  return AGENTS_STATE.builtin.find(a=>a.name===name)
      || AGENTS_STATE.custom.find(a=>a.name===name) || null;
}

function agentsSelect(name, keepCurrent){
  const a = agentsFind(name);
  if(!a) return;
  AGENTS_STATE.current = name;
  AGENTS_STATE.isNew = false;
  agentsRenderList();
  const g = id=>document.getElementById(id);
  const builtin = !!a.builtin;
  g("agents-name").value = a.name;
  g("agents-title").value = a.title||"";
  g("agents-desc").value = a.description||"";
  const allow = a.tools && a.tools.mode==="allow";
  g("agents-tools-mode").value = allow ? "allow" : "all";
  g("agents-tools-row").style.display = allow ? "" : "none";
  agentsRenderToolsBox(allow ? (a.tools.names||[]) : []);
  agentsRenderModelOptions(a.model||"");
  g("agents-prompt").value = a.system_prompt||"";
  g("agents-enabled").checked = !!a.enabled;
  // 内置：只开放 模型/启用；自定义：全开放
  ["agents-name","agents-title","agents-desc","agents-tools-mode","agents-prompt"].forEach(id=>{ g(id).disabled = builtin; });
  g("agents-tools-mode").disabled = builtin;
  g("agents-del-btn").style.display = builtin ? "none" : "";
  g("agents-hint").textContent = builtin ? "内置类型：仅可调整专属模型与启用开关" : "";
}

function agentsNew(){
  AGENTS_STATE.current = null;
  AGENTS_STATE.isNew = true;
  agentsRenderList();
  const g = id=>document.getElementById(id);
  g("agents-name").value=""; g("agents-name").disabled=false;
  g("agents-title").value=""; g("agents-title").disabled=false;
  g("agents-desc").value=""; g("agents-desc").disabled=false;
  g("agents-tools-mode").value="all"; g("agents-tools-mode").disabled=false;
  g("agents-tools-row").style.display="none";
  agentsRenderToolsBox([]);
  agentsRenderModelOptions("");
  g("agents-prompt").value=""; g("agents-prompt").disabled=false;
  g("agents-enabled").checked=true;
  g("agents-del-btn").style.display="none";
  g("agents-hint").textContent="新建自定义子智能体：填名称（snake_case）、显示名后保存";
  g("agents-name").focus();
}

// 工具白名单多选框（选项 = ListSkills 的当前已注册工具）
function agentsRenderToolsBox(checked){
  const box=document.getElementById("agents-tools-box"); if(!box) return;
  box.innerHTML = AGENTS_STATE.skills.map(s=>{
    const on = checked.includes(s.name);
    return `<label class="ag-tool${on?" on":""}"><input type="checkbox" value="${esc(s.name)}"${on?" checked":""}> ${esc(s.name)}</label>`;
  }).join("") || `<span class="mcfg-empty">（无可用工具）</span>`;
  box.querySelectorAll("input").forEach(i=>{
    i.addEventListener("change",()=>{ i.closest(".ag-tool").classList.toggle("on", i.checked); });
  });
}

// 模型下拉：继承默认 + provider 列表（id 作值）
function agentsRenderModelOptions(selected){
  const sel=document.getElementById("agents-model"); if(!sel) return;
  sel.innerHTML = `<option value="">继承默认（跟随当前模型）</option>`
    + AGENTS_STATE.providers.map(p=>`<option value="${esc(p.id)}">${esc(p.name)} · ${esc(p.model||"")}</option>`).join("");
  sel.value = selected||"";
}

function agentsToggleToolsMode(el){
  document.getElementById("agents-tools-row").style.display = el.value==="allow" ? "" : "none";
}

async function agentsSave(){
  const g = id=>document.getElementById(id);
  const name = g("agents-name").value.trim();
  if(!name){ showToast("名称不能为空"); return; }
  const cur = AGENTS_STATE.isNew ? null : agentsFind(AGENTS_STATE.current);
  const builtin = !!(cur && cur.builtin);
  const mode = g("agents-tools-mode").value;
  const names = [...g("agents-tools-box").querySelectorAll("input:checked")].map(i=>i.value);
  const spec = {
    name,
    title: g("agents-title").value.trim() || name,
    description: g("agents-desc").value.trim(),
    tools: mode==="allow" ? { mode:"allow", names } : { mode:"all" },
    model: g("agents-model").value || null,
    system_prompt: g("agents-prompt").value,
    enabled: g("agents-enabled").checked,
    builtin,
  };
  try{
    const r = await call({cmd:"save_agent", spec});
    if(!r.ok) throw new Error((r.error&&r.error.message)||"保存失败");
    showToast("已保存（热生效）");
    await agentsOpen();
    AGENTS_STATE.current = name;
    agentsSelect(name, true);
  }catch(e){ showToast("保存失败："+(e&&e.message||e)); }
}

async function agentsDelete(){
  const name = AGENTS_STATE.current;
  if(!name){ showToast("请先选择一个自定义子智能体"); return; }
  const a = agentsFind(name);
  if(a && a.builtin){ showToast("内置子智能体不可删除"); return; }
  // 两击确认（Tauri 壳禁原生气泡）：按钮短暂变为「确认删除？」，2.5s 后还原。
  const btn=document.getElementById("agents-del-btn");
  if(btn.dataset.confirm!=="1"){
    btn.dataset.confirm="1"; btn.textContent="确认删除？";
    setTimeout(()=>{ if(btn){ btn.dataset.confirm=""; btn.textContent="删除"; } }, 2500);
    return;
  }
  btn.dataset.confirm=""; btn.textContent="删除";
  try{
    const r = await call({cmd:"delete_agent", name});
    if(!r.ok) throw new Error((r.error&&r.error.message)||"删除失败");
    showToast("已删除");
    AGENTS_STATE.current = null;
    await agentsOpen();
  }catch(e){ showToast("删除失败："+(e&&e.message||e)); }
}

// 工具模式下拉切换（CSP 禁内联 onchange，显式绑定；defer 脚本执行时 DOM 已就绪）
{
  const _m = document.getElementById("agents-tools-mode");
  if(_m) _m.addEventListener("change", ()=>agentsToggleToolsMode(_m));
}
