import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import "@ui5/webcomponents/dist/MessageStrip.js";

@customElement("cmx-agent-error-note")
export class CmxAgentErrorNote extends LitElement {
  @property() message = "";

  static styles = css`
    :host {
      display: block;
    }
  `;

  render() {
    return html`${
      this.message
        ? html`<ui5-message-strip design="Negative">${this.message}</ui5-message-strip>`
        : ""
    }`;
  }
}
