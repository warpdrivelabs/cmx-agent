import { LitElement, html, css } from "lit";
import { customElement } from "lit/decorators.js";
import { StoreController } from "../../platform/store-controller";
import { settingsStore } from "../../platform/stores/settings-store";
import "../common/cmx-agent-panel-card";
import "../common/cmx-agent-empty-state";

/** 连接器面板（容器→store）：描述 + live 健康。 */
@customElement("cmx-agent-connector-panel")
export class CmxAgentConnectorPanel extends LitElement {
  private settings = new StoreController(this, settingsStore);

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
      flex: 1;
      color: var(--cmx-agent-color-text-secondary);
      font-size: var(--cmx-agent-font-size-sm);
    }
    .live {
      padding: 2px 8px;
      border-radius: var(--cmx-agent-border-radius-sm);
      font-size: var(--cmx-agent-font-size-sm);
      background: var(--cmx-agent-color-success);
      color: var(--cmx-agent-color-text-inverse);
    }
    .live.off {
      background: var(--cmx-agent-bg-hover);
      color: var(--cmx-agent-color-text-tertiary);
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void settingsStore.loadConnectors();
  }

  render() {
    const s = this.settings.state;
    return html`
      <cmx-agent-panel-card heading="专家 · 技能 · 连接器">
        ${
          s.connectors.length === 0
            ? html`<cmx-agent-empty-state heading="暂无连接器"></cmx-agent-empty-state>`
            : s.connectors.map(
                (c) => html`
                  <div class="row">
                    <span class="name">${c.name}</span>
                    <span class="desc">${c.description ?? ""}</span>
                    <span class="live ${c.live ? "" : "off"}">${c.live ? "在线" : "离线"}</span>
                  </div>
                `
              )
        }
      </cmx-agent-panel-card>
    `;
  }
}
