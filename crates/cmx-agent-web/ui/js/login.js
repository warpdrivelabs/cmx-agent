// ── SPA 登录视图 + hash 路由（同核多壳）──
// #/login → 显示登录视图；#/ 或空 → 显示主视图。
// 页面加载时检查登录态（whoami），未认证 → 自动切到 #/login。

// ── 视图切换 ──
function showLoginView () {
  document.getElementById('main-view').style.display = 'none';
  document.getElementById('login-view').style.display = 'block';
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
  try {
    // 调 whoami 判断是否已认证（bridge.js 的 call() 双桥兼容）
    const r = await call({ cmd: 'current_user' });
    if (r && r.ok && r.data && r.data.user) {
      location.hash = '#/';
      showMainView();
      refreshUser();
      refreshTasks();
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

// ── 更新检查（仅 Tauri 壳；启动后在登录视图首查一次，5s 后）──
// 退出登录回登录视图不重查不弹；关闭应用重开是新进程，会重新首查。
(async () => {
  if (!(window.__TAURI__ && window.__TAURI__.core)) return; // Web 壳无更新通道
  const snoozed = (v) => {
    try {
      const s = JSON.parse(localStorage.getItem('cmx-update-snooze') || 'null');
      return !!(s && s.version === v && Date.now() - s.ts < 30 * 60 * 1000);
    } catch (e) { return false; }
  };
  const showUpd = (info) => {
    let mask = document.getElementById('cmx-upd-mask'); if (mask) mask.remove();
    try { localStorage.setItem('cmx-update-snooze', JSON.stringify({ version: info.version, ts: Date.now() })); } catch (e) {}
    const esc = (s) => String(s ?? '').replace(/[&<>"']/g, c => ({'&':'&amp;','<':'&lt;','>':'&gt;','"':'&quot;',"'":'&#39;'}[c]));
    mask = document.createElement('div');
    mask.id = 'cmx-upd-mask'; mask.className = 'cmx-upd-mask';
    mask.innerHTML = '<div class="cmx-upd" role="dialog" aria-modal="true">'
      + '<div class="cmx-upd-title"><span class="dot"></span>发现新版本 ' + esc(info.version) + '</div>'
      + '<div class="cmx-upd-body">' + (esc(info.notes) || '暂无更新说明') + '</div>'
      + '<div class="cmx-upd-ops">'
      + (info.force ? '' : '<button class="cmx-upd-btn" data-x="later">稍后</button>')
      + '<button class="cmx-upd-btn primary" data-x="now">立即更新</button>'
      + '</div></div>';
    document.body.appendChild(mask);
    requestAnimationFrame(() => mask.classList.add('on'));
    const close = () => { mask.classList.remove('on'); setTimeout(() => mask.remove(), 200); };
    mask.addEventListener('click', async (e) => {
      const act = e.target.closest('[data-x]');
      if (act && act.dataset.x === 'now') {
        const btn = mask.querySelector('[data-x="now"]');
        btn.disabled = true; btn.textContent = '下载中…';
        if (window.__TAURI__.event) {
          try { window.__TAURI__.event.listen('update_progress', (ev) => {
            const p = ev.payload || {};
            btn.textContent = p.total ? '下载 ' + Math.round(p.received / p.total * 100) + '%'
                                      : '下载 ' + Math.round(p.received / 1024) + 'KB';
          }); } catch (err) {}
        }
        try {
          await window.__TAURI__.core.invoke('download_and_install');
          btn.textContent = '即将重启…';
        } catch (err) {
          btn.disabled = false; btn.textContent = '更新失败，点重试';
        }
      } else if (act && act.dataset.x === 'later') {
        close();
      } else if (!info.force && e.target === mask) { close(); }
    });
  };
  const runUpdateCheck = async () => {
    try { localStorage.removeItem('cmx-update-snooze'); } catch (e) {}
    try {
      const r = JSON.parse(await window.__TAURI__.core.invoke('check_update'));
      if (r && r.ok && r.data && r.data.update_available) {
        const info = { version: r.data.version, notes: r.data.notes, force: r.data.force };
        if (!info.force && snoozed(info.version)) return;
        showUpd(info);
      }
    } catch (e) { console.warn('[update] 首查失败（静默）:', e); }
  };
  setTimeout(runUpdateCheck, 500);
})();

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
    refreshUser();   // 刷新用户头像/名称
    refreshTasks();  // 刷新侧栏会话列表
  } catch (err) {
    errorEl.textContent = (typeof err === 'string' ? err : (err && err.message)) || '登录失败，请重试';
  } finally {
    submitBtn.disabled = false; submitBtn.textContent = old;
  }
});

// ── 脚本在 body 底部，DOM 已解析完 → 立即路由，不闪首页 ──
routeByHash();