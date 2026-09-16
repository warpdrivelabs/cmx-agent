// ── 设置中心「子智能体」分区（阶段一，方案 20260914 §6.7）──
// 列表：内置（不可删，只开放模型/启用）+ 自定义（可增删改）。
// 编辑器：名称 / 显示名 / 描述 / 工具（全部 | 白名单多选 | 黑名单排除，选项来自 ListSkills，
//         控制面三件 task/ask_user/exit_plan 子代理装配时必被收走，不列为选项）/
//         系统提示词 / 模型（继承默认 + ListProviders）/ 启用。
// 保存走 save_agent（内置=覆盖项 upsert；自定义=name upsert），保存即热生效（对在途子任务
// 不回溯——工具集/提示词在派发时快照）。同名冲突（新建撞既有/改名撞别的）两击确认才覆盖；
// 切换选中/重开分区丢弃未保存修改时 toast 提示（脏标记 dirty）。
let AGENTS_STATE = { builtin: [], custom: [], skills: [], providers: [], current: null, isNew: false, dirty: false };

// 控制面工具（对齐 cmx-agent-tools/src/task.rs 的 CONTROL_PLANE_TOOLS）：子代理一律收走，
// 类型显式勾选也不生效——列进白名单只会造成「勾了没用」的误导，直接不给选。
const AGENTS_CONTROL_PLANE_TOOLS = ["task", "ask_user", "exit_plan"];

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
    if(first) agentsSelect(first.name);
  } else {
    agentsSelect(AGENTS_STATE.current);
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
      +(!a.enabled?'<span class="lp-badge lp-badge-off">已停用</span>':"")
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

// 脏标记：编辑器任何输入即置位；切换选中/新建/重开分区会重载表单丢弃修改，此时提示一声。
function agentsResetDirty(msg){
  if(AGENTS_STATE.dirty){ AGENTS_STATE.dirty=false; if(msg) showToast(msg); }
}
function resetSaveBtn(){
  const btn=document.getElementById("agents-save-btn");
  if(btn){ btn.dataset.confirm=""; btn.textContent="保存"; }
}

function agentsSelect(name){
  const a = agentsFind(name);
  if(!a) return;
  agentsResetDirty("未保存的修改已丢弃");
  AGENTS_STATE.current = name;
  AGENTS_STATE.isNew = false;
  agentsRenderList();
  const g = id=>document.getElementById(id);
  const builtin = !!a.builtin;
  g("agents-name").value = a.name;
  g("agents-title").value = a.title||"";
  g("agents-desc").value = a.description||"";
  const mode = a.tools && (a.tools.mode==="allow"||a.tools.mode==="deny") ? a.tools.mode : "all";
  g("agents-tools-mode").value = mode;
  agentsSyncToolsRow(mode);
  // 名单始终按 spec 渲染（黑/白名单共用一份勾选态），模式切换只显隐行、不清勾选
  agentsRenderToolsBox((a.tools&&a.tools.names)||[]);
  // 内置：工具集/提示词编译进代码不可改；名单勾选改了也不落盘，一并禁掉
  g("agents-tools-box").querySelectorAll("input").forEach(i=>{
    i.disabled = builtin;
    i.closest(".ag-tool").classList.toggle("dis", builtin);
  });
  agentsRenderModelOptions(a.model||"");
  g("agents-prompt").value = a.system_prompt||"";
  g("agents-enabled").checked = !!a.enabled;
  ["agents-name","agents-title","agents-desc","agents-tools-mode","agents-prompt"].forEach(id=>{ g(id).disabled = builtin; });
  g("agents-del-btn").style.display = builtin ? "none" : "";
  resetSaveBtn();
  g("agents-hint").textContent = builtin ? "内置类型：仅可调整专属模型与启用开关" : "";
}

function agentsNew(){
  agentsResetDirty("未保存的修改已丢弃");
  AGENTS_STATE.current = null;
  AGENTS_STATE.isNew = true;
  agentsRenderList();
  const g = id=>document.getElementById(id);
  g("agents-name").value=""; g("agents-name").disabled=false;
  g("agents-title").value=""; g("agents-title").disabled=false;
  g("agents-desc").value=""; g("agents-desc").disabled=false;
  g("agents-tools-mode").value="all"; g("agents-tools-mode").disabled=false;
  agentsSyncToolsRow("all");
  agentsRenderToolsBox([]);
  agentsRenderModelOptions("");
  g("agents-prompt").value=""; g("agents-prompt").disabled=false;
  g("agents-enabled").checked=true;
  g("agents-del-btn").style.display="none";
  resetSaveBtn();
  g("agents-hint").textContent="新建自定义子智能体：填名称（snake_case）、显示名后保存";
  g("agents-name").focus();
}

// 工具白/黑名单多选框（选项 = ListSkills 的当前已注册工具，悬停看工具说明）
function agentsRenderToolsBox(checked){
  const box=document.getElementById("agents-tools-box"); if(!box) return;
  box.innerHTML = AGENTS_STATE.skills
    .filter(s=>!AGENTS_CONTROL_PLANE_TOOLS.includes(s.name))
    .map(s=>{
      const on = checked.includes(s.name);
      return `<label class="ag-tool${on?" on":""}" title="${esc(s.description||"")}"><input type="checkbox" value="${esc(s.name)}"${on?" checked":""}> ${esc(s.name)}</label>`;
    }).join("") || `<span class="mcfg-empty">（无可用工具）</span>`;
  box.querySelectorAll("input").forEach(i=>{
    i.addEventListener("change",()=>{ i.closest(".ag-tool").classList.toggle("on", i.checked); });
  });
  agentsApplyToolFilter();
}

// 名单筛选框：按子串过滤工具名（43+ 个工具平铺，不筛选没法用）
function agentsApplyToolFilter(){
  const f=document.getElementById("agents-tools-filter");
  const q=((f&&f.value)||"").trim().toLowerCase();
  document.querySelectorAll("#agents-tools-box .ag-tool").forEach(l=>{
    l.style.display = (!q || l.textContent.toLowerCase().includes(q)) ? "" : "none";
  });
}

// 模式行联动：显隐名单行 + 行标签跟随黑/白名单语义
function agentsSyncToolsRow(mode){
  const g=id=>document.getElementById(id);
  g("agents-tools-row").style.display = mode==="all" ? "none" : "";
  const lab=g("agents-tools-label");
  if(lab) lab.textContent = mode==="deny" ? "工具黑名单（排除所选）" : "工具白名单（仅所选）";
}
// 工具模式下拉切换：只切行显隐与标签，不清已勾选名单（黑白名单共用同一份勾选态）
function agentsToggleToolsMode(el){
  agentsSyncToolsRow(el.value);
}

// 模型下拉：继承默认 + provider 列表（id 作值）
function agentsRenderModelOptions(selected){
  const sel=document.getElementById("agents-model"); if(!sel) return;
  sel.innerHTML = `<option value="">继承默认（跟随当前模型）</option>`
    + AGENTS_STATE.providers.map(p=>`<option value="${esc(p.id)}">${esc(p.name)} · ${esc(p.model||"")}</option>`).join("");
  sel.value = selected||"";
}

async function agentsSave(){
  const g = id=>document.getElementById(id);
  const name = g("agents-name").value.trim();
  if(!name){ showToast("名称不能为空"); return; }
  const cur = AGENTS_STATE.isNew ? null : agentsFind(AGENTS_STATE.current);
  const builtin = !!(cur && cur.builtin);
  const mode = g("agents-tools-mode").value;
  const names = [...g("agents-tools-box").querySelectorAll("input:checked")].map(i=>i.value);
  // 白名单零勾选 = 子智能体零工具（起来后几乎必然失败），拦在前端给出明确原因
  if(!builtin && mode==="allow" && !names.length){ showToast("白名单模式至少勾选一个工具"); return; }
  // 同名冲突（新建撞既有自定义 / 改名撞别的自定义）：upsert 会静默覆盖，两击确认（对齐删除按钮）
  const btn=g("agents-save-btn");
  const conflict = !builtin && AGENTS_STATE.custom.find(a=>a.name===name && a.name!==(cur&&cur.name));
  if(conflict && btn.dataset.confirm!=="1"){
    btn.dataset.confirm="1"; btn.textContent="同名已存在，确认覆盖？";
    setTimeout(()=>{ if(btn){ btn.dataset.confirm=""; btn.textContent="保存"; } }, 2500);
    return;
  }
  resetSaveBtn();
  const spec = {
    name,
    title: g("agents-title").value.trim() || name,
    description: g("agents-desc").value.trim(),
    tools: mode==="all" ? { mode:"all" } : { mode, names },
    model: g("agents-model").value || null,
    system_prompt: g("agents-prompt").value,
    enabled: g("agents-enabled").checked,
    builtin,
  };
  try{
    const r = await call({cmd:"save_agent", spec});
    if(!r.ok) throw new Error((r.error&&r.error.message)||"保存失败");
    showToast("已保存（新派发的子任务生效）");
    AGENTS_STATE.dirty=false;
    AGENTS_STATE.current = name;   // 先改选中再重拉：agentsOpen 会按 current 选中，避免闪跳
    await agentsOpen();
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
    AGENTS_STATE.dirty=false;
    AGENTS_STATE.current = null;
    await agentsOpen();
  }catch(e){ showToast("删除失败："+(e&&e.message||e)); }
}

// 工具模式下拉切换（CSP 禁内联 onchange，显式绑定；defer 脚本执行时 DOM 已就绪）
{
  const _m = document.getElementById("agents-tools-mode");
  if(_m) _m.addEventListener("change", ()=>agentsToggleToolsMode(_m));
  const _f = document.getElementById("agents-tools-filter");
  if(_f) _f.addEventListener("input", agentsApplyToolFilter);
  // 脏标记：分区里任何编辑控件输入/切换即置位（筛选框不算编辑内容）
  const _sec = document.getElementById("settings-sec-agents");
  if(_sec){
    const _mark=e=>{ if(e.target && e.target.id!=="agents-tools-filter") AGENTS_STATE.dirty=true; };
    _sec.addEventListener("input", _mark);
    _sec.addEventListener("change", _mark);
  }
}
