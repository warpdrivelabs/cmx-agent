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
    list.classList.add("on");
    wrap.classList.add("on");
  }

  function buildItems () {
    list.innerHTML = "";
    Array.from(sel.options).forEach(opt => {
      const item = document.createElement("div");
      item.className = "cmx-dd-item" + (opt.value === sel.value ? " active" : "");
      item.innerHTML = "<span>" + opt.textContent + "</span>" + CMX_DD_CHECK;
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