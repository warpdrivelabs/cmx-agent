import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Select.js";
import "@ui5/webcomponents/dist/Option.js";
import "@ui5/webcomponents/dist/Label.js";
import {
  APPROVAL_POLICIES,
  SANDBOX_MODES,
  type ApprovalPolicy,
  type Policy,
  type SandboxMode
} from "../../protocol/policy";

/** composer 两旋钮快速切换（展示组件）：变更抛事件，容器调 settings-store（保留在 composer 内，§2.3）。 */
@customElement("cmx-agent-policy-picker")
export class CmxAgentPolicyPicker extends LitElement {
  @property({ attribute: false }) policy: Policy = { sandbox: "read-only", approval: "on-request" };
  @property({ type: Boolean }) disabled = false;

  static styles = css`
    :host {
      display: inline-flex;
      align-items: center;
      gap: var(--cmx-agent-space-xs);
    }
    ui5-select {
      min-width: 120px;
    }
  `;

  private pick(part: "sandbox" | "approval", value: string): void {
    this.dispatchEvent(
      new CustomEvent("cmx-agent-policy-change", {
        detail: { policy: { ...this.policy, [part]: value } },
        bubbles: true,
        composed: true
      })
    );
  }

  render() {
    return html`
      <ui5-select
        .disabled=${this.disabled}
        title="沙箱能力（两旋钮之一）"
        @change=${(e: CustomEvent<{ selectedOption?: { value: string } }>) =>
          this.pick("sandbox", e.detail.selectedOption?.value ?? "read-only")}
      >
        ${SANDBOX_MODES.map(
          (m: SandboxMode) =>
            html`<ui5-option ?selected=${m === this.policy.sandbox} .value=${m}>🛡 ${m}</ui5-option>`
        )}
      </ui5-select>
      <ui5-select
        .disabled=${this.disabled}
        title="审批许可（两旋钮之一，与沙箱正交）"
        @change=${(e: CustomEvent<{ selectedOption?: { value: string } }>) =>
          this.pick("approval", e.detail.selectedOption?.value ?? "on-request")}
      >
        ${APPROVAL_POLICIES.map(
          (m: ApprovalPolicy) =>
            html`<ui5-option ?selected=${m === this.policy.approval} .value=${m}
              >🔑 ${m}</ui5-option
            >`
        )}
      </ui5-select>
    `;
  }
}
