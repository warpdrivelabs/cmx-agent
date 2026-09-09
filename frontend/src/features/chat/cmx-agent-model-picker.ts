import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Select.js";
import "@ui5/webcomponents/dist/Option.js";

/** composer 模型快速切换（展示组件）：list_models / set_model 由容器接。 */
@customElement("cmx-agent-model-picker")
export class CmxAgentModelPicker extends LitElement {
  @property({ attribute: false }) models: string[] = [];
  @property() current = "";
  @property({ type: Boolean }) disabled = false;

  static styles = css`
    :host {
      display: inline-flex;
      align-items: center;
    }
    ui5-select {
      min-width: 180px;
    }
  `;

  render() {
    return html`
      <ui5-select
        .disabled=${this.disabled}
        title="模型切换"
        @change=${(e: CustomEvent<{ selectedOption?: { value: string } }>) =>
          this.dispatchEvent(
            new CustomEvent("cmx-agent-model-change", {
              detail: { model: e.detail.selectedOption?.value ?? "" },
              bubbles: true,
              composed: true
            })
          )}
      >
        ${this.models.map(
          (m) => html`<ui5-option ?selected=${m === this.current} .value=${m}>◎ ${m}</ui5-option>`
        )}
      </ui5-select>
    `;
  }
}
