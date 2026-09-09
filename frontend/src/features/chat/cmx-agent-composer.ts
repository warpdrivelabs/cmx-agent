import { LitElement, html, css } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Button.js";
import type { Policy } from "../../protocol/policy";
import "./cmx-agent-policy-picker";
import "./cmx-agent-model-picker";

/** 输入区（展示+领域事件）：发送/草稿/两旋钮/模型切换全部抛事件，容器接 store。 */
@customElement("cmx-agent-composer")
export class CmxAgentComposer extends LitElement {
  @property() draft = "";
  @property({ type: Boolean }) streaming = false;
  @property({ type: Boolean }) disabled = false;
  @property({ attribute: false }) policy: Policy = { sandbox: "read-only", approval: "on-request" };
  @property({ attribute: false }) models: string[] = [];
  @property() currentModel = "";
  @state() private text = "";

  static styles = css`
    :host {
      display: block;
      padding: var(--cmx-agent-space-md) var(--cmx-agent-space-lg);
      border-top: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border-secondary);
      background: var(--cmx-agent-bg-container);
    }
    .box {
      display: flex;
      flex-direction: column;
      gap: var(--cmx-agent-space-sm);
      max-width: 860px;
      margin: 0 auto;
    }
    textarea {
      width: 100%;
      box-sizing: border-box;
      min-height: 48px;
      max-height: 200px;
      padding: var(--cmx-agent-space-sm) var(--cmx-agent-space-md);
      border: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border);
      border-radius: var(--cmx-agent-border-radius-lg);
      background: var(--cmx-agent-bg-container);
      color: var(--cmx-agent-color-text);
      font: inherit;
      resize: none;
      outline: none;
    }
    textarea:focus-visible {
      border-color: var(--cmx-agent-color-primary);
    }
    .row {
      display: flex;
      align-items: center;
      gap: var(--cmx-agent-space-sm);
    }
    .spacer {
      flex: 1;
    }
  `;

  protected updated(changed: Map<string, unknown>): void {
    if (changed.has("draft") && !this.text) {
      this.text = this.draft; // 切回会话恢复草稿（一次）
    }
  }

  private send(): void {
    const text = this.text.trim();
    if (!text || this.streaming || this.disabled) return;
    this.text = "";
    this.dispatchEvent(
      new CustomEvent("cmx-agent-send", { detail: { text }, bubbles: true, composed: true })
    );
  }

  private sync(value: string): void {
    this.text = value;
    this.dispatchEvent(
      new CustomEvent("cmx-agent-draft", { detail: { text: value }, bubbles: true, composed: true })
    );
  }

  render() {
    return html`
      <div class="box">
        <textarea
          .value=${this.text}
          placeholder="发消息…  ⏎ 发送 · ⇧⏎ 换行"
          @input=${(e: InputEvent) => {
            this.sync((e.target as HTMLTextAreaElement).value);
            const el = e.target as HTMLTextAreaElement;
            el.style.height = "auto";
            el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
          }}
          @keydown=${(e: KeyboardEvent) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              this.send();
            }
          }}
        ></textarea>
        <div class="row">
          <cmx-agent-policy-picker
            .policy=${this.policy}
            .disabled=${this.streaming}
            @cmx-agent-policy-change=${(e: CustomEvent<{ policy: Policy }>) =>
              this.dispatchEvent(
                new CustomEvent("cmx-agent-policy-change", {
                  detail: e.detail,
                  bubbles: true,
                  composed: true
                })
              )}
          ></cmx-agent-policy-picker>
          <cmx-agent-model-picker
            .models=${this.models}
            .current=${this.currentModel}
            .disabled=${this.streaming}
            @cmx-agent-model-change=${(e: CustomEvent<{ model: string }>) =>
              this.dispatchEvent(
                new CustomEvent("cmx-agent-model-change", {
                  detail: e.detail,
                  bubbles: true,
                  composed: true
                })
              )}
          ></cmx-agent-model-picker>
          <span class="spacer"></span>
          <ui5-button
            design="Emphasized"
            icon="navigation-up-arrow"
            .disabled=${this.streaming || this.disabled || !this.text.trim()}
            @click=${() => this.send()}
          >
            ${this.streaming ? "生成中…" : "发送"}
          </ui5-button>
        </div>
      </div>
    `;
  }
}
