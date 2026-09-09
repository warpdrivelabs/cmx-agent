import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";

@customElement("cmx-agent-settings-nav")
export class CmxAgentSettingsNav extends LitElement {
  @property({ attribute: false })
  items: Array<{ id: string; label: string }> = [];
  @property() active = "";

  static styles = css`
    .item {
      display: block;
      padding: var(--cmx-agent-space-sm) var(--cmx-agent-space-md);
      border-radius: var(--cmx-agent-border-radius);
      cursor: pointer;
      color: var(--cmx-agent-color-text-secondary);
    }
    .item:hover {
      background: var(--cmx-agent-bg-hover);
    }
    .item.active {
      background: var(--cmx-agent-color-primary-bg);
      color: var(--cmx-agent-color-primary);
    }
  `;

  render() {
    return html`${this.items.map(
      (it) =>
        html`<div
          class="item ${it.id === this.active ? "active" : ""}"
          @click=${() =>
            this.dispatchEvent(
              new CustomEvent("cmx-agent-nav", {
                detail: { id: it.id },
                bubbles: true,
                composed: true
              })
            )}
        >
          ${it.label}
        </div>`
    )}`;
  }
}
