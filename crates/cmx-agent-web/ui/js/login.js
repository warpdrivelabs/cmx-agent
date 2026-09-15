// ── SPA 登录视图 + hash 路由（同核多壳）──
// #/login → 显示登录视图；#/ 或空 → 显示主视图。
// 页面加载时检查登录态（whoami），未认证 → 自动切到 #/login。

// ── 视图切换 ──
function showLoginView () {
  document.getElementById('main-view').style.display = 'none';
  document.getElementById('login-view').style.display = 'block';
  // 登出/换号清场：复位到登录表单（注册成功进主界面后登出，不能停在注册表单）、清输入与错误提示。
  // 用 DOM 直查而非闭包常量——初始化路由可能早于文件尾部常量初始化执行。
  const loginFormEl = document.getElementById('login-form');
  const regFormEl = document.getElementById('register-form');
  if (regFormEl && regFormEl.style.display !== 'none') showRegisterForm(false);
  if (loginFormEl) loginFormEl.reset();
  if (regFormEl) regFormEl.reset();
  const loginErr = document.getElementById('error');
  if (loginErr) loginErr.textContent = '';
  const regErr = document.getElementById('register-error');
  if (regErr) regErr.textContent = '';
  // 首次显示时 seed 星空（只在还没有子元素时执行一次）
  const starsFar = document.getElementById('space-stars-far');
  if (starsFar && !starsFar.childElementCount) seedLoginSpace();
}
function showMainView () {
  document.getElementById('login-view').style.display = 'none';
  document.getElementById('main-view').style.display = 'flex';
  // 主视图显示后聚焦输入框
  const inp = document.getElementById('inp');
  if (inp) inp.focus();
}
function routeByHash () {
  if (location.hash === '#/login') { showLoginView(); return; }
  // 非登录 hash → 检查登录态再路由
  checkAuthAndRoute();
}
async function checkAuthAndRoute () {
  // Tauri 壳：先等后台会话回放结束（成功/失败都放行），再查登录态——避免已登录用户
  // 启动时回放未完成、current_user 暂空而闪一下登录页。断网时回放约 5s（connect_timeout）
  // 结束；8s JS 兜底防命令悬挂，旧包无此命令（reject）也直接放行。
  if (window.__TAURI__ && window.__TAURI__.core) {
    try {
      await Promise.race([
        window.__TAURI__.core.invoke('session_restore_done'),
        new Promise(res => setTimeout(res, 8000)),
      ]);
    } catch (e) { /* 旧包无此命令：直接路由 */ }
  }
  try {
    // 调 whoami 判断是否已认证（bridge.js 的 call() 双桥兼容）
    const r = await call({ cmd: 'current_user' });
    if (r && r.ok && r.data && r.data.user) {
      location.hash = '#/';
      showMainView();
      if (typeof refreshUser === "function") refreshUser();
      if (typeof refreshTasks === "function") refreshTasks();
      if (typeof refreshModelLabel === "function") refreshModelLabel();
      return;
    }
  } catch (e) { /* 未认证或桥不通 */ }
  location.hash = '#/login';
  showLoginView();
}
window.addEventListener('hashchange', () => {
  if (location.hash === '#/login') showLoginView();
  // 其他 hash 不做主动切换（由 checkAuthAndRoute / 登录成功回调负责）
});

// ── 空间主题装饰（纯前端动画，seed 随机星点/流星；首次显示登录视图时执行）──
document.getElementById('year').textContent = String(new Date().getFullYear());
function seedSpaceStars (host, count, sizeMin, sizeMax) {
  for (let i = 0; i < count; i++) {
    const el = document.createElement('span'); el.className = 'space-star';
    const size = sizeMin + Math.random() * (sizeMax - sizeMin);
    el.style.width = size + 'px'; el.style.height = size + 'px';
    el.style.left = (Math.random() * 100) + '%'; el.style.top = (Math.random() * 100) + '%';
    el.style.setProperty('--star-o-min', (0.12 + Math.random() * 0.28).toFixed(2));
    el.style.setProperty('--star-o-max', (0.45 + Math.random() * 0.55).toFixed(2));
    el.style.animationDuration = (2.2 + Math.random() * 4.5) + 's';
    el.style.animationDelay = (-Math.random() * 6) + 's';
    host.appendChild(el);
  }
}
function seedLoginSpace () {
  seedSpaceStars(document.getElementById('space-stars-far'), 52, 1, 1.6);
  seedSpaceStars(document.getElementById('space-stars-mid'), 28, 1.6, 2.8);
  const rushHost = document.getElementById('space-rush');
  for (let i = 0; i < 58; i++) {
    const el = document.createElement('span'); el.className = 'space-rush-star';
    const angle = Math.random() * Math.PI * 2; const dist = 32 + Math.random() * 58;
    el.style.left = (75 + Math.random() * 8 - 4) + '%'; el.style.top = (45 + Math.random() * 8 - 4) + '%';
    el.style.setProperty('--dx', (Math.cos(angle) * dist) + 'vmax');
    el.style.setProperty('--dy', (Math.sin(angle) * dist) + 'vmax');
    el.style.setProperty('--star-size', (1 + Math.random() * 2.2).toFixed(1) + 'px');
    el.style.setProperty('--star-scale', (1.4 + Math.random() * 2.8).toFixed(2));
    el.style.animationDuration = (1.6 + Math.random() * 2.6) + 's';
    el.style.animationDelay = (-Math.random() * 4.5) + 's';
    rushHost.appendChild(el);
  }
  const shooterHost = document.getElementById('space-shooters');
  for (let i = 0; i < 7; i++) {
    const el = document.createElement('span'); el.className = 'space-shooter';
    el.style.setProperty('--shoot-angle', (-18 + Math.random() * 36) + 'deg');
    el.style.top = (8 + Math.random() * 72) + '%'; el.style.left = (-12 + Math.random() * 18) + '%';
    el.style.animationDuration = (3.8 + Math.random() * 5.5) + 's';
    el.style.animationDelay = (-Math.random() * 12) + 's';
    shooterHost.appendChild(el);
  }
}

// ── 更新检查（仅 Tauri 壳；启动后 500ms 首查）──
// 复用 update.js 的 checkUpdate(true)（静默模式：失败无感，有新版弹自绘窗+标题栏按钮）。
// 退出登录回登录视图不重查不弹；关闭应用重开是新进程，会重新首查。
setTimeout(() => { if (typeof checkUpdate === "function") checkUpdate(true); }, 500);

// ── 登录逻辑（同核多壳）：原生 Tauri 壳走 invoke("login")（SPA 下返回成功即切视图）；Web 壳回退 POST /api。──
const form = document.getElementById('login-form');
const errorEl = document.getElementById('error');
const submitBtn = document.getElementById('submit');
async function callLogin (username, password) {
  if (window.__TAURI__ && window.__TAURI__.core) {
    const raw = await window.__TAURI__.core.invoke('login', { username, password });
    return JSON.parse(raw);
  }
  const r = await fetch('/api', { method: 'POST', headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ cmd: 'login', username, password }) });
  const j = await r.json();
  if (!j.ok) throw new Error((j.error && j.error.message) || '登录失败');
  return j.data;
}
form.addEventListener('submit', async (e) => {
  e.preventDefault();
  errorEl.textContent = '';
  const old = submitBtn.textContent;
  submitBtn.disabled = true; submitBtn.textContent = '登录中…';
  try {
    const username = form.username.value.trim();
    const password = form.password.value;
    if (!username || !password) { errorEl.textContent = '请输入用户名和密码'; return; }
    await callLogin(username, password);
    // 登录成功 → SPA 切视图（不再跳页面）
    location.hash = '#/';
    showMainView();
    if (typeof refreshUser === "function") refreshUser();   // main.js 加载后可用
    if (typeof refreshTasks === "function") refreshTasks(); // main.js 加载后可用
    if (typeof refreshModelLabel === "function") refreshModelLabel(); // 模型标签（首次调用可能竞态失败）
  } catch (err) {
    errorEl.textContent = (typeof err === 'string' ? err : (err && err.message)) || '登录失败，请重试';
  } finally {
    submitBtn.disabled = false; submitBtn.textContent = old;
  }
});

// ── 脚本在 body 底部，DOM 已解析完。启动路由挪到文件最末尾（注册表单常量初始化之后），
// 避免 showLoginView 复位逻辑触碰未初始化的绑定。──

// ── 自助注册（同核多壳）：登录/注册表单互斥切换；注册走前门 register 命令（免登录白名单），
// 成功即登录（门户直发 token 对，后端落 auth.json），与登录成功完全同路径进主界面。──
const regForm = document.getElementById('register-form');
const regError = document.getElementById('register-error');
const regSubmit = document.getElementById('register-submit');
const gotoRegister = document.getElementById('goto-register');
const gotoLogin = document.getElementById('goto-login');
const formTitle = document.getElementById('form-title');
const formSubtitle = document.getElementById('form-subtitle');
// 与门户 /api/auth/register 同规的用户名格式（后端仍为最终裁决）
const REG_USERNAME_RE = /^[A-Za-z0-9_@.\-]{2,100}$/;

function showRegisterForm (show) {
  form.style.display = show ? 'none' : '';
  regForm.style.display = show ? '' : 'none';
  gotoRegister.style.display = show ? 'none' : '';
  gotoLogin.style.display = show ? '' : 'none';
  formTitle.textContent = show ? '创建账号' : '欢迎回来';
  formSubtitle.textContent = show ? '注册 TrueMate，开启你的工作' : '登录 TrueMate，开启你的工作';
  (show ? regForm : form).querySelector('input').focus();
}
gotoRegister.addEventListener('click', () => showRegisterForm(true));
gotoLogin.addEventListener('click', () => showRegisterForm(false));

// 注册入口显隐：后端 ui_config（免登录白名单）决定；查询失败保持默认展示（注册成败最终由门户裁决）。
(async () => {
  try {
    const r = await call({ cmd: 'ui_config' });
    if (r && r.ok && r.data && r.data.register_enabled === false) {
      gotoRegister.style.display = 'none';
    }
  } catch (e) { /* 查询失败保持默认 */ }
})();

regForm.addEventListener('submit', async (e) => {
  e.preventDefault();
  regError.textContent = '';
  const username = regForm.username.value.trim();
  const nickname = regForm.nickname.value.trim();
  const password = regForm.password.value;
  const password2 = regForm.password2.value;
  if (!username || !password) { regError.textContent = '请输入用户名和密码'; return; }
  if (!REG_USERNAME_RE.test(username)) { regError.textContent = '用户名须为 2-100 位字母、数字或 _ @ . - 组成'; return; }
  if (password !== password2) { regError.textContent = '两次输入的密码不一致'; return; }
  // 复杂度提示（与改密同规；门户服务端为最终裁决）
  if (password.length < 8 || !/[A-Z]/.test(password) || !/[a-z]/.test(password)
    || !/[0-9]/.test(password) || !/[!@#$%^&*()_+\-=[\]{}|;':",./<>?`~]/.test(password)) {
    regError.textContent = '密码须 8 位以上，且包含大写字母、小写字母、数字和特殊字符';
    return;
  }
  const old = regSubmit.textContent;
  regSubmit.disabled = true; regSubmit.textContent = '注册中…';
  try {
    const r = await call({ cmd: 'register', username, password, nickname: nickname || undefined });
    if (!r || r.ok === false) {
      throw new Error((r && r.error && r.error.message) || '注册未开放或失败，请重试');
    }
    // 注册即登录 → SPA 切主视图（同登录成功回调）
    location.hash = '#/';
    showMainView();
    if (typeof refreshUser === "function") refreshUser();
    if (typeof refreshTasks === "function") refreshTasks();
    if (typeof refreshModelLabel === "function") refreshModelLabel();
  } catch (err) {
    regError.textContent = (typeof err === 'string' ? err : (err && err.message)) || '注册失败，请重试';
  } finally {
    regSubmit.disabled = false; regSubmit.textContent = old;
  }
});

// ── 启动路由（放文件最末尾：注册表单相关常量均已初始化，showLoginView 复位逻辑可安全执行）──
routeByHash();
