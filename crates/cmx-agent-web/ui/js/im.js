// ── 设置 → IM 遥控连接配置（后端 im.json；Tauri 壳 invoke("im_config"）· 多通道：飞书/QQ/微信 可同时在线 ──
let _scfgSecretUnlocked=false, _scfgQqUnlocked=false;
let _scfgSel="feishu"; // 当前选中的通道（左列高亮 + 右侧详情），独立于勾选启用态
// 各通道勾选启用态（暂存源：勾选框在左列动态生成，每次重渲染按此回填）。
let _scfgEnabled = { feishu:false, qq:false, wechat:false };
let _scfgWxPolling=false; // 微信扫码轮询进行中（关面板/登录成功/终态错误时置停）
// credEl = 凭证状态徽标读取的输入框（有值 = 已配；微信是只读 bot_id，扫码后才非空）。
const SCFG_CHANNELS=[
  {id:"feishu",   name:"飞书",      block:"scfg-feishu-block",   credKey:"app_secret_masked",     credEl:"scfg-app-id"},
  {id:"qq",       name:"QQ 机器人", block:"scfg-qq-block",       credKey:"qq_secret_masked",      credEl:"scfg-qq-app-id"},
  {id:"wechat",   name:"微信",      block:"scfg-wechat-block",   credKey:"wechat_token_masked",   credEl:"scfg-wx-bot-id"},
];
// 设置中心「IM 遥控」分区装配（原 menuSettings）：读 im_config 回显；Web 壳提示落「账户」分区。
// DOM 已平移进设置中心（scfg-* id 全保留），保存/扫码逻辑不变。
async function scfgOpen(){
  if(!(window.__TAURI__ && window.__TAURI__.core)){
    showSettingsSection("account");
    infoDialog("IM 遥控", "当前仅在桌面版（Tauri 壳）可配置：Web 壳未装配 IM 桥。");
    return;
  }  scfgResetLocks();
  let r=null;
  try{ r = JSON.parse(await window.__TAURI__.core.invoke("im_config",{action:"get"})); }catch(e){ showToast("读取失败："+e); return; }
  if(!r||!r.ok){ showToast("读取 IM 配置失败："+((r&&r.error&&r.error.message)||"未知")); return; }
  const d=r.data;
  // 多通道：回显 active 勾选；旧文件无 active 时按 kind 单通道回显。
  // 先把「启用态」记到暂存变量——勾选框是 scfgRenderList 动态生成的，此时还不存在。
  const active = (d.active&&d.active.length) ? d.active : (d.kind?[d.kind]:[]);
  _scfgEnabled = { feishu:active.includes("feishu"), qq:active.includes("qq"), wechat:active.includes("wechat") };
  document.getElementById("scfg-enabled").checked= d.enabled!==false;
  // 无人值守全权（默认开）：旧配置无该字段时后端 masked()/默认 JSON 均回 true。
  document.getElementById("scfg-full-access").checked = d.full_access!==false;
  document.getElementById("scfg-app-id").value  = d.app_id||"";
  document.getElementById("scfg-app-secret").value = d.app_secret_masked||"";
  document.getElementById("scfg-base").value    = d.base||"";
  document.getElementById("scfg-qq-app-id").value = d.qq_app_id||"";
  document.getElementById("scfg-qq-secret").value = d.qq_secret_masked||"";
  document.getElementById("scfg-wx-bot-id").value = d.wechat_bot_id||"";
  // 微信/QQ 扫码 UI 复位（上次会话的二维码/状态不残留）。
  document.getElementById("scfg-wx-qr-wrap").style.display="none";
  document.getElementById("scfg-wx-status").textContent="";
  _scfgWxPolling=false;
  document.getElementById("scfg-qq-qr-wrap").style.display="none";
  document.getElementById("scfg-qq-status").textContent="";
  _scfgQqPolling=false;
  const note=[];
  if(d.env_active) note.push("⚠ 检测到环境变量 CMX_AGENT_IM_*（开发模式）优先生效，此处保存的配置暂不生效。");
  if(!d.configured) note.push("尚未配置：勾选通道并填入凭证，保存即启用遥控。勾选的通道同时在线。");
  document.getElementById("scfg-note").textContent = note.join("  ·  ");
  // 默认选中：激活列表第一个通道（没有则飞书）。
  _scfgSel = active[0] || "feishu";
  scfgRenderList();
  scfgSwitchKind();
}
// 左列渲染：通道名 + 启用勾选 + 凭证状态（已配/未配）。复用模型面板 mcfg-list 样式。
function scfgRenderList(){
  const list=document.getElementById("scfg-list");
  list.innerHTML=SCFG_CHANNELS.map(ch=>{
    const en=!!_scfgEnabled[ch.id];
    const hasCred = !!document.getElementById(ch.credEl).value.trim();
    const on=ch.id===_scfgSel;
    return `<div class="mcfg-list-item${on?' on':''}" data-act="scfgSelect" data-id="${ch.id}">`
      +`<div style="min-width:0;display:flex;align-items:center;gap:7px">`
      +`<input type="checkbox" id="scfg-en-${ch.id}" ${en?"checked":""} data-scfg-cb="1" title="勾选 = 启用并在线">`
      +`<span class="lp-name">${ch.name}</span></div>`
      +(hasCred?'<span class="lp-dot" title="凭证已填"></span>':'<span class="lp-badge">未配</span>')
      +`</div>`;
  }).join("");
  // 勾选框：change 写回暂存态并刷新徽标；click 阻止冒泡（否则触发父行 scfgSelect 切详情）。
  list.querySelectorAll('input[data-scfg-cb]').forEach(cb=>{
    cb.addEventListener("change", e=>{ _scfgEnabled[cb.closest("[data-id]").dataset.id]=cb.checked; scfgRenderList(); });
    cb.addEventListener("click", e=>e.stopPropagation());
  });
}
// 切换左列选中：右侧只显示所选通道的凭证块（勾选启用与否不影响查看，只影响保存是否上线）。
function scfgSelectChannel(el){
  const id=el.dataset.id;
  if(id===_scfgSel) return;
  _scfgSel=id;
  scfgSwitchKind();
}
function scfgSwitchKind(){
  for(const ch of SCFG_CHANNELS){
    document.getElementById(ch.block).style.display = ch.id===_scfgSel ? "" : "none";
  }
  document.getElementById("scfg-list").querySelectorAll(".mcfg-list-item").forEach(el=>{
    el.classList.toggle("on", el.dataset.id===_scfgSel);
  });
}
function closeSettings(){ _scfgWxPolling=false; _scfgQqPolling=false; document.getElementById("settings-overlay").classList.add("hidden"); }
function scfgResetLocks(){
  _scfgSecretUnlocked=false; _scfgQqUnlocked=false;
  const s=document.getElementById("scfg-app-secret"), sb=document.getElementById("scfg-secret-lock");
  const q=document.getElementById("scfg-qq-secret"), qb=document.getElementById("scfg-qq-lock");
  s.readOnly=true; s.type="password"; s.value=""; sb.textContent="🔒";
  q.readOnly=true; q.type="password"; q.value=""; qb.textContent="🔒";
}
function scfgToggleSecretLock(){
  const input=document.getElementById("scfg-app-secret"), btn=document.getElementById("scfg-secret-lock");
  if(_scfgSecretUnlocked){ input.readOnly=true; input.type="password"; input.value=""; btn.textContent="🔒"; _scfgSecretUnlocked=false; }
  else { input.readOnly=false; input.type="text"; input.value=""; btn.textContent="🔓"; _scfgSecretUnlocked=true; input.focus(); }
}
function scfgToggleQqLock(){
  const input=document.getElementById("scfg-qq-secret"), btn=document.getElementById("scfg-qq-lock");
  if(_scfgQqUnlocked){ input.readOnly=true; input.type="password"; input.value=""; btn.textContent="🔒"; _scfgQqUnlocked=false; }
  else { input.readOnly=false; input.type="text"; input.value=""; btn.textContent="🔓"; _scfgQqUnlocked=true; input.focus(); }
}
// 通道勾选/选中切换由 scfgRenderList / scfgSelectChannel 负责（左列动态生成，显式监听）。
// ── 扫码登录共用：内容 → 二维码图（GIF data-url；组件未加载/编码失败在状态行兜底提示）。
// 微信（iLink）与 QQ（OpenClaw 绑定）的二维码内容都是**字符串**（URL），前端渲染。 ──
function scfgRenderQr(content, img, wrap, st){
  if(typeof qrcode==="undefined"){ st.textContent="二维码组件未加载（js/vendor/qrcode.min.js）"; return false; }
  try{
    const q=qrcode(0,"M"); q.addData(content); q.make();
    img.onload=()=>{ wrap.style.display=""; };
    img.onerror=()=>{ st.textContent="二维码渲染失败"; };
    img.src=q.createDataURL(6,8);
    wrap.style.display="";
    return true;
  }catch(e){ st.textContent="二维码渲染失败"; return false; }
}
// ── 微信扫码登录（iLink）：bot_token 只能扫码获得，无法手填。后端只下发二维码**内容
// 字符串**（qrcode_img_content 实测是授权页 URL，非图片）→ 前端用 vendor 的 qrcode-generator
// 渲染成 data-url。start 取码展示 → ~2s 一次 poll（waiting/scaned/refreshed/confirmed）。
// confirmed 后端已把凭证落盘 im.json 并热重载（note 带结果说明），这里同步勾选态 +
// 回显 bot_id——用户点「保存」只固化其余字段，不点也已生效。 ──
async function scfgWechatLogin(){
  if(_scfgWxPolling){ showToast("扫码进行中，请稍候"); return; }
  const btn=document.getElementById("scfg-wx-btn"), st=document.getElementById("scfg-wx-status");
  const wrap=document.getElementById("scfg-wx-qr-wrap"), img=document.getElementById("scfg-wx-qr");
  btn.disabled=true; st.textContent="正在获取二维码…"; wrap.style.display="none";
  let r=null;
  try{ r=JSON.parse(await window.__TAURI__.core.invoke("im_wechat_login",{action:"start"})); }
  catch(e){ const m="获取二维码失败："+e; btn.disabled=false; st.textContent=m; return showToast(m); }
  if(!r||!r.ok){
    const m="获取二维码失败："+((r&&r.error&&r.error.message)||"未知");
    btn.disabled=false; st.textContent=m; return showToast(m);
  }
  if(!scfgRenderQr(r.data.qr, img, wrap, st)){ btn.disabled=false; return; }
  st.textContent="等待扫码…（微信扫一扫 → 手机确认）";
  _scfgWxPolling=true;
  while(_scfgWxPolling){
    await new Promise(res=>setTimeout(res,2000));
    if(!_scfgWxPolling) break; // 面板已关闭
    let p=null;
    try{ p=JSON.parse(await window.__TAURI__.core.invoke("im_wechat_login",{action:"poll"})); }
    catch(e){ continue; } // 单次网络抖动不中断，下轮再试
    if(!p||!p.ok){
      // 终态错误（二维码多次过期 / 10 分钟超时 / 服务异常）：停止轮询，可重按按钮重来。
      // 完整原因常驻状态行（toast 1.6s 不够读），方便用户回报。
      const m="登录失败："+((p&&p.error&&p.error.message)||"未知");
      _scfgWxPolling=false; st.textContent=m;
      showToast(m); break;
    }
    const s=p.data&&p.data.status;
    if(s==="scaned"){ st.textContent="已扫码：请在手机上确认"; }
    else if(s==="refreshed"){ if(p.data.qr) scfgRenderQr(p.data.qr, img, wrap, st); st.textContent="二维码已过期，已自动刷新"; }
    else if(s==="confirmed"){
      _scfgWxPolling=false;
      wrap.style.display="none"; // 登录成功：二维码使命完成，不再展示
      st.textContent="✓ 已登录";
      document.getElementById("scfg-wx-bot-id").value=(p.data.bot_id||"");
      _scfgEnabled.wechat=true; scfgRenderList(); // 左列微信自动勾选（保存时随 active 落盘）
      showToast("✓ 微信登录成功："+((p.data&&p.data.note)||"已启用"));
      break;
    } else if(s==="idle"){
      _scfgWxPolling=false; st.textContent="扫码会话已失效，请重试"; break;
    }
  }
  btn.disabled=false;
}
// ── QQ 机器人扫码登录（官方 OpenClaw 通道）：二维码内容 = q.qq.com 授权页 URL，手机 QQ
// 扫码打开并确认 → 后端拿到官方下发的 AppID/AppSecret（secret 经绑定密钥 AES-GCM 解密）
// 自动落盘 im.json 并热重载。AppID 只读输入框同步回填；无需再手动去开放平台复制凭证。 ──
let _scfgQqPolling=false; // QQ 扫码轮询进行中（关面板/终态时置停）
async function scfgQqLogin(){
  if(_scfgQqPolling){ showToast("扫码进行中，请稍候"); return; }
  const btn=document.getElementById("scfg-qq-btn"), st=document.getElementById("scfg-qq-status");
  const wrap=document.getElementById("scfg-qq-qr-wrap"), img=document.getElementById("scfg-qq-qr");
  btn.disabled=true; st.textContent="正在获取二维码…"; wrap.style.display="none";
  let r=null;
  try{ r=JSON.parse(await window.__TAURI__.core.invoke("im_qq_login",{action:"start"})); }
  catch(e){ const m="获取二维码失败："+e; btn.disabled=false; st.textContent=m; return showToast(m); }
  if(!r||!r.ok){
    const m="获取二维码失败："+((r&&r.error&&r.error.message)||"未知");
    btn.disabled=false; st.textContent=m; return showToast(m);
  }
  if(!scfgRenderQr(r.data.qr, img, wrap, st)){ btn.disabled=false; return; }
  st.textContent="等待扫码…（手机 QQ 扫一扫 → 打开页面确认）";
  _scfgQqPolling=true;
  while(_scfgQqPolling){
    await new Promise(res=>setTimeout(res,2000));
    if(!_scfgQqPolling) break; // 面板已关闭
    let p=null;
    try{ p=JSON.parse(await window.__TAURI__.core.invoke("im_qq_login",{action:"poll"})); }
    catch(e){ continue; } // 单次网络抖动不中断，下轮再试
    if(!p||!p.ok){
      // 终态（二维码过期等）：停止轮询，可重按「扫码登录」重来。原因常驻状态行。
      const m="登录失败："+((p&&p.error&&p.error.message)||"未知");
      _scfgQqPolling=false; st.textContent=m;
      showToast(m); break;
    }
    const s=p.data&&p.data.status;
    if(s==="waiting"){ /* 未扫码/未确认：继续轮 */ }
    else if(s==="confirmed"){
      _scfgQqPolling=false;
      wrap.style.display="none";
      st.textContent="✓ 凭证已获取";
      document.getElementById("scfg-qq-app-id").value=(p.data.app_id||"");
      document.getElementById("scfg-qq-secret").value="（已写入，无需填写）";
      _scfgEnabled.qq=true; scfgRenderList(); // 左列 QQ 自动勾选
      showToast("✓ QQ 登录成功："+((p.data&&p.data.note)||"已启用"));
      break;
    } else if(s==="idle"){
      _scfgQqPolling=false; st.textContent="扫码会话已失效，请重试"; break;
    }
  }
  btn.disabled=false;
}
async function imcfgSave(){
  const active=SCFG_CHANNELS.filter(ch=>_scfgEnabled[ch.id]).map(ch=>ch.id);
  const payload={
    kind:active[0]||"",
    active,
    enabled:document.getElementById("scfg-enabled").checked,
    personal:true,
    full_access:document.getElementById("scfg-full-access").checked,
    app_id:document.getElementById("scfg-app-id").value.trim(),
    base:document.getElementById("scfg-base").value,
    app_secret_action:_scfgSecretUnlocked?"set":"keep",
    qq_app_id:document.getElementById("scfg-qq-app-id").value.trim(),
    qq_secret_action:_scfgQqUnlocked?"set":"keep",
  };
  if(_scfgSecretUnlocked) payload.app_secret_value=document.getElementById("scfg-app-secret").value.trim();
  if(_scfgQqUnlocked)     payload.qq_secret_value=document.getElementById("scfg-qq-secret").value.trim();
  if(document.getElementById("scfg-enabled").checked && active.length===0){
    showToast("请至少勾选一个 IM 通道（或取消「启用遥控」）"); return;
  }
  let r=null;
  try{ r=JSON.parse(await window.__TAURI__.core.invoke("im_config",{action:"set", payload:JSON.stringify(payload)})); }
  catch(e){ showToast("保存失败："+e); return; }
  if(r&&r.ok){ showToast("IM 配置："+((r.data&&r.data.note)||"已保存")); closeSettings(); }
  else { showToast("保存失败："+((r&&r.error&&r.error.message)||"未知")); }
}

// menuAbout 已移入 js/settings.js（关于分区）。

// ── 修改密码（门户 /api/auth/change-password）：用户菜单入口 + 登录后 must_change_password 强制弹框 ──
// 门户改密成功即吊销该用户全部 token（后端 change_password 已同步本地登出）→ 前端引导重新登录。
let _pwdForced = false;     // 强制改密态：must_change_password=true 期间弹框不可关闭，唯一出口=「退出登录」（登出回登录页，或直接关 app）
// 密码策略镜像门户 PasswordPolicy：≥8 位 + 大写 + 小写 + 数字 + 特殊字符。
const PWD_SPECIAL = "!@#$%^&*()_+-=[]{}|;':\",./<>?`~";
function pwdPolicyError(p){
  if(p.length < 8) return "密码长度不能少于 8 位";
  if(!/[A-Z]/.test(p)) return "密码必须包含大写字母";
  if(!/[a-z]/.test(p)) return "密码必须包含小写字母";
  if(!/[0-9]/.test(p)) return "密码必须包含数字";
  if(![...p].some(c=>PWD_SPECIAL.includes(c))) return "密码必须包含特殊字符（如 !@#$%^&*）";
  return "";
}
function pwdShowError(msg){
  const box = document.getElementById("pwd-err");
  box.textContent = msg; box.style.display = "block";
  showToast(msg);
}
function openPwdDialog(forced){
  document.getElementById("pwd-overlay").classList.remove("hidden");
  document.getElementById("pwd-warn").style.display = forced ? "flex" : "none";
  // 强制态摘掉所有关闭入口（✕ / 稍后再说），换成「退出登录」＝登出回登录页。
  document.getElementById("pwd-x").style.display = forced ? "none" : "";
  document.getElementById("pwd-later").style.display = forced ? "none" : "";
  document.getElementById("pwd-exit").style.display = forced ? "" : "none";
  document.getElementById("pwd-err").style.display = "none";
  ["pwd-old","pwd-new","pwd-confirm"].forEach(id=>{ document.getElementById(id).value=""; });
  setTimeout(()=>document.getElementById("pwd-old").focus(), 50);
}
function userChangePassword(){ closeMenu();
  if(!CURRENT_USER){ showToast("尚未登录"); return; }
  openPwdDialog(false);
}
function pwdHide(){ document.getElementById("pwd-overlay").classList.add("hidden"); }
function pwdClose(){ if(_pwdForced) return; pwdHide(); } // 强制态无出口（按钮已隐藏，此处双保险）
async function pwdExit(){
  // 强制改密态点「退出登录」：本地登出（清会话+删 auth.json）→ 原生壳回登录窗（也可直接关 app）；Web 壳跳登录页。
  pwdHide(); _pwdForced = false;
  try{ await call({cmd:"logout"}); }catch(e){}
  CURRENT_USER = null;
  location.hash = "#/login"; showLoginView(); // SPA：切登录视图（两壳统一）
}
async function pwdSave(){
  const btn = document.querySelector("#pwd-overlay .mcfg-save");
  const errBox = document.getElementById("pwd-err");
  errBox.style.display = "none";
  const oldP = document.getElementById("pwd-old").value;
  const newP = document.getElementById("pwd-new").value;
  const conf = document.getElementById("pwd-confirm").value;
  if(!oldP || !newP) return pwdShowError("请输入旧密码和新密码");
  if(newP !== conf)  return pwdShowError("两次输入的新密码不一致");
  const policyErr = pwdPolicyError(newP);
  if(policyErr)      return pwdShowError(policyErr);
  if(newP === oldP)  return pwdShowError("新密码不能与旧密码相同");
  btn.disabled = true; btn.textContent = "提交中…";
  try{
    const r = await call({cmd:"change_password", old_password:oldP, new_password:newP});
    if(r && r.ok){
      pwdHide(); _pwdForced = false;
      CURRENT_USER = null;
      showToast("密码修改成功，请重新登录");
      // 门户已吊销全部 token：原生壳回登录窗；Web 壳跳登录页。
      location.hash = "#/login"; showLoginView(); // SPA：切登录视图（两壳统一）
    } else {
      // 门户业务错误（如策略不符 / 旧密码错误）：信封 msg 已透传到 error.message。
      pwdShowError("修改失败："+((r && r.error && r.error.message) || "未知错误"));
    }
  }catch(e){
    // invoke/fetch 层异常也不能静默——弹框内联 + toast 双通道提示，避免"无反馈卡死"。
    console.error("change_password failed:", e);
    pwdShowError("修改失败："+(e && e.message ? e.message : "请求异常"));
  }finally{
    btn.disabled = false; btn.textContent = "确认修改";
  }
}

