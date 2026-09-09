import { LitElement, html, css } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Input.js";

/** 过滤列表骨架（§9.5.3）：搜索框 + 列表 + 空态；业务列表只写 item slot。 */
@customElement("cmx-agent-filter-list")
export class CmxAgentFilterList extends LitElement {
  @property({ type: String }) placeholder = "搜索";
  @property({ type: Boolean }) searchable = true;
  @state() private keyword = "";

  static styles = css`
    :host {
      display: flex;
      flex-direction: column;
      min-height: 0;
      height: 100%;
    }
    ui5-input {
      margin-bottom: var(--cmx-agent-space-sm);
    }
    .list {
      flex: 1;
      min-height: 0;
      overflow-y: auto;
    }
  `;

  render() {
    return html`
      ${
        this.searchable
          ? html`<ui5-input
              placeholder=${this.placeholder}
              .value=${this.keyword}
              @input=${(e: InputEvent) => {
                this.keyword = (e.target as HTMLInputElement).value;
                this.dispatchEvent(
                  new CustomEvent("cmx-agent-filter", {
                    detail: { keyword: this.keyword },
                    bubbles: true,
                    composed: true
                  })
                );
              }}
            ></ui5-input>`
          : ""
      }
      <div class="list"><slot></slot></div>
    `;
  }
}
