import { LitElement, html, css, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { settingsStore } from "../../platform/stores/settings-store";
import { toast } from "../common/cmx-agent-toast";
import type { PluginInfo } from "../../protocol/response";
import { plugColor, plugIcon, plugMeta } from "./cmx-agent-plugins-side-panel";

const KIND_LABEL: Record<string, string> = {
  http: "HTTP 端点",
  command: "Shell 命令",
  wasm: "WebAssembly",
  mcp: "MCP 服务",
  connector: "连接器"
};

/** 插件详情 tab（对齐旧 tpl-plugins + renderPluginDetail + 安装/卸载/启停交互）。 */
@customElement("cmx-agent-plugins-view")
export class CmxAgentPluginsView extends LitElement {
  @property({ attribute: false }) plugin: PluginInfo | null = null;
  @property({ type: Boolean }) installed = false;
  @state() private confirmUninstall = false;
  @state() private busy = "";
  private confirmTimer?: number;

  static styles = css`
    :host {
      flex: 1;
      display: flex;
      flex-direction: column;
      min-height: 0;
      overflow: auto;
      background: var(--bg);
    }
    .pg-empty {
      color: var(--muted);
      font-size: 13px;
      padding: 40px 30px;
      text-align: center;
    }
    .pg-detail-inner {
      padding: 0 0 30px;
    }
    .pg-topbar {
      height: 3px;
      background: var(--muted);
    }
    .is-installed .pg-topbar {
      background: var(--aqua);
    }
    .pg-head {
      display: flex;
      gap: 16px;
      padding: 22px 26px 12px;
    }
    .pg-ico {
      width: 64px;
      height: 64px;
      flex: 0 0 64px;
      border-radius: 14px;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 34px;
      color: var(--ws-white);
    }
    .pg-h-main {
      min-width: 0;
      flex: 1;
    }
    .pg-title {
      font-size: 19px;
      font-weight: 700;
      display: flex;
      align-items: center;
      gap: 10px;
      flex-wrap: wrap;
    }
    .pg-sub {
      color: var(--muted);
      font-size: 12.5px;
      margin-top: 4px;
    }
    .pg-desc {
      color: var(--ink2);
      font-size: 13.5px;
      margin-top: 8px;
      line-height: 1.5;
    }
    .pg-badge {
      font-size: 11px;
      font-weight: 600;
      padding: 3px 9px;
      border-radius: 20px;
      border: 1px solid;
    }
    .pg-badge.installed {
      color: var(--aqua);
      border-color: var(--aqua);
      background: var(--accent-soft);
    }
    .pg-badge.disabled {
      color: var(--muted);
      border-color: var(--muted);
    }
    .pg-badge.market {
      color: var(--muted);
      border-color: var(--border2);
    }
    .pg-actions {
      display: flex;
      gap: 8px;
      margin-top: 14px;
      flex-wrap: wrap;
    }
    .pg-actbtn {
      font-size: 12.5px;
      padding: 6px 14px;
      border-radius: 8px;
      border: 1px solid var(--border2);
      background: var(--surface);
      color: var(--ink);
      cursor: pointer;
      font-family: inherit;
    }
    .pg-actbtn:hover {
      background: var(--hover);
    }
    .pg-actbtn.primary {
      background: var(--aqua);
      border-color: var(--aqua);
      color: var(--ws-white);
    }
    .pg-actbtn.primary:hover {
      filter: brightness(1.06);
    }
    .pg-actbtn.ghost {
      background: transparent;
      color: var(--ink2);
    }
    .pg-actbtn.danger {
      background: var(--red);
      border-color: var(--red);
      color: var(--ws-white);
    }
    .pg-actbtn:disabled {
      opacity: 0.6;
      cursor: default;
    }
    .pg-sec-h {
      font-size: 12px;
      font-weight: 700;
      color: var(--muted);
      letter-spacing: 0.5px;
      padding: 16px 26px 8px;
      border-top: 1px solid var(--border);
      margin-top: 14px;
    }
    .pg-chips {
      display: flex;
      flex-wrap: wrap;
      gap: 7px;
      padding: 2px 26px 4px;
    }
    .pg-chip {
      font-size: 11.5px;
      padding: 4px 10px;
      border-radius: 7px;
      background: var(--surface);
      border: 1px solid var(--border2);
      color: var(--ink2);
    }
    .pg-chip.perm {
      color: var(--violet);
      border-color: var(--violet);
    }
    .pg-chip.warn {
      color: var(--orange);
      border-color: var(--orange);
    }
    .pg-kvs {
      padding: 6px 26px;
    }
    .pg-kv {
      display: flex;
      gap: 10px;
      font-size: 12.5px;
      padding: 3px 0;
    }
    .pg-kv .k {
      flex: 0 0 90px;
      color: var(--muted);
    }
    .pg-kv .v {
      color: var(--ink2);
      word-break: break-all;
      font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    }
    .pg-frame-bar {
      padding: 4px 26px 8px;
      font-size: 12px;
      color: var(--muted);
    }
    .pg-link {
      color: var(--blue);
      cursor: pointer;
      text-decoration: underline;
      word-break: break-all;
    }
    .pg-frame-hint {
      color: var(--muted);
      margin-left: 6px;
    }
    .pg-frame {
      display: block;
      width: calc(100% - 52px);
      height: 60vh;
      margin: 0 26px;
      border: 1px solid var(--border2);
      border-radius: 10px;
      background: var(--ws-white);
    }
    .pg-empty.sm {
      padding: 16px;
      text-align: left;
    }
  `;

  private kv(k: string, v: unknown) {
    if (v == null || v === "") return nothing;
    return html`<div class="pg-kv">
      <span class="k">${k}</span><span class="v">${String(v)}</span>
    </div>`;
  }

  private async doInstall(): Promise<void> {
    const p = this.plugin;
    if (!p) return;
    if (!p.manifest) {
      toast("该市场条目未内嵌清单，无法安装", "error");
      return;
    }
    this.busy = "install";
    const ok = await settingsStore.installPlugin(p.manifest);
    this.busy = "";
    if (ok) {
      toast(`已安装「${p.name}」`);
      this.dispatchEvent(
        new CustomEvent("cmx-agent-plugin-installed", { bubbles: true, composed: true })
      );
    } else {
      toast("安装失败：" + (settingsStore.getState().error ?? "未知错误"), "error");
    }
  }

  private async doUninstall(): Promise<void> {
    const p = this.plugin;
    if (!p) return;
    // 两段式确认（对齐旧 pluginUninstall）：首点变红「确认卸载」，3s 内再点执行
    if (!this.confirmUninstall) {
      this.confirmUninstall = true;
      this.confirmTimer = window.setTimeout(() => (this.confirmUninstall = false), 3000);
      return;
    }
    window.clearTimeout(this.confirmTimer);
    this.confirmUninstall = false;
    this.busy = "uninstall";
    const ok = await settingsStore.uninstallPlugin(p.name);
    this.busy = "";
    if (ok) {
      toast(`已卸载「${p.name}」`);
      this.dispatchEvent(
        new CustomEvent("cmx-agent-plugin-uninstalled", { bubbles: true, composed: true })
      );
    } else {
      toast("卸载失败：" + (settingsStore.getState().error ?? "未知错误"), "error");
    }
  }

  private async doToggle(): Promise<void> {
    const p = this.plugin;
    if (!p) return;
    const enable = p.enabled === false;
    this.busy = "toggle";
    const ok = await settingsStore.togglePlugin(p.name, enable);
    this.busy = "";
    if (ok) {
      toast(enable ? "已启用" : "已禁用");
      this.dispatchEvent(
        new CustomEvent("cmx-agent-plugin-changed", { bubbles: true, composed: true })
      );
    } else {
      toast("操作失败：" + (settingsStore.getState().error ?? "未知错误"), "error");
    }
  }

  render() {
    const p = this.plugin;
    if (!p) {
      return html`<div class="pg-empty">← 在左侧插件列表中选择一个插件查看详情</div>`;
    }
    const enabled = p.enabled !== false;
    const badge = this.installed
      ? enabled
        ? html`<span class="pg-badge installed">✓ 已安装 · 已启用</span>`
        : html`<span class="pg-badge disabled">⊘ 已安装 · 已禁用</span>`
      : html`<span class="pg-badge market">☁ 未安装</span>`;
    const actions = this.installed
      ? html`
          <button
            class="pg-actbtn ${this.confirmUninstall ? "danger" : ""}"
            type="button"
            .disabled=${this.busy !== ""}
            @click=${() => void this.doUninstall()}
          >
            ${this.busy === "uninstall" ? "卸载中…" : this.confirmUninstall ? "确认卸载" : "卸载"}
          </button>
          <button
            class="pg-actbtn ghost"
            type="button"
            .disabled=${this.busy !== ""}
            @click=${() => void this.doToggle()}
          >
            ${this.busy === "toggle" ? "切换中…" : enabled ? "禁用" : "启用"}
          </button>
        `
      : html`
          <button
            class="pg-actbtn primary"
            type="button"
            .disabled=${this.busy !== ""}
            @click=${() => void this.doInstall()}
          >
            ${this.busy === "install" ? "安装中…" : "安装"}
          </button>
        `;
    const openBtn = p.homepage
      ? html`<button
          class="pg-actbtn ghost"
          type="button"
          @click=${() => {
            try {
              window.open(p.homepage, "_blank", "noopener");
            } catch {
              toast("无法打开链接", "error");
            }
          }}
        >
          ⧉ 在浏览器打开
        </button>`
      : nothing;
    const chips = [
      html`<span class="pg-chip">${KIND_LABEL[p.kind] ?? p.kind ?? "插件"}</span>`,
      ...(p.permissions ?? []).map((pm) => html`<span class="pg-chip perm">🔒 ${pm}</span>`),
      p.requires_approval ? html`<span class="pg-chip warn">需审批</span>` : nothing
    ];
    const carrier = [
      p.kind === "http" ? this.kv("base_url", p.base_url) : nothing,
      p.kind === "http" ? this.kv("method", p.method) : nothing,
      p.kind === "http" ? this.kv("path", p.path) : nothing,
      p.kind === "command" || p.kind === "mcp" ? this.kv("command", p.command) : nothing,
      (p.kind === "command" || p.kind === "mcp") && (p.args ?? []).length
        ? this.kv("args", (p.args ?? []).join(" "))
        : nothing,
      p.kind === "wasm" ? this.kv("module", p.module) : nothing,
      p.kind === "wasm" ? this.kv("invoke", p.invoke) : nothing,
      p.kind === "wasm" ? this.kv("runtime", p.runtime) : nothing
    ].filter((x) => x !== nothing);
    return html`
      <div class="pg-detail-inner ${this.installed ? "is-installed" : "is-market"}">
        <div class="pg-topbar"></div>
        <div class="pg-head">
          <div class="pg-ico" style="background:${plugColor(p)}">${plugIcon(p)}</div>
          <div class="pg-h-main">
            <div class="pg-title">${p.name} ${badge}</div>
            <div class="pg-sub">${plugMeta(p, this.installed)}</div>
            <div class="pg-desc">${p.description ?? ""}</div>
            <div class="pg-actions">${actions}${openBtn}</div>
          </div>
        </div>
        <div class="pg-sec-h">能力与权限</div>
        <div class="pg-chips">${chips}</div>
        ${carrier.length ? html`<div class="pg-kvs">${carrier}</div>` : nothing}
        <div class="pg-sec-h">信息</div>
        ${
          p.homepage
            ? html`
                <div class="pg-frame-bar">
                  信息页：<a
                    class="pg-link"
                    @click=${() => window.open(p.homepage, "_blank", "noopener")}
                    >${p.homepage}</a
                  ><span class="pg-frame-hint"
                    >（若下方空白说明该页禁止内嵌，请点链接在浏览器打开）</span
                  >
                </div>
                <iframe
                  class="pg-frame"
                  src=${p.homepage}
                  referrerpolicy="no-referrer"
                  sandbox="allow-scripts allow-same-origin allow-popups allow-forms"
                ></iframe>
              `
            : html`<div class="pg-empty sm">
                该插件未提供信息页（cmx-plugin.json 的 homepage 字段）。
              </div>`
        }
      </div>
    `;
  }
}
