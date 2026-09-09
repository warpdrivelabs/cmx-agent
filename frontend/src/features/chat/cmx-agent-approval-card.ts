import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";

/** 审批卡片（展示组件，§9.5.2）：按钮点击抛领域事件，容器（chat-view）调 store。 */
@customElement("cmx-agent-approval-card")
export class CmxAgentApprovalCard extends LitElement {
  @property({ attribute: false })
  request!: { call_id: string; tool: string; reason: string; seq: number };
  @property() sessionId = "";

  static styles = css`
    :host {
      display: block;
      background: var(--panel2);
      background-image: linear-gradient(180deg, rgba(217, 168, 78, 0.06), transparent);
      border: 1px solid rgba(217, 168, 78, 0.55);
      border-radius: 10px;
      padding: 10px 14px;
      font-size: 12.5px;
    }
    .card {
      display: flex;
      flex-wrap: wrap;
      align-items: center;
      gap: 12px;
    }
    .info {
      flex: 1;
      min-width: 200px;
    }
    .tool {
      font-weight: 600;
      color: rgba(217, 168, 78, 1);
    }
    .reason {
      color: var(--ink2);
      margin-top: 4px;
    }
    .actions {
      display: flex;
      gap: 9px;
    }
    .apbtn {
      flex: 0 0 auto;
      padding: 7px 18px;
      border-radius: 9px;
      font-size: 13px;
      font-weight: 600;
      cursor: pointer;
      border: 1px solid var(--border2);
      background: var(--panel);
      color: var(--ink2);
      font-family: inherit;
    }
    .apbtn.allow {
      background: linear-gradient(135deg, var(--aqua), var(--green));
      color: var(--ws-white);
      border-color: transparent;
    }
    .apbtn.allow:hover {
      filter: brightness(1.08);
    }
    .apbtn.reject:hover {
      border-color: var(--red);
      color: var(--red);
    }
    .apbtn.allowall {
      color: rgba(217, 168, 78, 1);
    }
    .apbtn.allowall:hover {
      border-color: rgba(217, 168, 78, 0.8);
      background: rgba(217, 168, 78, 0.1);
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
          <button class="apbtn reject" @click=${() => this.decide(false, false)}>拒绝</button>
          <button class="apbtn allowall" @click=${() => this.decide(true, true)}>
            本对话全部允许
          </button>
          <button class="apbtn allow" @click=${() => this.decide(true, false)}>允许</button>
        </div>
      </div>
    `;
  }
}
