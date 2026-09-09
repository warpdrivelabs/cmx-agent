import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Button.js";

/** 审批卡片（展示组件，§9.5.2）：按钮点击抛领域事件，容器（chat-view）调 store。 */
@customElement("cmx-agent-approval-card")
export class CmxAgentApprovalCard extends LitElement {
  @property({ attribute: false })
  request!: { call_id: string; tool: string; reason: string; seq: number };
  @property() sessionId = "";

  static styles = css`
    :host {
      display: block;
      margin: var(--cmx-agent-space-sm) 0;
    }
    .card {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      gap: var(--cmx-agent-space-md);
      border: var(--cmx-agent-line-width) solid var(--cmx-agent-color-warning);
      border-radius: var(--cmx-agent-border-radius);
      background: var(--cmx-agent-bg-container);
      padding: var(--cmx-agent-space-md);
    }
    .info {
      flex: 1;
      min-width: 200px;
    }
    .tool {
      font-weight: 600;
      color: var(--cmx-agent-color-text);
    }
    .reason {
      color: var(--cmx-agent-color-text-secondary);
      font-size: var(--cmx-agent-font-size-sm);
    }
    .actions {
      display: flex;
      gap: var(--cmx-agent-space-sm);
    }
  `;

  private decide(approved: boolean, all: boolean): void {
    this.dispatchEvent(
      new CustomEvent("cmx-agent-approve", {
        detail: { callId: this.request.call_id, approved, all, sessionId: this.sessionId },
        bubbles: true,
        composed: true
      })
    );
  }

  render() {
    return html`
      <div class="card">
        <div class="info">
          <div class="tool">⚠ 需要审批：${this.request.tool}</div>
          <div class="reason">${this.request.reason}</div>
        </div>
        <div class="actions">
          <ui5-button design="Negative" @click=${() => this.decide(false, false)}>拒绝</ui5-button>
          <ui5-button @click=${() => this.decide(true, true)}>本对话全部允许</ui5-button>
          <ui5-button design="Emphasized" @click=${() => this.decide(true, false)}>允许</ui5-button>
        </div>
      </div>
    `;
  }
}
