import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import "@ui5/webcomponents/dist/BusyIndicator.js";

@customElement("cmx-agent-loading-block")
export class CmxAgentLoadingBlock extends LitElement {
  @property({ type: Boolean }) active = false;
  @property() text = "加载中…";

  static styles = css`
    :host {
      display: block;
    }
    .wrap {
      display: flex;
      align-items: center;
      justify-content: center;
      gap: var(--cmx-agent-space-sm);
      padding: var(--cmx-agent-space-lg);
      color: var(--cmx-agent-color-text-secondary);
    }
  `;

  render() {
    return html`${
      this.active
        ? html`<div class="wrap">
            <ui5-busyindicator size="M"></ui5-busyindicator><span>${this.text}</span>
          </div>`
        : ""
    }`;
  }
}
