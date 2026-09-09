import { LitElement, html, css, nothing } from "lit";
import { customElement, query, state } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Button.js";
import { StoreController } from "../../platform/store-controller";
import { sessionStore } from "../../platform/stores/session-store";
import { streamStore } from "../../platform/stores/stream-store";
import { settingsStore } from "../../platform/stores/settings-store";
import { themeStore } from "../../platform/stores/theme-store";
import { authStore } from "../../platform/stores/auth-store";
import { navigate, onRoute, type Route } from "../../router";
import { toast } from "../common/cmx-agent-toast";
import type { Policy } from "../../protocol/policy";
import type { SessionMeta } from "../../protocol/events-window";
import type { PluginInfo } from "../../protocol/response";
import "./cmx-agent-session-list";
import "./cmx-agent-message-list";
import "./cmx-agent-composer";
import "./cmx-agent-im-panel";
import "./cmx-agent-plugins-side-panel";
import "./cmx-agent-connectors-view";
import "./cmx-agent-plugins-view";
import "./cmx-agent-model-menu";
import "./cmx-agent-model-config-dialog";
import "../common/cmx-agent-error-note";

type OpenTabKind = "session" | "connectors" | "plugins";

interface OpenTab {
  id: string;
  kind: OpenTabKind;
  title: string;
}

/** 快捷动作（对齐旧 QUICK：点击=预填输入框，不直接发送）。 */
const QUICK = ["📄 文档处理", "💳 金融服务", "📊 数据分析及可视化", "🧰 个人工作台", "🔍 深度研究"];
const APP_VERSION = "M2 · 0.1.0";

interface SpeechRecognitionLike {
  stop(): void;
}

/**
 * 聊天工作台（容器）：1:1 对齐旧 index.html——
 * 活动栏（badge/用户菜单/cmx 菜单）/ 侧栏三面板（Agent/IM/插件）/ splitter 拖宽 /
 * 多 tab（溢出下拉 + 右键菜单 + 连接器/插件详情 tab）/ home（QUICK 预填）/ 弹层体系。
 */
@customElement("cmx-agent-chat-view")
export class CmxAgentChatView extends LitElement {
  private sessions = new StoreController(this, sessionStore);
  private stream = new StoreController(this, streamStore);
  private settings = new StoreController(this, settingsStore);
  private theme = new StoreController(this, themeStore);
  private auth = new StoreController(this, authStore);
  private offRoute?: () => void;
  private offSplitter?: () => void;
  private tabKeyDown: (e: KeyboardEvent) => void = () => undefined;
  private docClick: (e: MouseEvent) => void = () => undefined;
  @state() private openTabs: OpenTab[] = [];
  @state() private sidePanel: "agent" | "im" | "plugins" = "agent";
  @state() private sideCollapsed = false;
  private offSidebarToggle?: () => void;
  @state() private connBadge = "";
  @state() private openMenu: "" | "cmx" | "user" | "tabdd" | "tabctx" = "";
  @state() private tabCtx = { x: 0, y: 0, tabId: "" };
  @state() private pluginDetail: { plugin: PluginInfo; installed: boolean } | null = null;
  /** 当前激活 tab（null=home 空态，对齐旧 ACTIVE）。 */
  @state() private activeTab: "session" | "connectors" | "plugins" | null = null;
  @query("cmx-agent-model-config-dialog")
  private modelCfgDialog?: HTMLElement & { openDialog: () => Promise<void> };
  @query("#home-composer") private homeComposer?: HTMLElement & { setText: (v: string) => void };

  static styles = css`
    :host {
      display: flex;
      height: 100%;
      min-height: 0;
      background: var(--bg);
      color: var(--ink);
      font-size: 13px;
    }
    .activitybar {
      width: 52px;
      flex: 0 0 52px;
      background: var(--panel);
      border-right: 1px solid var(--border);
      display: flex;
      flex-direction: column;
      align-items: center;
      padding-top: 6px;
      gap: 2px;
    }
    .act {
      width: 44px;
      height: 44px;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 20px;
      border-radius: 8px;
      cursor: pointer;
      color: var(--ink2);
      position: relative;
    }
    .act:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .act.active {
      color: var(--aqua);
    }
    .act.active::before {
      content: "";
      position: absolute;
      left: -4px;
      top: 8px;
      bottom: 8px;
      width: 3px;
      border-radius: 2px;
      background: var(--aqua);
    }
    .act-spacer {
      flex: 1;
    }
    .act .badge {
      position: absolute;
      top: 5px;
      right: 5px;
      min-width: 15px;
      height: 15px;
      padding: 0 3px;
      border-radius: 8px;
      background: var(--red);
      color: var(--ws-white);
      font-size: 9px;
      font-weight: 700;
      display: flex;
      align-items: center;
      justify-content: center;
    }
    .user-act {
      padding: 0;
    }
    .user-act .uic {
      width: 30px;
      height: 30px;
      border-radius: 50%;
      background: linear-gradient(135deg, var(--aqua), var(--green));
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 16px;
    }
    .cmx-act {
      padding: 0;
      overflow: hidden;
      border: none;
      background: transparent;
      cursor: pointer;
    }
    .cmx-act img {
      width: 30px;
      height: 30px;
      border-radius: 8px;
      object-fit: cover;
      display: block;
    }
    .cmx-act:hover {
      background: var(--hover);
    }
    /* 弹层菜单（cmxmenu / usermenu / tab-dropdown / tabctx） */
    .cmxmenu {
      position: fixed;
      left: 56px;
      bottom: 10px;
      min-width: 180px;
      background: var(--panel);
      border: 1px solid var(--border2);
      border-radius: 10px;
      padding: 5px;
      box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
      z-index: 80;
    }
    .usermenu {
      min-width: 220px;
      bottom: 52px;
    }
    .mi {
      display: flex;
      align-items: center;
      gap: 10px;
      padding: 9px 11px;
      border-radius: 7px;
      font-size: 13.5px;
      color: var(--ink2);
      cursor: pointer;
    }
    .mi:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .mi-i {
      width: 16px;
      text-align: center;
      font-size: 14px;
    }
    .mi-sep {
      height: 1px;
      background: var(--border);
      margin: 5px 8px;
    }
    .um-head {
      display: flex;
      align-items: center;
      gap: 10px;
      padding: 8px 9px;
    }
    .um-av {
      width: 40px;
      height: 40px;
      border-radius: 50%;
      flex: 0 0 40px;
      background: linear-gradient(135deg, var(--aqua), var(--green));
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 20px;
    }
    .um-name {
      font-weight: 700;
      font-size: 14px;
    }
    .um-mail {
      font-size: 11.5px;
      color: var(--muted);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .um-danger {
      color: var(--red);
    }
    .um-danger:hover {
      background: rgba(227, 73, 72, 0.12);
    }
    .tab-overflow {
      width: 36px;
      flex: 0 0 36px;
      border-left: 1px solid var(--border);
      color: var(--ink2);
      font-size: 15px;
      background: transparent;
      border-top: none;
      border-right: none;
      border-bottom: none;
      cursor: pointer;
    }
    .tab-overflow:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .tab-dropdown {
      position: absolute;
      top: calc(var(--header-h) + 4px);
      right: 14px;
      min-width: 220px;
      max-height: 60vh;
      overflow: auto;
      background: var(--panel);
      border: 1px solid var(--border2);
      border-radius: 10px;
      padding: 5px;
      box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
      z-index: 80;
    }
    .tab-dd-item {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 8px 10px;
      border-radius: 7px;
      font-size: 13px;
      color: var(--ink2);
      cursor: pointer;
    }
    .tab-dd-item:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .tab-dd-item.active {
      color: var(--aqua);
    }
    .tab-dd-item .ddx {
      margin-left: auto;
      color: var(--muted);
      opacity: 0.6;
    }
    .tab-dd-item .ddx:hover {
      color: var(--red);
      opacity: 1;
    }
    .tabctx {
      position: fixed;
      min-width: 150px;
    }
    /* 左右分栏拖动条 */
    .splitter {
      flex: 0 0 6px;
      align-self: stretch;
      cursor: col-resize;
      position: relative;
      z-index: 5;
      background: transparent;
    }
    .splitter::before {
      content: "";
      position: absolute;
      top: 0;
      bottom: 0;
      left: 50%;
      transform: translateX(-50%);
      width: 1px;
      background: var(--border);
    }
    .splitter::after {
      content: "";
      position: absolute;
      left: 50%;
      top: 50%;
      transform: translate(-50%, -50%);
      width: 4px;
      height: 34px;
      border-radius: 3px;
      background: var(--border2);
      transition:
        background 0.15s,
        height 0.15s;
    }
    .splitter:hover::after,
    .splitter.dragging::after {
      background: var(--aqua);
      height: 52px;
    }
    aside {
      width: var(--aside-w);
      flex: 0 0 var(--aside-w);
      display: flex;
      flex-direction: column;
      min-height: 0;
      background: var(--panel);
      border-right: 1px solid var(--border);
    }
    .brand {
      height: var(--header-h);
      display: flex;
      gap: 9px;
      align-items: center;
      padding: 0 4px;
      box-sizing: border-box;
      border-bottom: 1px solid var(--border);
      font-weight: 700;
      color: var(--ink2);
      font-size: 12px;
      letter-spacing: 1px;
      flex: none;
    }
    .brand .uname {
      font-size: 14px;
    }
    .brand .v {
      color: var(--muted);
      font-size: 11px;
      margin-left: auto;
      font-weight: 400;
    }
    .side-body {
      flex: 1;
      min-height: 0;
      display: flex;
      flex-direction: column;
      padding: 8px;
    }
    main {
      flex: 1;
      min-width: 0;
      display: flex;
      flex-direction: column;
      min-height: 0;
      background: var(--bg);
    }
    .tabbar {
      display: flex;
      align-items: stretch;
      height: var(--header-h);
      border-bottom: 1px solid var(--border);
      background: var(--panel);
      flex: none;
      overflow: hidden;
    }
    .tabstrip {
      flex: 1;
      display: flex;
      align-items: stretch;
      overflow-x: auto;
      overflow-y: hidden;
    }
    .tabstrip::-webkit-scrollbar {
      height: 0;
    }
    .tab {
      display: flex;
      align-items: center;
      gap: 7px;
      padding: 0 9px 0 13px;
      max-width: 200px;
      font-size: 12.5px;
      color: var(--ink2);
      cursor: pointer;
      border-right: 1px solid var(--border);
      white-space: nowrap;
      position: relative;
    }
    .tab .t {
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .tab:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .tab.active {
      background: var(--bg);
      color: var(--ink);
    }
    /* 融合式 active：顶部 2px 青条 */
    .tab.active::before {
      content: "";
      position: absolute;
      left: 0;
      right: 0;
      top: 0;
      height: 2px;
      background: var(--aqua);
    }
    .tab .x {
      opacity: 0.55;
      border-radius: 4px;
      padding: 0 3px;
    }
    .tab .x:hover {
      opacity: 1;
      background: var(--hover);
      color: var(--red);
    }
    .home {
      flex: 1;
      min-height: 0;
      overflow-y: auto;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: 18px;
      padding: 32px;
    }
    .hero {
      font-size: 34px;
      font-weight: 700;
      margin-bottom: 26px;
      text-align: center;
      letter-spacing: 1px;
      color: var(--ink);
    }
    .hero .g {
      background: linear-gradient(90deg, var(--aqua), var(--blue));
      -webkit-background-clip: text;
      background-clip: text;
      color: transparent;
    }
    .cats {
      display: flex;
      gap: 10px;
    }
    .cat {
      padding: 8px 18px;
      border-radius: 999px;
      background: var(--panel2);
      border: 1px solid var(--border);
      color: var(--ink2);
      font-size: 13px;
      cursor: pointer;
    }
    .cat.active {
      border-color: var(--aqua);
      color: var(--aqua);
    }
    .quick {
      display: flex;
      flex-wrap: wrap;
      gap: 10px;
      justify-content: center;
      max-width: 640px;
    }
    .qa {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 10px 16px;
      border-radius: 12px;
      background: var(--panel2);
      border: 1px solid var(--border2);
      color: var(--ink2);
      cursor: pointer;
      font-size: 13px;
    }
    .qa:hover {
      border-color: var(--aqua);
      color: var(--ink);
    }
    .msg-area {
      flex: 1;
      min-height: 0;
      display: flex;
      flex-direction: column;
    }
    .bh-t {
      flex: 1;
    }
    .bh-badge {
      font-size: 10px;
      color: var(--muted);
      border: 1px solid var(--border2);
      border-radius: 6px;
      padding: 1px 6px;
    }
    .newbtn {
      display: flex;
      align-items: center;
      gap: 9px;
      width: 100%;
      padding: 11px 12px;
      border-radius: 10px;
      background: var(--surface);
      border: 1px solid var(--border2);
      font-weight: 600;
      font-size: 14px;
      margin: 12px 0 8px;
      color: var(--ink);
      cursor: pointer;
      font-family: inherit;
    }
    .newbtn:hover {
      background: var(--hover);
    }
    .newbtn .plus {
      font-size: 17px;
      color: var(--aqua);
    }
    .navitem {
      display: flex;
      align-items: center;
      gap: 11px;
      padding: 9px 12px;
      border-radius: 8px;
      color: var(--ink2);
      font-size: 14px;
    }
    .navitem:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .navitem .i {
      width: 17px;
      text-align: center;
      opacity: 0.85;
    }
    .navitem .tag {
      margin-left: auto;
      font-size: 11px;
      color: var(--muted);
    }
    .seclabel {
      color: var(--muted);
      font-size: 12px;
      font-weight: 600;
      padding: 14px 12px 6px;
      display: flex;
      align-items: center;
      gap: 6px;
    }
    .seclabel .c {
      color: var(--ink2);
    }
    .task {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 9px 12px;
      border-radius: 8px;
      color: var(--ink2);
      font-size: 13.5px;
      cursor: pointer;
    }
    .task:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .task.active {
      background: var(--surface);
      color: var(--ink);
    }
    .task .tt {
      flex: 1;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .task .when {
      font-size: 11px;
      color: var(--muted);
    }
    .titlebar-line {
      padding: 6px 14px;
      border-bottom: 1px solid var(--border);
      color: var(--muted);
      font-size: 12px;
      background: var(--panel);
      flex: none;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void sessionStore.refreshSessions();
    void settingsStore.loadModels();
    void settingsStore.init();
    // 标题栏 ⇤ 按钮 → 折叠/展开左侧栏（app 壳 dispatch 的全局事件）
    const onToggle = () => (this.sideCollapsed = !this.sideCollapsed);
    window.addEventListener("cmx-agent-toggle-sidebar", onToggle);
    this.offSidebarToggle = () => window.removeEventListener("cmx-agent-toggle-sidebar", onToggle);
    // ESC 关闭弹层 + 点外部关闭（capture 阶段，shadow 内弹层锚定 fixed）
    this.tabKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") this.closeMenus();
    };
    document.addEventListener("keydown", this.tabKeyDown);
    this.docClick = (e: MouseEvent) => {
      if (!(e.composedPath() as Node[]).includes(this)) this.closeMenus();
    };
    document.addEventListener("click", this.docClick);
    queueMicrotask(() => {
      this.offSplitter = this.initSplitter();
    });
    this.offRoute = onRoute((route: Route) => {
      if (route.name === "chat") {
        if (route.sessionId === "connectors" || route.sessionId === "plugins") {
          this.activeTab = route.sessionId;
          // 直接路由（刷新/回退）也要登记 tab（对齐旧 openTab）
          const id = route.sessionId;
          if (!this.openTabs.some((t) => t.id === id)) {
            this.openTabs = [
              ...this.openTabs,
              {
                id,
                kind: id,
                title: id === "connectors" ? "专家·技能·连接器" : "插件"
              }
            ];
          }
        } else if (route.sessionId) {
          this.activeTab = "session";
          void sessionStore.selectSession(route.sessionId);
          this.ensureTab(route.sessionId);
        } else {
          this.activeTab = null;
        }
      }
    });
  }

  disconnectedCallback(): void {
    this.offRoute?.();
    this.offSidebarToggle?.();
    this.offSplitter?.();
    document.removeEventListener("keydown", this.tabKeyDown);
    document.removeEventListener("click", this.docClick);
    super.disconnectedCallback();
  }

  private ensureTab(sessionId: string): void {
    if (!this.openTabs.some((t) => t.id === sessionId)) {
      const meta = this.sessions.state.sessions.find((s: SessionMeta) => s.id === sessionId);
      this.openTabs = [
        ...this.openTabs,
        { id: sessionId, kind: "session", title: meta?.title || sessionId }
      ];
    }
  }

  private activateTab(id: string): void {
    navigate(`#/chat/${encodeURIComponent(id)}`);
  }

  private closeTab(id: string): void {
    const idx = this.openTabs.findIndex((t) => t.id === id);
    this.openTabs = this.openTabs.filter((t) => t.id !== id);
    if (id === "connectors" || id === "plugins") this.pluginDetail = null;
    if (this.sessions.state.currentSessionId === id) {
      const next = this.openTabs[Math.max(0, idx - 1)];
      navigate(next ? `#/chat/${encodeURIComponent(next.id)}` : "#/chat");
    }
  }

  /** 回到 home 空态（对齐旧 newTask：清空当前激活，保留已开 tab）。 */
  private backToHome(): void {
    navigate("#/chat");
  }

  /** 打开连接器 tab（复用同一 tab，对齐旧 openConnectors）。 */
  private openConnectors(): void {
    if (!this.openTabs.some((t) => t.id === "connectors")) {
      this.openTabs = [
        ...this.openTabs,
        { id: "connectors", kind: "connectors", title: "专家·技能·连接器" }
      ];
    }
    navigate("#/chat/connectors");
  }

  /** 打开插件详情 tab（对齐旧 openPluginDetail：所有插件共用一个 tab）。 */
  private openPluginDetail(plugin: PluginInfo, installed: boolean): void {
    this.pluginDetail = { plugin, installed };
    if (!this.openTabs.some((t) => t.id === "plugins")) {
      this.openTabs = [...this.openTabs, { id: "plugins", kind: "plugins", title: "插件" }];
    }
    navigate("#/chat/plugins");
  }

  /** 连接器工具 chip：预填到当前 home 输入框并发起新会话（对齐旧 runConnectorTool）。 */
  private async runConnectorTool(tool: string, online: boolean): Promise<void> {
    if (!online) {
      toast("该连接器离线，无法调用。请先启动对应 cmx 服务。", "error");
      return;
    }
    const prompt = tool.startsWith("flow")
      ? "列出所有流程定义"
      : tool.startsWith("onto")
        ? "列出所有对象类型"
        : "列出所有报表";
    await this.send(prompt, null);
  }

  /** 连接器健康 badge（旧 conn-badge：在线/总数）。 */
  private onConnectorsCount(e: CustomEvent<{ online: number; total: number }>): void {
    this.connBadge = `${e.detail.online}/${e.detail.total}`;
  }

  /** QUICK 快捷动作：预填 home 输入框并聚焦（对齐旧行为，不直接发送）。 */
  private prefillQuick(q: string): void {
    const text = `${q.replace(/^\S+\s/, "")}：`;
    this.homeComposer?.setText(text);
    this.homeComposer?.focus();
  }

  // ── 弹层（cmx 菜单 / 用户菜单 / tab 下拉 / 右键菜单）：互斥 ──
  private closeMenus(): void {
    this.openMenu = "";
  }

  private toggleMenu(m: "cmx" | "user" | "tabdd"): void {
    this.openMenu = this.openMenu === m ? "" : m;
  }

  private openTabCtxMenu(e: MouseEvent, tabId: string): void {
    e.preventDefault();
    this.tabCtx = { x: e.clientX, y: e.clientY, tabId };
    this.openMenu = "tabctx";
  }

  private closeTabById(id: string): void {
    if (id === "connectors" || id === "plugins") {
      this.openTabs = this.openTabs.filter((t) => t.id !== id);
      this.pluginDetail = null;
      if (location.hash === `#/chat/${id}`) navigate("#/chat");
    } else {
      this.closeTab(id);
    }
  }

  private closeOthers(id: string): void {
    this.openTabs.filter((t) => t.id !== id).forEach((t) => this.closeTabById(t.id));
    this.activateTab(id);
  }

  private closeRight(id: string): void {
    const idx = this.openTabs.findIndex((t) => t.id === id);
    this.openTabs.slice(idx + 1).forEach((t) => this.closeTabById(t.id));
  }

  private closeAllTabs(): void {
    [...this.openTabs].forEach((t) => this.closeTabById(t.id));
    navigate("#/chat");
  }

  // ── 用户菜单动作（对齐旧 userProfile/userSwitch/userWorkspace/userLogout）──
  private get userLabel(): string {
    const u = this.auth.state.user;
    const legacy = u as unknown as { nickname?: string };
    return u ? (legacy.nickname ?? u.display_name ?? u.username ?? "已登录") : "未登录";
  }

  private userInitial(): string {
    const name = this.userLabel;
    return name && name !== "未登录" ? name.trim().charAt(0).toUpperCase() : "👤";
  }

  private userProfile(): void {
    this.closeMenus();
    const u = this.auth.state.user as (Record<string, unknown> & { username?: string }) | null;
    if (!u) {
      toast("尚未登录。", "error");
      return;
    }
    const roles = (u.roles as string[] | undefined)?.join("、") ?? "—";
    alert(
      `账户信息\n\n${this.userLabel}（@${u.username ?? ""}）\n用户 ID：${String(u.user_id ?? "")}\n角色：${roles}\n\n对接 cmx 门户统一认证（/api/auth）。`
    );
  }

  private userSwitch(): void {
    this.closeMenus();
    alert("切换账户\n\n退出后重新登录即可切换账户/租户。");
  }

  private userWorkspace(): void {
    this.closeMenus();
    alert("工作空间（占位）\n\n管理沙箱工作区根目录与数据目录。");
  }

  private async userLogout(): Promise<void> {
    this.closeMenus();
    if (!confirm("确定退出登录？")) return;
    await authStore.logout();
    navigate("#/login");
  }

  // ── cmx 菜单动作 ──
  private menuAbout(): void {
    this.closeMenus();
    alert(
      `TrueMate\n版本 ${APP_VERSION}\n\n复刻并超越 WorkBuddy · 同核多壳架构\n形(前门) / 核(回合循环+守卫) / 体(cmx 引擎连接器)`
    );
  }

  private async checkUpdate(): Promise<void> {
    this.closeMenus();
    await new Promise((r) => setTimeout(r, 500));
    alert(`当前已是最新版本（${APP_VERSION}）。\n\n自动更新将在接入发布服务器后启用。`);
  }

  /** 语音输入（Web Speech API，对齐旧 toggleVoice）。 */
  private toggleVoice(): void {
    const w = window as unknown as {
      SpeechRecognition?: new () => SpeechRecognitionLike;
      webkitSpeechRecognition?: new () => SpeechRecognitionLike;
      _rec?: SpeechRecognitionLike | null;
    };
    if (w._rec) {
      try {
        w._rec.stop();
      } catch {
        // 忽略
      }
      w._rec = null;
      return;
    }
    const SR = w.SpeechRecognition ?? w.webkitSpeechRecognition;
    if (!SR) {
      toast("此环境不支持语音识别（需 Web Speech API）", "error");
      return;
    }
    void SR;
    toast("🎙 语音识别仅在支持的浏览器环境可用");
  }

  /** splitter 拖动调宽（200–480 clamp + localStorage 持久化，对齐旧 initSplitter）。 */
  private initSplitter(): () => void {
    const sp = this.shadowRoot?.querySelector<HTMLElement>(".splitter");
    if (!sp) return () => undefined;
    const readW = (): number => {
      try {
        return parseInt(localStorage.getItem("cmx-agent.aside-w") ?? "", 10) || 270;
      } catch {
        return 270;
      }
    };
    let startX = 0;
    let startW = 0;
    let dragging = false;
    const onDown = (e: PointerEvent) => {
      dragging = true;
      startX = e.clientX;
      startW = readW();
      sp.classList.add("dragging");
      sp.setPointerCapture(e.pointerId);
    };
    const onMove = (e: PointerEvent) => {
      if (!dragging) return;
      const w = Math.max(200, Math.min(480, startW + (e.clientX - startX)));
      this.style.setProperty("--aside-w", `${w}px`);
    };
    const onUp = () => {
      if (!dragging) return;
      dragging = false;
      sp.classList.remove("dragging");
      try {
        localStorage.setItem("cmx-agent.aside-w", String(readW()));
      } catch {
        // 忽略
      }
    };
    sp.addEventListener("pointerdown", onDown);
    sp.addEventListener("pointermove", onMove);
    sp.addEventListener("pointerup", onUp);
    sp.addEventListener("pointercancel", onUp);
    // 恢复宽度
    this.style.setProperty("--aside-w", `${readW()}px`);
    return () => {
      sp.removeEventListener("pointerdown", onDown);
      sp.removeEventListener("pointermove", onMove);
      sp.removeEventListener("pointerup", onUp);
      sp.removeEventListener("pointercancel", onUp);
    };
  }

  private async newSession(): Promise<void> {
    const id = `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
    const created = await sessionStore.createSession(id);
    if (created) {
      this.ensureTab(id);
      navigate(`#/chat/${encodeURIComponent(id)}`);
    } else {
      toast("新建会话失败", "error");
    }
  }

  private async send(text: string, sessionId: string | null): Promise<void> {
    let sid = sessionId ?? this.sessions.state.currentSessionId;
    if (!sid) {
      sid = `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
      await sessionStore.createSession(sid);
      this.ensureTab(sid);
      navigate(`#/chat/${encodeURIComponent(sid)}`);
    }
    const currentSid = sid;
    void sessionStore.sendMessage(currentSid, text).then(() => {
      void sessionStore.refreshSessions();
      const tab = this.openTabs.find((t) => t.id === currentSid);
      const meta = this.sessions.state.sessions.find((s: SessionMeta) => s.id === currentSid);
      if (tab && meta?.title) {
        this.openTabs = this.openTabs.map((t) =>
          t.id === currentSid ? { ...t, title: meta.title || t.title } : t
        );
      }
    });
  }

  render() {
    const s = this.sessions.state;
    const st = this.stream.state;
    const se = this.settings.state;
    const current = s.currentSessionId;
    const win = current ? s.windowsBySession[current] : undefined;
    const streamingHere = current ? st.streamingSessionIds.has(current) : false;
    const delta = st.lastDelta?.sessionId === current ? st.lastDelta.text : null;
    return html`
      <div class="activitybar">
        <div
          class="act ${this.sidePanel === "agent" ? "active" : ""}"
          title="Agent"
          @click=${() => (this.sidePanel = "agent")}
        >
          🤖
        </div>
        <div
          class="act ${this.sidePanel === "im" ? "active" : ""}"
          id="act-im"
          title="即时通讯"
          @click=${() => (this.sidePanel = "im")}
        >
          💬<span class="badge">3</span>
        </div>
        <div
          class="act ${this.sidePanel === "plugins" ? "active" : ""}"
          id="act-plugins"
          title="插件管理"
          @click=${() => (this.sidePanel = "plugins")}
        >
          🧩
        </div>
        <div class="act-spacer"></div>
        <div class="act user-act" title=${this.userLabel} @click=${() => this.toggleMenu("user")}>
          <span class="uic">${this.userInitial()}</span>
        </div>
        <div class="act cmx-act" title="设置 · 关于 · 更多" @click=${() => this.toggleMenu("cmx")}>
          <img src="cmx.png" alt="cmx" />
        </div>
      </div>
      <aside id="aside-el" ?hidden=${this.sideCollapsed}>
        <div class="brand">
          ${
            this.sidePanel === "agent"
              ? html`<span class="uname">Agent</span><span class="v">M1</span>`
              : this.sidePanel === "im"
                ? html`<span class="uname">即时通讯</span>`
                : html`<span class="uname">插件管理</span>`
          }
        </div>
        <div class="side-body">
          ${
            this.sidePanel === "agent"
              ? html`
                  <button class="newbtn" type="button" @click=${() => void this.newSession()}>
                    <span class="plus">⊕</span> 新建任务
                  </button>
                  <div class="navitem"><span class="i">🅐</span> 助理</div>
                  <div class="navitem"><span class="i">▦</span> 项目</div>
                  <div class="navitem" @click=${() => this.openConnectors()}>
                    <span class="i">🧩</span> 专家·技能·连接器
                    <span class="tag">${this.connBadge}</span>
                  </div>
                  <div class="navitem"><span class="i">⚙</span> 自动化</div>
                  <div class="navitem">⋯ 更多 <span class="tag">资料库·灵感</span></div>
                  <div class="seclabel">任务 <span class="c">(${s.sessions.length})</span></div>
                  <cmx-agent-session-list
                    .sessions=${s.sessions}
                    .currentSessionId=${current}
                    .streamingIds=${st.streamingSessionIds}
                    .unreadIds=${s.unreadIds}
                    @cmx-agent-select-session=${(e: CustomEvent<{ sessionId: string }>) =>
                      this.activateTab(e.detail.sessionId)}
                    @cmx-agent-delete-session=${(e: CustomEvent<{ sessionId: string }>) => {
                      void sessionStore.deleteSession(e.detail.sessionId);
                      this.openTabs = this.openTabs.filter((t) => t.id !== e.detail.sessionId);
                    }}
                  ></cmx-agent-session-list>
                `
              : this.sidePanel === "im"
                ? html`<cmx-agent-im-panel></cmx-agent-im-panel>`
                : html`<cmx-agent-plugins-side-panel
                    .selectedKey=${this.pluginDetail?.plugin.name ?? ""}
                    .selectedInstalled=${this.pluginDetail?.installed ?? false}
                    @cmx-agent-plugin-select=${(
                      e: CustomEvent<{ plugin: PluginInfo; installed: boolean }>
                    ) => this.openPluginDetail(e.detail.plugin, e.detail.installed)}
                  ></cmx-agent-plugins-side-panel>`
          }
        </div>
      </aside>
      <div class="splitter" title="拖动调整左侧栏宽度"></div>
      <main>
        ${
          this.openTabs.length
            ? html`
                <div class="tabbar">
                  <div class="tabstrip">
                    ${this.openTabs.map(
                      (t) => html`
                        <div
                          class="tab ${
                            (t.kind === "session" &&
                              this.activeTab === "session" &&
                              t.id === current) ||
                            (t.kind !== "session" && this.activeTab === t.id)
                              ? "active"
                              : ""
                          }"
                          @click=${() => this.activateTab(t.id)}
                          @contextmenu=${(e: MouseEvent) => this.openTabCtxMenu(e, t.id)}
                        >
                          <span>${t.kind === "session" ? "💬" : "🧩"}</span
                          ><span class="t">${t.title}</span>
                          <span
                            class="x"
                            @click=${(e: Event) => {
                              e.stopPropagation();
                              this.closeTabById(t.id);
                            }}
                            >✕</span
                          >
                        </div>
                      `
                    )}
                  </div>
                  <button
                    class="tab-overflow"
                    type="button"
                    title="打开的标签页"
                    @click=${() => this.toggleMenu("tabdd")}
                  >
                    ⌄
                  </button>
                </div>
              `
            : nothing
        }
        ${
          this.activeTab === "connectors"
            ? html`<cmx-agent-connectors-view
                @cmx-agent-connectors-count=${(e: CustomEvent<{ online: number; total: number }>) =>
                  this.onConnectorsCount(e)}
                @cmx-agent-run-connector-tool=${(
                  e: CustomEvent<{ tool: string; online: boolean }>
                ) => void this.runConnectorTool(e.detail.tool, e.detail.online)}
              ></cmx-agent-connectors-view>`
            : this.activeTab === "plugins"
              ? html`<cmx-agent-plugins-view
                  .plugin=${this.pluginDetail?.plugin ?? null}
                  .installed=${this.pluginDetail?.installed ?? false}
                  @cmx-agent-plugin-installed=${() => {
                    void settingsStore.loadPlugins();
                    this.pluginDetail = null;
                    this.closeTabById("plugins");
                  }}
                  @cmx-agent-plugin-uninstalled=${() => {
                    void settingsStore.loadPlugins();
                    this.pluginDetail = null;
                    this.closeTabById("plugins");
                  }}
                  @cmx-agent-plugin-changed=${() => void settingsStore.loadPlugins()}
                ></cmx-agent-plugins-view>`
              : current
                ? html`
                    <div class="msg-area">
                      <div class="titlebar-line">${win?.title || current}</div>
                      <cmx-agent-error-note .message=${win?.error ?? ""}></cmx-agent-error-note>
                      <cmx-agent-message-list
                        .events=${win?.events ?? []}
                        .sessionId=${current}
                        .pending=${Boolean(win?.pending) && !streamingHere}
                        .streamingText=${delta}
                        .canLoadEarlier=${Boolean(win && win.start > 0 && win.loaded)}
                        @cmx-agent-load-earlier=${() => void sessionStore.loadEventsBefore(current)}
                        @cmx-agent-approve=${(
                          e: CustomEvent<{
                            callId: string;
                            approved: boolean;
                            all: boolean;
                            sessionId: string;
                          }>
                        ) => {
                          void sessionStore
                            .resolveApproval(
                              e.detail.callId,
                              e.detail.approved,
                              e.detail.all,
                              e.detail.sessionId
                            )
                            .then((ok) =>
                              toast(ok ? "已提交审批" : "审批提交失败", ok ? "info" : "error")
                            );
                        }}
                      ></cmx-agent-message-list>
                      <cmx-agent-composer
                        .draft=${current ? (s.draftsBySession[current] ?? "") : ""}
                        .streaming=${streamingHere}
                        .policy=${se.policy}
                        .models=${se.models}
                        .currentModel=${se.currentModel}
                        @cmx-agent-send=${(e: CustomEvent<{ text: string }>) =>
                          void this.send(e.detail.text, current)}
                        @cmx-agent-draft=${(e: CustomEvent<{ text: string }>) =>
                          sessionStore.saveDraft(current, e.detail.text)}
                        @cmx-agent-policy-change=${(e: CustomEvent<{ policy: Policy }>) =>
                          void settingsStore
                            .setPolicy(e.detail.policy.sandbox, e.detail.policy.approval)
                            .then((ok) => ok || toast("策略切换失败", "error"))}
                        @cmx-agent-model-change=${(
                          e: CustomEvent<{ model: string; pid?: string }>
                        ) =>
                          void settingsStore
                            .setModel(e.detail.model, e.detail.pid)
                            .then((ok) => ok || toast("模型切换失败", "error"))}
                        @cmx-agent-open-model-config=${() => void this.modelCfgDialog?.openDialog()}
                      ></cmx-agent-composer>
                    </div>
                  `
                : html`
                    <div class="home">
                      <div class="hero"><span class="g">TrueMate</span> - 专注工作，真心搭档</div>
                      <div class="cats">
                        <div class="cat active">☕ 日常办公</div>
                        <div class="cat">⌨ 代码开发</div>
                        <div class="cat">🎨 设计创意</div>
                      </div>
                      <div class="quick">
                        ${QUICK.map(
                          (q) => html`
                            <div
                              class="qa"
                              title="点击预填指令（占位示例）"
                              @click=${() => this.prefillQuick(q)}
                            >
                              <span>${q}</span>
                            </div>
                          `
                        )}
                      </div>
                      <cmx-agent-composer
                        id="home-composer"
                        .big=${true}
                        .streaming=${false}
                        .policy=${se.policy}
                        .models=${se.models}
                        .currentModel=${se.currentModel}
                        @cmx-agent-send=${(e: CustomEvent<{ text: string }>) =>
                          void this.send(e.detail.text, null)}
                        @cmx-agent-policy-change=${(e: CustomEvent<{ policy: Policy }>) =>
                          void settingsStore.setPolicy(
                            e.detail.policy.sandbox,
                            e.detail.policy.approval
                          )}
                        @cmx-agent-model-change=${(
                          e: CustomEvent<{ model: string; pid?: string }>
                        ) => void settingsStore.setModel(e.detail.model, e.detail.pid)}
                        @cmx-agent-open-model-config=${() => void this.modelCfgDialog?.openDialog()}
                      ></cmx-agent-composer>
                    </div>
                  `
        }
        ${
          // tab 溢出下拉（对齐旧 toggleTabOverflow）
          this.openMenu === "tabdd"
            ? html`<div class="tab-dropdown">
                ${
                  this.openTabs.length
                    ? this.openTabs.map(
                        (t) => html`
                          <div
                            class="tab-dd-item"
                            @click=${() => {
                              this.activateTab(t.id);
                              this.closeMenus();
                            }}
                          >
                            <span>${t.kind === "session" ? "💬" : "🧩"}</span>
                            <span
                              style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap"
                              >${t.title}</span
                            >
                            <span
                              class="ddx"
                              @click=${(e: Event) => {
                                e.stopPropagation();
                                this.closeTabById(t.id);
                                this.closeMenus();
                              }}
                              >✕</span
                            >
                          </div>
                        `
                      )
                    : html`<div class="tab-dd-item">（没有打开的标签页）</div>`
                }
              </div>`
            : nothing
        }
        ${
          // cmx 功能菜单（活动栏底部图片按钮弹出）
          this.openMenu === "cmx"
            ? html`<div class="cmxmenu">
                <div
                  class="mi"
                  @click=${() => {
                    this.closeMenus();
                    alert(
                      "IM 遥控当前仅在桌面版（Tauri 壳）可配置：Web 壳未装配 IM 桥。\n\n其余设置项（主题/工作空间等）为 M3 占位。"
                    );
                  }}
                >
                  <span class="mi-i">⚙</span> 设置
                </div>
                <div
                  class="mi"
                  @click=${() => {
                    themeStore.toggleTone();
                    this.closeMenus();
                  }}
                >
                  <span class="mi-i">☀</span> 切换主题
                </div>
                <div class="mi" @click=${() => void this.checkUpdate()}>
                  <span class="mi-i">⭮</span> 检查更新
                </div>
                <div class="mi-sep"></div>
                <div class="mi" @click=${() => this.menuAbout()}>
                  <span class="mi-i">ⓘ</span> 关于 TrueMate
                </div>
              </div>`
            : nothing
        }
        ${
          // 用户菜单（活动栏底部用户头像弹出）
          this.openMenu === "user"
            ? html`<div class="cmxmenu usermenu">
                <div class="um-head">
                  <div class="um-av">${this.userInitial()}</div>
                  <div class="um-info">
                    <div class="um-name">${this.userLabel}</div>
                    <div class="um-mail">
                      ${
                        this.auth.state.user
                          ? `@${this.auth.state.user.username ?? ""}`
                          : "点此登录 cmx 门户"
                      }
                    </div>
                  </div>
                </div>
                <div class="mi-sep"></div>
                <div class="mi" @click=${() => this.userProfile()}>
                  <span class="mi-i">👤</span> 账户信息
                </div>
                <div class="mi" @click=${() => this.userSwitch()}>
                  <span class="mi-i">⇄</span> 切换账户
                </div>
                <div class="mi" @click=${() => this.userWorkspace()}>
                  <span class="mi-i">🗂</span> 工作空间
                </div>
                <div class="mi-sep"></div>
                <div class="mi um-danger" @click=${() => void this.userLogout()}>
                  <span class="mi-i">⎋</span> 退出登录
                </div>
              </div>`
            : nothing
        }
        ${
          // tab 右键上下文菜单（fixed 定位）
          this.openMenu === "tabctx"
            ? html`<div
                class="cmxmenu tabctx"
                style="left:${this.tabCtx.x}px;top:${this.tabCtx.y}px"
              >
                <div
                  class="mi"
                  @click=${() => {
                    this.closeTabById(this.tabCtx.tabId);
                    this.closeMenus();
                  }}
                >
                  <span class="mi-i">✕</span> 关闭
                </div>
                <div
                  class="mi"
                  @click=${() => {
                    this.closeOthers(this.tabCtx.tabId);
                    this.closeMenus();
                  }}
                >
                  <span class="mi-i">⊘</span> 关闭其他
                </div>
                <div
                  class="mi"
                  @click=${() => {
                    this.closeRight(this.tabCtx.tabId);
                    this.closeMenus();
                  }}
                >
                  <span class="mi-i">→</span> 关闭右侧
                </div>
                <div class="mi-sep"></div>
                <div
                  class="mi"
                  @click=${() => {
                    this.closeAllTabs();
                    this.closeMenus();
                  }}
                >
                  <span class="mi-i">⊗</span> 全部关闭
                </div>
              </div>`
            : nothing
        }
        <cmx-agent-model-config-dialog
          @cmx-agent-model-config-saved=${() => void settingsStore.loadModels()}
        ></cmx-agent-model-config-dialog>
      </main>
    `;
  }
}
