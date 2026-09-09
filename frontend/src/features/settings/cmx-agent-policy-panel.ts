import { LitElement, html } from "lit";
import { customElement } from "lit/decorators.js";
import { StoreController } from "../../platform/store-controller";
import { settingsStore } from "../../platform/stores/settings-store";
import "../common/cmx-agent-panel-card";
import "../chat/cmx-agent-policy-picker";

/** 沙箱与审批策略面板：与 composer picker 共享 settings-store（两旋钮唯一真源）。 */
@customElement("cmx-agent-policy-panel")
export class CmxAgentPolicyPanel extends LitElement {
  private settings = new StoreController(this, settingsStore);

  render() {
    const s = this.settings.state;
    return html`
      <cmx-agent-panel-card heading="沙箱与审批（两旋钮正交）">
        <p>
          当前：沙箱能力 <strong>${s.policy.sandbox}</strong> · 审批许可
          <strong>${s.policy.approval}</strong>
        </p>
        <cmx-agent-policy-picker
          .policy=${s.policy}
          @cmx-agent-policy-change=${(e: CustomEvent) => {
            const p = e.detail.policy;
            void settingsStore.setPolicy(p.sandbox, p.approval);
          }}
        ></cmx-agent-policy-picker>
      </cmx-agent-panel-card>
    `;
  }
}
