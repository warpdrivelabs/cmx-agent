import { LitElement, html, css } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { StoreController } from "../../platform/store-controller";
import { settingsStore } from "../../platform/stores/settings-store";
import type { PluginInfo } from "../../protocol/response";

/** kind → 图标兜底 / 底色 / 中文名（对齐旧 UI KIND_ICON/KIND_COLOR/KIND_LABEL）。 */
const KIND_ICON: Record<string, string> = {
  http: "🌐",
  command: "⌘",
  wasm: "🧱",
  mcp: "🔌",
  connector: "🧩"
};
const KIND_COLOR: Record<string, string> = {
  http: "#2a78d6",
  command: "#4a3aa7",
  wasm: "#1baf7a",
  mcp: "#eda100",
  connector: "#eb6834"
};
const KIND_LABEL: Record<string, string> = {
  http: "HTTP 端点",
  command: "Shell 命令",
  wasm: "WebAssembly",
  mcp: "MCP 服务",
  connector: "连接器"
};

/** 市场演示目录（后端 market 为空时兜底，对齐旧 PLUG_MARKET_DEMO）。 */
const PLUG_MARKET_DEMO: PluginInfo[] = [
  {
    name: "cmx-rules",
    kind: "http",
    version: "1.0.0",
    author: "cmx",
    description: "决策表 / FEEL 规则评估连接器",
    icon: "⚖",
    homepage: "https://feel.cmx.dev/",
    installable: true,
    manifest: {
      name: "cmx-rules",
      kind: "http",
      version: "1.0.0",
      description: "决策表 / FEEL 规则评估连接器",
      icon: "⚖",
      homepage: "https://feel.cmx.dev/",
      base_url: "http://127.0.0.1:8094",
      method: "POST",
      path: "/api/rules/v1/evaluate",
      permissions: ["net:fetch"]
    }
  },
  {
    name: "doc-tools",
    kind: "command",
    version: "0.9.0",
    author: "社区",
    description: "Office / PDF 解析与生成（命令载体示例）",
    icon: "📄",
    homepage: "https://example.com/",
    installable: true,
    manifest: {
      name: "doc-tools",
      kind: "command",
      version: "0.9.0",
      description: "Office / PDF 解析（命令载体示例）",
      icon: "📄",
      author: "社区",
      homepage: "https://example.com/",
      command: "echo",
      args: ["doc {file}"],
      requires_approval: true,
      permissions: ["exec"]
    }
  },
  {
    name: "mcp-bridge",
    kind: "mcp",
    version: "1.2.0",
    author: "cmx",
    description: "接入任意 Model Context Protocol 服务",
    icon: "🔌",
    homepage: "https://modelcontextprotocol.io/",
    installable: true,
    manifest: {
      name: "mcp-bridge",
      kind: "mcp",
      version: "1.2.0",
      description: "接入任意 MCP 服务",
      icon: "🔌",
      author: "cmx",
      homepage: "https://modelcontextprotocol.io/",
      command: "my-mcp-server",
      args: ["--stdio"]
    }
  }
];

export function plugIcon(p: PluginInfo): string {
  return p.icon || KIND_ICON[p.kind] || "🧩";
}
export function plugColor(p: PluginInfo): string {
  return KIND_COLOR[p.kind] || "#6b7280";
}
export function plugMeta(p: PluginInfo, installed: boolean): string {
  const state = installed ? (p.enabled === false ? "已禁用" : "已启用") : null;
  return [
    p.author || "cmx",
    p.version ? "v" + p.version : null,
    KIND_LABEL[p.kind] || p.kind,
    state
  ]
    .filter(Boolean)
    .join(" · ");
}

/**
 * 侧栏插件面板（master，对齐旧 loadPluginSidebar/renderPluginSidebar）：
 * 搜索 + 刷新 + 已装/市场分组列表；点行抛 cmx-agent-plugin-select 开详情 tab。
 */
@customElement("cmx-agent-plugins-side-panel")
export class CmxAgentPluginsSidePanel extends LitElement {
  private settings = new StoreController(this, settingsStore);
  @property() selectedKey = "";
  @property({ type: Boolean }) selectedInstalled = false;
  @state() private keyword = "";
  @state() private loading = false;

  static styles = css`
    :host {
      display: flex;
      flex-direction: column;
      flex: 1;
      min-height: 0;
    }
    .pg-search {
      display: flex;
      gap: 6px;
      padding: 2px 0 10px;
    }
    .pg-search input {
      flex: 1;
      background: var(--surface);
      border: 1px solid var(--border2);
      color: var(--ink);
      border-radius: 8px;
      padding: 7px 10px;
      font-size: 12.5px;
      outline: none;
      font-family: inherit;
      min-width: 0;
    }
    .pg-search input:focus {
      border-color: var(--aqua);
    }
    .pg-refresh {
      flex: 0 0 auto;
      background: var(--surface);
      border: 1px solid var(--border2);
      color: var(--ink2);
      border-radius: 8px;
      padding: 0 10px;
      cursor: pointer;
    }
    .pg-refresh:hover {
      background: var(--hover);
    }
    .pg-list {
      flex: 1;
      overflow: auto;
      margin: 0 -4px;
    }
    .pg-sec {
      font-size: 11px;
      font-weight: 700;
      color: var(--muted);
      padding: 12px 8px 5px;
      letter-spacing: 0.5px;
    }
    .pg-hint,
    .pg-loading {
      color: var(--muted);
      font-size: 12px;
      padding: 8px 10px;
    }
    .plug-item {
      display: flex;
      gap: 10px;
      padding: 9px 8px;
      border-radius: 9px;
      cursor: pointer;
    }
    .plug-item:hover {
      background: var(--hover);
    }
    .plug-item.sel {
      background: var(--accent-soft);
    }
    .plug-item.sel .pn {
      color: var(--aqua);
    }
    .plug-item.pg-off .pic {
      filter: grayscale(1);
      opacity: 0.55;
    }
    .plug-item.pg-off .pn {
      color: var(--muted);
    }
    .pic {
      width: 38px;
      height: 38px;
      border-radius: 9px;
      flex: 0 0 38px;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 19px;
      color: var(--ws-white);
    }
    .pm {
      flex: 1;
      min-width: 0;
    }
    .pn {
      font-weight: 600;
      font-size: 13.5px;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .pd {
      font-size: 12px;
      color: var(--muted);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
      margin-top: 1px;
    }
    .pmeta {
      font-size: 11px;
      color: var(--muted);
      margin-top: 3px;
    }
    .pbtn {
      align-self: center;
      font-size: 11.5px;
      padding: 4px 10px;
      border-radius: 7px;
      border: 1px solid var(--border2);
      color: var(--ink2);
      flex: 0 0 auto;
    }
    .pbtn.installed {
      color: var(--aqua);
      border-color: var(--aqua);
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.loading = true;
    await settingsStore.loadPlugins();
    this.loading = false;
  }

  private pick(p: PluginInfo, installed: boolean): void {
    this.dispatchEvent(
      new CustomEvent("cmx-agent-plugin-select", {
        detail: { plugin: p, installed },
        bubbles: true,
        composed: true
      })
    );
  }

  private row(p: PluginInfo, installed: boolean) {
    const off = installed && p.enabled === false;
    const sel = installed === this.selectedInstalled && p.name === this.selectedKey;
    return html`
      <div
        class="plug-item ${sel ? "sel" : ""} ${off ? "pg-off" : ""}"
        @click=${() => this.pick(p, installed)}
      >
        <div class="pic" style="background:${plugColor(p)}">${plugIcon(p)}</div>
        <div class="pm">
          <div class="pn">${p.name}</div>
          <div class="pd">${p.description ?? ""}</div>
          <div class="pmeta">${plugMeta(p, installed)}</div>
        </div>
        <div class="pbtn ${installed ? "installed" : ""}">
          ${installed ? (off ? "已禁用" : "已安装") : "安装"}
        </div>
      </div>
    `;
  }

  render() {
    const s = this.settings.state;
    const installed = s.installedPlugins;
    let market = s.marketPlugins;
    if (!market.length) market = PLUG_MARKET_DEMO;
    const kw = this.keyword.trim().toLowerCase();
    const hit = (p: PluginInfo) =>
      !kw ||
      (p.name ?? "").toLowerCase().includes(kw) ||
      (p.description ?? "").toLowerCase().includes(kw);
    return html`
      <div class="pg-search">
        <input
          type="text"
          placeholder="🔍 搜索已安装 / 市场插件"
          .value=${this.keyword}
          @input=${(e: InputEvent) => (this.keyword = (e.target as HTMLInputElement).value)}
        />
        <button class="pg-refresh" title="刷新" type="button" @click=${() => void this.load()}>
          ⟳
        </button>
      </div>
      <div class="pg-list">
        ${
          this.loading
            ? html`<div class="pg-loading">正在加载插件…</div>`
            : html`
                <div class="pg-sec">已安装 (${installed.length})</div>
                ${
                  installed.length
                    ? installed.filter(hit).map((p) => this.row(p, true))
                    : html`<div class="pg-hint">（暂无已安装插件；从市场安装后重启生效）</div>`
                }
                <div class="pg-sec">市场 (${market.length})</div>
                ${market.filter(hit).map((p) => this.row(p, false))}
              `
        }
      </div>
    `;
  }
}
