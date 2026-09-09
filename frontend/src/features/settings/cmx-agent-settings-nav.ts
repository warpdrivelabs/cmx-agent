import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";

@customElement("cmx-agent-settings-nav")
export class CmxAgentSettingsNav extends LitElement {
  @property({ attribute: false })
  items: Array<{ id: string; label: string }> = [];
  @property() active = "";

  static styles = css`
    .item {
      display: flex;
      align-items: center;
      gap: 10px;
      padding: 11px 14px;
      margin: 2px 4px;
      border-radius: 9px;
      cursor: pointer;
      color: var(--ink2);
      font-size: 13.5px;
    }
    .item:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .item.active {
      background: var(--accent-soft);
      color: var(--aqua);
      font-weight: 600;
      box-shadow: inset 3px 0 0 var(--aqua);
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
