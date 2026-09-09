import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";

/** 面板骨架（§9.5.3）：标题 + 内容 + 操作条，业务面板只填 slot。 */
@customElement("cmx-agent-panel-card")
export class CmxAgentPanelCard extends LitElement {
  @property() heading = "";

  static styles = css`
    :host {
      display: block;
    }
    .card {
      background: var(--cmx-agent-bg-container);
      border: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border);
      border-radius: var(--cmx-agent-border-radius-lg);
      box-shadow: var(--cmx-agent-box-shadow);
      overflow: hidden;
    }
    header {
      padding: var(--cmx-agent-space-md) var(--cmx-agent-space-lg);
      border-bottom: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border-secondary);
      font-size: var(--cmx-agent-font-size-lg);
      color: var(--cmx-agent-color-text);
    }
    .body {
      padding: var(--cmx-agent-space-lg);
    }
    footer {
      display: flex;
      justify-content: flex-end;
      gap: var(--cmx-agent-space-sm);
      padding: var(--cmx-agent-space-md) var(--cmx-agent-space-lg);
      border-top: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border-secondary);
    }
    footer:empty {
      display: none;
    }
  `;

  render() {
    return html`
      <section class="card">
        ${this.heading ? html`<header>${this.heading}</header>` : ""}
        <div class="body"><slot></slot></div>
        <footer><slot name="actions"></slot></footer>
      </section>
    `;
  }
}
