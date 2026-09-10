/* ============================================================
   智方 CloudMatrix - 页面交互
   ============================================================ */
(function () {
  'use strict';

  // ---------- 工具函数 ----------
  const $ = (sel, root = document) => root.querySelector(sel);
  const $$ = (sel, root = document) => Array.from(root.querySelectorAll(sel));
  const on = (el, evt, handler, opts) => el && el.addEventListener(evt, handler, opts);

  // ---------- Toast 提示 ----------
  const toastEl = $('#toast');
  let toastTimer;
  function showToast(msg) {
    if (!toastEl) return;
    toastEl.textContent = msg;
    toastEl.classList.add('is-show');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toastEl.classList.remove('is-show'), 2200);
  }

  // ---------- Lucide 图标渲染 ----------
  // lucide 1.x 起 createIcons 为破坏性 API：必须显式传 { icons }，无参调用直接抛错
  // （图标全部保持空白）。动态插入 <i data-lucide> 后统一走这里重渲。
  function refreshIcons() {
    if (window.lucide && window.lucide.icons) {
      window.lucide.createIcons({ icons: window.lucide.icons });
    }
  }

  // ---------- Header 滚动效果 ----------
  const header = $('#site-header');
  const onScrollHeader = () => {
    if (!header) return;
    if (window.scrollY > 8) header.classList.add('is-scrolled');
    else header.classList.remove('is-scrolled');
  };
  on(window, 'scroll', onScrollHeader, { passive: true });
  onScrollHeader();

  // ---------- 移动端菜单 ----------
  const menuToggle = $('#menu-toggle');
  const mobileMenu = $('#mobile-menu');
  function closeMobileMenu() {
    if (!mobileMenu || !menuToggle) return;
    mobileMenu.classList.remove('is-open');
    menuToggle.setAttribute('aria-expanded', 'false');
    const icon = menuToggle.querySelector('[data-lucide], svg');
    if (icon && window.lucide) {
      menuToggle.innerHTML = '';
      const span = document.createElement('i');
      span.setAttribute('data-lucide', 'menu');
      menuToggle.appendChild(span);
      refreshIcons();
    }
  }
  function openMobileMenu() {
    if (!mobileMenu || !menuToggle) return;
    mobileMenu.classList.add('is-open');
    menuToggle.setAttribute('aria-expanded', 'true');
    if (window.lucide) {
      // 延迟到本事件冒泡结束后再替换图标：立即清空 innerHTML 会让本事件
      // 的 e.target（旧 SVG）脱离 DOM，冒泡到 document 的「点击外部关闭」
      // 判断 contains(e.target) 失效——菜单会被刚开即关。
      setTimeout(() => {
        menuToggle.innerHTML = '';
        const span = document.createElement('i');
        span.setAttribute('data-lucide', 'x');
        menuToggle.appendChild(span);
        refreshIcons();
      }, 0);
    }
  }
  on(menuToggle, 'click', () => {
    if (mobileMenu.classList.contains('is-open')) closeMobileMenu();
    else openMobileMenu();
  });
  // 点击菜单项后关闭
  $$('a', mobileMenu).forEach(a => on(a, 'click', closeMobileMenu));
  // 点击外部关闭
  on(document, 'click', (e) => {
    if (!mobileMenu.classList.contains('is-open')) return;
    if (mobileMenu.contains(e.target) || menuToggle.contains(e.target)) return;
    closeMobileMenu();
  });
  // ESC 关闭
  on(document, 'keydown', (e) => {
    if (e.key === 'Escape') closeMobileMenu();
  });

  // ---------- 平滑滚动 + 滚动高亮 ----------
  const navLinks = $$('a.nav-link[href^="#"]');
  const sections = navLinks
    .map(a => $(a.getAttribute('href')))
    .filter(Boolean);

  function setActiveNav(id) {
    navLinks.forEach(a => {
      const target = a.getAttribute('href');
      if (target === `#${id}`) a.classList.add('is-active');
      else a.classList.remove('is-active');
    });
  }

  // 滚动监听
  const spyObserver = new IntersectionObserver((entries) => {
    entries.forEach(entry => {
      if (entry.isIntersecting && entry.target.id) {
        setActiveNav(entry.target.id);
      }
    });
  }, { rootMargin: '-40% 0px -55% 0px', threshold: 0 });
  sections.forEach(s => spyObserver.observe(s));

  // ---------- OS 检测 ----------
  function detectOS() {
    const ua = navigator.userAgent;
    const platform = (navigator.platform || '').toLowerCase();
    if (/mac/i.test(platform) || /macintosh|iphone|ipad|ipod/i.test(ua)) return 'macos';
    if (/win/i.test(platform) || /windows/i.test(ua)) return 'windows';
    if (/linux/i.test(platform) || /linux|android/i.test(ua)) return 'linux';
    return null;
  }
  const userOS = detectOS();
  const osLabels = { linux: 'Linux', windows: 'Windows', macos: 'macOS' };

  // 高亮推荐 OS（徽章已移除，仅保留检测逻辑用于 toast 文案）
  const osTiles = $$('.os-tile');

  // 更新下载按钮文案（按钮已移除，保留检测逻辑备用）
  // const ctaText = $('#download-cta-text');
  // if (ctaText) { ... }

  // ---------- 更新源（智能体自动更新边缘通道） ----------
  // PORTAL_BASE：门户绝对地址（第一优先）；失败自动回落页面同源（适合 nginx 把
  // /agent-updates/ 反代到门户的部署）。换门户地址只改这里。
  const PORTAL_BASE = 'http://192.168.137.111:8080';
  const trimBase = (b) => String(b || '').replace(/\/+$/, '');

  // 产物键（tauri updater 命名 {os}-{arch}-{installer}）→ 展示文案
  const FMT_TEXT = {
    nsis: '.exe 安装程序',
    msi: '.msi 安装包',
    dmg: '.dmg 磁盘映像',
    app: '.app.tar.gz 应用包',
    deb: '.deb 安装包',
    appimage: '.AppImage',
    rpm: '.rpm 安装包',
  };
  // OS 卡片 → 产物键前缀（macOS 的产物键是 darwin-*）
  const OS_PREFIX = { linux: 'linux-', windows: 'windows-', macos: 'darwin-' };

  let activeBase = trimBase(PORTAL_BASE);

  function fmtTextOf(key) {
    const fmt = key.split('-').slice(2).join('-');
    return FMT_TEXT[fmt] || (fmt ? '.' + fmt : '安装包');
  }

  // 下载地址：优先 latest.json 下发的（门户转发绝对地址），否则按生效源拼转发地址
  function downloadUrlOf(key, entry, version) {
    const u = (entry && entry.url || '').trim();
    if (/^https?:\/\//i.test(u)) return u;
    return activeBase + '/agent-updates/download?version=' + encodeURIComponent(version) +
      '&platform=' + encodeURIComponent(key);
  }

  function setMeta(text) {
    const meta = $('#latest-meta');
    if (meta) meta.textContent = text;
  }

  // 按 latest.json 的 platforms 填充 OS 卡片（无产物的平台置灰）
  function applyArtifacts(data) {
    const platforms = (data && data.platforms) || {};
    const version = (data && data.version) || '';
    const date = /(\d{4})-(\d{2})-(\d{2})/.exec(String((data && data.pub_date) || ''));

    // 产物键的格式段（tauri 键 {os}-{arch}-{fmt} 的第三段），用于「同格式多架构」计数
    const fmtOf = (k) => k.split('-').slice(2).join('-');

    osTiles.forEach(tile => {
      const prefix = OS_PREFIX[tile.dataset.os];
      let keys = Object.keys(platforms).filter(k =>
        k.startsWith(prefix) && ((platforms[k].url || '').trim() || (platforms[k].file_id || '').trim()));
      // macOS 只发 .dmg（首装分发格式）：.app.tar.gz 是 tauri updater 产物，不下发给用户。
      // 未登记 dmg 键 → 卡片置灰提示「暂未提供」，绝不回退 tar.gz。
      if (tile.dataset.os === 'macos') {
        keys = keys.filter(k => /-dmg$/.test(k));
      }
      if (!keys.length) {
        tile.classList.add('is-unavailable');
        tile.querySelector('.os-size').textContent = '暂未提供';
        return;
      }
      const key = keys[0];
      tile.dataset.url = downloadUrlOf(key, platforms[key], version);
      const sameFmt = keys.filter(k => fmtOf(k) === fmtOf(key));
      tile.querySelector('.os-size').textContent =
        fmtTextOf(key) + (sameFmt.length > 1 ? ' · ' + sameFmt.length + ' 架构' : '');
    });

    setMeta(version
      ? ' · 最新 v' + version + (date ? '（' + date[1].slice(2) + '-' + date[2] + '-' + date[3] + '）' : '')
      : '');
  }

  function markUnavailable(msg) {
    osTiles.forEach(tile => {
      tile.classList.add('is-unavailable');
      tile.querySelector('.os-size').textContent = '暂未提供';
    });
    setMeta(msg ? ' · ' + msg : '');
  }

  // 多源探测：门户绝对地址（跨域，需门户为 /agent-updates/* 开 CORS）→ 页面同源。
  // 204 是更新源权威应答（无已发布版本），不再换源；其余失败（网络 / CORS /
  // 非 ok / JSON 解析失败）自动换下一个源重试。
  function loadLatest() {
    const candidates = [];
    const primary = trimBase(PORTAL_BASE);
    if (primary) candidates.push(primary);
    candidates.push(''); // 同源兜底
    let idx = 0;
    (function attempt() {
      if (idx >= candidates.length) {
        markUnavailable('版本信息获取失败');
        return;
      }
      const base = trimBase(candidates[idx++]);
      fetch(base + '/agent-updates/latest.json', { cache: 'no-store' })
        .then(res => {
          if (res.status === 204) return null;
          if (!res.ok) throw new Error('HTTP ' + res.status);
          return res.json();
        })
        .then(data => {
          if (!data || !data.version || !data.platforms) {
            markUnavailable('更新源暂无已发布版本');
            return;
          }
          activeBase = base;
          applyArtifacts(data);
        })
        .catch(attempt);
    })();
  }
  loadLatest();

  // ---------- OS 卡片点击 -> 下载 ----------
  osTiles.forEach(tile => {
    on(tile, 'click', () => {
      const url = tile.dataset.url;
      const osName = tile.dataset.os;
      if (!url) {
        showToast('该平台安装包暂未提供，请稍后再试');
        return;
      }
      showToast(`即将开始下载 ${osLabels[osName] || osName} 版本`);
      window.open(url, '_blank');
    });
  });

  // ---------- FAQ 折叠 ----------
  const faqItems = $$('.faq-item');
  function setFaqOpen(item, open) {
    const answer = item.querySelector('.faq-answer');
    const btn = item.querySelector('.faq-question');
    if (!answer || !btn) return;
    if (open) {
      item.classList.add('is-open');
      btn.setAttribute('aria-expanded', 'true');
      answer.style.maxHeight = answer.scrollHeight + 'px';
    } else {
      item.classList.remove('is-open');
      btn.setAttribute('aria-expanded', 'false');
      answer.style.maxHeight = '0';
    }
  }
  faqItems.forEach(item => {
    const btn = item.querySelector('.faq-question');
    if (!btn) return;
    on(btn, 'click', () => {
      const isOpen = item.classList.contains('is-open');
      // 手风琴效果：关闭其他
      faqItems.forEach(other => {
        if (other !== item) setFaqOpen(other, false);
      });
      setFaqOpen(item, !isOpen);
    });
  });
  // 初始化：默认展开第一个
  faqItems.forEach((item, idx) => setFaqOpen(item, idx === 0));

  // 窗口尺寸变化时重新计算已展开 FAQ 的高度
  on(window, 'resize', () => {
    faqItems.forEach(item => {
      if (item.classList.contains('is-open')) {
        const answer = item.querySelector('.faq-answer');
        if (answer) answer.style.maxHeight = answer.scrollHeight + 'px';
      }
    });
  });

  // ---------- 滚动入场动画 ----------
  const animateEls = $$('[data-animate]');
  if ('IntersectionObserver' in window) {
    const animateObserver = new IntersectionObserver((entries) => {
      entries.forEach(entry => {
        if (entry.isIntersecting) {
          entry.target.classList.add('is-visible');
          animateObserver.unobserve(entry.target);
        }
      });
    }, { rootMargin: '0px 0px -10% 0px', threshold: 0.1 });
    animateEls.forEach(el => animateObserver.observe(el));
  } else {
    animateEls.forEach(el => el.classList.add('is-visible'));
  }

  // ---------- 减少动画偏好 ----------
  if (window.matchMedia('(prefers-reduced-motion: reduce)').matches) {
    animateEls.forEach(el => el.classList.add('is-visible'));
  }

  // ---------- 控制台彩蛋 ----------
  if (window.console && console.log) {
    const style = 'font-size:14px;font-weight:bold;color:#2563eb;background:#dbeafe;padding:4px 10px;border-radius:6px;';
    console.log('%c普联软件·智方（CloudMatrix）', style);
    console.log('%cTrueMate · Work true. Mate true. 专注工作，真心搭档。', 'color:#6b7280;font-size:12px;');
  }
})();
