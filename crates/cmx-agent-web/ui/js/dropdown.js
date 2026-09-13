// ── 自绘下拉组件 ──
// 把每个 <select> 替换为 .cmx-dd（trigger + popup list），原生 select 隐藏但保留
// .value 读写 + change 事件兼容（defineProperty 拦截 value setter 同步 UI）。
"use strict";

const CMX_DD_CHEVRON = '<svg class="cmx-dd-arrow" width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 9 12 15 18 9"/></svg>';
const CMX_DD_CHECK   = '<svg class="dd-check" width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round"><polyline points="20 6 9 17 4 12"/></svg>';

function cmxInitDropdown (sel) {
  if (sel._cmxDdInit) return; // 防重复
  sel._cmxDdInit = true;

  // 包一层 wrapper
  const wrap = document.createElement("div");
  wrap.className = "cmx-dd";
  sel.parentNode.insertBefore(wrap, sel);
  wrap.appendChild(sel);
  sel.classList.add("cmx-dd-native");

  // trigger 按钮
  const trigger = document.createElement("button");
  trigger.type = "button";
  trigger.className = "cmx-dd-trigger";
  trigger.innerHTML = '<span class="cmx-dd-label"></span>' + CMX_DD_CHEVRON;

  // 弹出列表
  const list = document.createElement("div");
  list.className = "cmx-dd-list";

  function closeList () { list.classList.remove("on"); wrap.classList.remove("on"); }
  function openList () {
    // 关掉其他打开的 dropdown
    document.querySelectorAll(".cmx-dd-list.on").forEach(l => { l.classList.remove("on"); l.closest(".cmx-dd")?.classList.remove("on"); });
    buildItems();
    // 挂 body + fixed 定位（悬浮框）：absolute 列表会被弹框的 overflow:auto 容器
    // （如设置面板 .mcfg-detail）裁剪——列表只在 trigger 下方露出一条。脱离容器即根治。
    document.body.appendChild(list);
    const rect = trigger.getBoundingClientRect();
    const listH = Math.min(Array.from(sel.options).length * 38 + 8, 260); // 近似高度（item ~38px + padding）
    list.style.left = rect.left + "px";
    // 以触发框宽度起步、可被内容自然撑宽（对勾 margin-left:auto 才能贴到列表右缘，不挤在文字后）。
    list.style.minWidth = rect.width + "px";
    list.style.width = "auto";
    // 视口底部溢出检测：trigger 底部 + 弹出列表高 > viewport → 向上弹
    if (rect.bottom + listH > window.innerHeight && rect.top > listH) {
      list.style.top = (rect.top - listH - 4) + "px";
    } else {
      list.style.top = (rect.bottom + 4) + "px";
    }
    list.classList.add("on");
    wrap.classList.add("on");
  }

  function buildItems () {
    list.innerHTML = "";
    Array.from(sel.options).forEach(opt => {
      const item = document.createElement("div");
      item.className = "cmx-dd-item" + (opt.value === sel.value ? " active" : "");
      // textContent 若含 HTML 字面量（如文件名 <img onerror>），经 innerHTML 会被当标签解析——统一转义。
      item.innerHTML = "<span>" + esc(opt.textContent) + "</span>" + CMX_DD_CHECK;
      item.onclick = (e) => {
        e.stopPropagation();
        sel.value = opt.value;
        sel.dispatchEvent(new Event("change", { bubbles: true }));
        updateLabel();
        closeList();
      };
      list.appendChild(item);
    });
  }

  function updateLabel () {
    const opt = sel.options[sel.selectedIndex];
    trigger.querySelector(".cmx-dd-label").textContent = opt ? opt.textContent : "";
  }

  // trigger 点击 → 开关
  trigger.addEventListener("click", (e) => {
    e.stopPropagation();
    list.classList.contains("on") ? closeList() : openList();
  });

  // 键盘：Enter/Space 开关、↑↓ 移动 active、Enter 选中、Esc 关闭
  trigger.addEventListener("keydown", (e) => {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); list.classList.contains("on") ? closeList() : openList(); return; }
    if (!list.classList.contains("on")) return;
    const items = [...list.querySelectorAll(".cmx-dd-item")];
    let idx = items.findIndex(i => i.classList.contains("active"));
    if (e.key === "ArrowDown") { e.preventDefault(); idx = Math.min(idx + 1, items.length - 1); }
    else if (e.key === "ArrowUp") { e.preventDefault(); idx = Math.max(idx - 1, 0); }
    else if (e.key === "Escape") { closeList(); return; }
    else return;
    items.forEach(i => i.classList.remove("active"));
    items[idx]?.classList.add("active");
    items[idx]?.scrollIntoView({ block: "nearest" });
    e.preventDefault();
  });
  // 列表展开时 Enter 选中当前 active 项
  list.addEventListener("keydown", (e) => {
    if (e.key !== "Enter") return;
    const active = list.querySelector(".cmx-dd-item.active");
    if (active) { active.click(); }
  });
  // trigger 可聚焦
  trigger.tabIndex = 0;

  // 点外面关闭（由全局关闭器统一处理，不挂 per-instance listener）
  wrap._cmxClose = closeList;

  // 拦截 select.value setter → 同步 trigger 文本
  const desc = Object.getOwnPropertyDescriptor(HTMLSelectElement.prototype, "value");
  Object.defineProperty(sel, "value", {
    get () { return desc.get.call(sel); },
    set (v) { desc.set.call(sel, v); updateLabel(); }
  });

  // change 事件也同步（防御）
  sel.addEventListener("change", updateLabel);

  // 初始渲染
  updateLabel();
  wrap.appendChild(trigger);
  wrap.appendChild(list);
}

function cmxInitAllDropdowns () {
  document.querySelectorAll("select").forEach(cmxInitDropdown);
}

// 脚本在 body 底部，DOM 已解析完 → 立即初始化
cmxInitAllDropdowns();
// ── 全局下拉关闭器（单监听器，替代 per-instance document listeners）──
document.addEventListener("click", (e) => {
  document.querySelectorAll(".cmx-dd.on").forEach(wrap => {
    if (!wrap.contains(e.target) && wrap._cmxClose) wrap._cmxClose();
  });
});
// 悬浮列表挂 body + fixed 定位：容器滚动 / 窗口缩放会让坐标失准 → 统一关闭（capture 捕获容器内滚动）。
window.addEventListener("scroll", () => {
  document.querySelectorAll(".cmx-dd.on").forEach(w => w._cmxClose && w._cmxClose());
}, true);
window.addEventListener("resize", () => {
  document.querySelectorAll(".cmx-dd.on").forEach(w => w._cmxClose && w._cmxClose());
});
