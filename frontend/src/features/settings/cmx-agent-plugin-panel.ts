import { LitElement, html, css } from "lit";
import { customElement, state } from "lit/decorators.js";
import { StoreController } from "../../platform/store-controller";
import { settingsStore } from "../../platform/stores/settings-store";
import "@ui5/webcomponents/dist/Button.js";
import { toast } from "../common/cmx-agent-toast";
import "../common/cmx-agent-panel-card";

/** 插件面板（容器→store）：安装/卸载/启停。 */
@customElement("cmx-agent-plugin-panel")
export class CmxAgentPluginPanel extends LitElement {
  private settings = new StoreController(this, settingsStore);
  @state() private busyName = "";

  static styles = css`
    .row {
      display: flex;
      align-items: center;
      gap: var(--cmx-agent-space-sm);
      padding: var(--cmx-agent-space-sm) 0;
      border-bottom: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border-secondary);
    }
    .row:last-child {
      border-bottom: none;
    }
    .name {
      font-weight: 600;
      color: var(--cmx-agent-color-text);
    }
    .desc {
      color: var(--cmx-agent-color-text-tertiary);
      font-size: var(--cmx-agent-font-size-sm);
      flex: 1;
      min-width: 0;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .tag {
      font-size: var(--cmx-agent-font-size-sm);
      color: var(--cmx-agent-color-info);
    }
    .tag.off {
      color: var(--cmx-agent-color-text-tertiary);
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void settingsStore.loadPlugins();
  }

  private async toggle(p: { name: string; enabled?: boolean }): Promise<void> {
    this.busyName = p.name;
    const ok = await settingsStore.togglePlugin(p.name, !p.enabled);
    this.busyName = "";
    if (ok) toast("操作成功");
    else toast(this.settings.state.error ?? "操作失败", "error");
  }

  private async install(p: { name: string }): Promise<void> {
    this.busyName = p.name;
    const ok = await settingsStore.installPlugin(p);
    this.busyName = "";
    if (ok) toast("已安装");
    else toast(this.settings.state.error ?? "安装失败", "error");
  }

  private async uninstall(p: { name: string }): Promise<void> {
    this.busyName = p.name;
    const ok = await settingsStore.uninstallPlugin(p.name);
    this.busyName = "";
    if (ok) toast("已卸载");
    else toast(this.settings.state.error ?? "卸载失败", "error");
  }

  render() {
    const s = this.settings.state;
    return html`
      <cmx-agent-panel-card heading="插件">
        ${
          s.plugins.length === 0
            ? html`<p>暂无插件</p>`
            : s.plugins.map(
                (p) => html`
                  <div class="row">
                    <span class="name">${p.name}</span>
                    <span class="tag ${p.enabled ? "" : "off"}">
                      ${
                        p.installed === false || p.market ? "市场" : p.enabled ? "已启用" : "已禁用"
                      }
                    </span>
                    <span class="desc">${p.description ?? ""}</span>
                    ${
                      p.installed === false || p.market
                        ? html`<ui5-button
                            .disabled=${this.busyName === p.name}
                            @click=${() => void this.install(p)}
                            >安装</ui5-button
                          >`
                        : html`
                            <ui5-button
                              .disabled=${this.busyName === p.name}
                              @click=${() => void this.toggle(p)}
                              >${p.enabled ? "禁用" : "启用"}</ui5-button
                            >
                            <ui5-button
                              design="Negative"
                              .disabled=${this.busyName === p.name}
                              @click=${() => void this.uninstall(p)}
                              >卸载</ui5-button
                            >
                          `
                    }
                  </div>
                `
              )
        }
      </cmx-agent-panel-card>
    `;
  }
}
