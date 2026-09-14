// ── 设置中心（重构）：活动栏底部 logo 唯一入口，六分区两栏 ──
// 分组「账户与模型」：账户 / 模型（js/model.js 的 mcfg* 逻辑，DOM 平移 id 未动）/ IM 遥控（js/im.js 的 scfg* 逻辑）。
// 分组「通用」：外观（主题卡）/ 通用 / 关于。
// 打开即落上次分区（localStorage cmx-settings-section，首次=账户）；_pwdForced 强制改密态优先（拒绝打开）。
// 分区跳转 data-act="settingsSection" data-sec="…"；Esc 关闭；未登录红点 #logo-dot 由 refreshUser 维护。

const SETTINGS_SECTIONS = {
  account:    { title: "账户",     desc: "登录账号、密码与退出" },
  models:     { title: "模型",     desc: "模型服务（Provider）管理：内置预设 + 自定义" },
  im:         { title: "IM 遥控",  desc: "飞书 / QQ / 微信 机器人接入，保存热重载即时生效" },
  agents:     { title: "子智能体", desc: "子智能体类型管理：内置 + 自定义（工具白名单 / 提示词 / 专属模型），保存热生效" },
  appearance: { title: "外观",     desc: "主题外观" },
  general:    { title: "通用",     desc: "数据目录" },
  about:      { title: "关于",     desc: "版本与更新" },
};
let _settingsSection = null;

function settingsCurrentTheme(){
  return document.documentElement.getAttribute("data-theme")==="light" ? "light" : "dark";
}
// 渲染主题卡选中态（打开外观分区 / applyTheme 后调用）
function renderThemeCards(){
  const cur = settingsCurrentTheme();
  document.querySelectorAll("#theme-cards .theme-card").forEach(c=>{
    c.classList.toggle("on", c.dataset.t===cur);
  });
}
// 分区切换：左导航高亮 + 右侧整片切换 + 标题/说明。不写存储（写存储只在显式打开/点击时）。
function showSettingsSection(sec){
  const meta = SETTINGS_SECTIONS[sec] || SETTINGS_SECTIONS.account;
  _settingsSection = meta===SETTINGS_SECTIONS[sec] ? sec : "account";
  document.querySelectorAll("#settings-nav .settings-item").forEach(b=>{
    b.classList.toggle("on", b.dataset.sec===_settingsSection);
  });
  document.querySelectorAll(".settings-sec").forEach(s=>{
    s.classList.toggle("on", s.id==="settings-sec-"+_settingsSection);
  });
  document.getElementById("settings-title").textContent = meta.title;
  document.getElementById("settings-desc").textContent  = meta.desc;
  if(_settingsSection==="appearance") renderThemeCards();
  if(_settingsSection==="account")   renderAccountSection();
  if(_settingsSection==="about")     renderAboutSection();
}
// 打开设置中心（可指定分区，缺省=上次分区）。_pwdForced 时拒绝（改密弹框优先）。
async function openSettings(sec){
  if(typeof _pwdForced!=="undefined" && _pwdForced) return;   // 强制改密弹框不可绕过
  closeMenu();
  const overlay = document.getElementById("settings-overlay");
  if(overlay.classList.contains("hidden")){
    let saved = "account";
    try{ saved = localStorage.getItem("cmx-settings-section") || "account"; }catch(e){}
    showSettingsSection(SETTINGS_SECTIONS[sec] ? sec : saved);
    // 模型分区打开时按需装配（原 openModelConfig 的加载逻辑；平移后由分区首开触发）
    if(_settingsSection==="models" && typeof mcfgOpen==="function") await mcfgOpen();
    // IM 分区打开时按需装配（原 menuSettings 的读取回显逻辑）
    if(_settingsSection==="im" && typeof scfgOpen==="function") await scfgOpen();
    // 子智能体分区打开时按需装配（阶段一）
    if(_settingsSection==="agents" && typeof agentsOpen==="function") await agentsOpen();
    overlay.classList.remove("hidden");
  } else if(sec){
    showSettingsSection(sec);
  }
}
function closeSettings(){
  document.getElementById("settings-overlay").classList.add("hidden");
}
function toggleSettings(){
  const overlay = document.getElementById("settings-overlay");
  overlay.classList.contains("hidden") ? openSettings() : closeSettings();
}
// data-act="settingsSection"（左导航点击）：切分区 + 记忆 + 按需装配（模型/IM 分区的数据加载，
// 与 openSettings 打开时同款语义：每次进分区都重拉，对齐原「每次打开面板都刷新」的行为）。
function settingsSection(el){
  const sec = el.dataset.sec;
  try{ localStorage.setItem("cmx-settings-section", sec); }catch(e){}
  showSettingsSection(sec);
  if(sec==="models" && typeof mcfgOpen==="function") mcfgOpen();
  if(sec==="im" && typeof scfgOpen==="function") scfgOpen();
  if(sec==="agents" && typeof agentsOpen==="function") agentsOpen();
}
// 模型弹层/其它入口的分区直达（data-act="openSettingsSection" data-section="…"）
function openSettingsSection(el){
  openSettings(el.dataset.section);
}
function setThemeCard(el){
  applyTheme(el.dataset.t);
  renderThemeCards();
}

// ── 账户分区：身份卡（头像/昵称/@登录名/状态徽标/改密）+ 账户 ID 明细 + 通栏退出登录 ──
// CURRENT_USER 可见字段仅 user_id/username/nickname/roles/must_change_password（auth.rs public_json）。
function renderAccountSection(){
  const u = (typeof CURRENT_USER!=="undefined") ? CURRENT_USER : null;
  const label = u ? (u.nickname || u.username) : "未登录";
  document.getElementById("acc-av").textContent = u ? label.trim().charAt(0).toUpperCase() : "👤";
  document.getElementById("acc-name").textContent = label;
  // 状态徽标：绿点「已登录」/ 灰点「未登录」
  document.getElementById("acc-badge").classList.toggle("on", !!u);
  document.getElementById("acc-badge-txt").textContent = u ? "已登录" : "未登录";
  // 昵称下一行：@登录名（登录态）/ 引导文案（未登录）
  document.getElementById("acc-uid").textContent = u ? ("@"+u.username) : "登录 cmx 门户后同步账户信息";
  document.getElementById("acc-id").textContent = (u && u.user_id) || "—";
  // 修改密码：未登录无意义，随登录态显隐
  document.getElementById("acc-pwd-btn").style.display = u ? "" : "none";
}
function accChangePassword(){ userChangePassword(); }   // 复用 js/im.js 的改密弹框逻辑
function accLogout(){
  document.getElementById("logout-overlay").classList.remove("hidden");
}

// ── 关于分区：版本号 + 检查更新（手动入口唯一处，复用 update.js checkUpdate）──
function renderAboutSection(){
  document.getElementById("about-ver").textContent = "版本 " + APP_VERSION;
}
function aboutCheckUpdate(){ checkUpdate(false); }

// Esc 关闭设置中心（输入框聚焦时也响应；改密弹框等不受影响）
document.addEventListener("keydown", e=>{
  if(e.key==="Escape"){
    const overlay = document.getElementById("settings-overlay");
    if(overlay && !overlay.classList.contains("hidden")) closeSettings();
  }
});
// 点遮罩空白处关闭（target 必须是遮罩自身）
{
  const _ov = document.getElementById("settings-overlay");
  _ov.addEventListener("click", e=>{ if(e.target===_ov) closeSettings(); });
}
