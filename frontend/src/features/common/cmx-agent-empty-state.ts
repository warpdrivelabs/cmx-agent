import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";

@customElement("cmx-agent-empty-state")
export class CmxAgentEmptyState extends LitElement {
  @property() heading = "";
  @property() hint = "";

  static styles = css`
    :host {
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
      gap: var(--cmx-agent-space-sm);
      padding: var(--cmx-agent-space-xl);
      color: var(--cmx-agent-color-text-tertiary);
    }
    .heading {
      font-size: var(--cmx-agent-font-size-lg);
      color: var(--cmx-agent-color-text-secondary);
    }
  `;

  render() {
    return html`${this.heading ? html`<div class="heading">${this.heading}</div>` : ""}
      ${this.hint ? html`<div>${this.hint}</div>` : ""} <slot></slot>`;
  }
}
