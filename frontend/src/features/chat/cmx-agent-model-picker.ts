import { LitElement, html, css } from "lit";
import { customElement, property, query, state } from "lit/decorators.js";
import "./cmx-agent-model-menu";

/** composer 模型快速切换：外观与交互对齐旧 ui/index.html「◎ Auto ▾」按钮 + modelmenu 弹层。 */
@customElement("cmx-agent-model-picker")
export class CmxAgentModelPicker extends LitElement {
  @property({ attribute: false }) models: string[] = [];
  @property() current = "";
  @property({ type: Boolean }) disabled = false;
  @state() private menuAnchor: DOMRect | null = null;
  @query("cmx-agent-model-menu")
  private menu?: HTMLElement & { show: () => Promise<void> };

  static styles = css`
    :host {
      position: relative;
      display: inline-flex;
    }
    /* 旧 .chat-composer .model 按钮 */
    .model {
      display: flex;
      align-items: center;
      gap: 5px;
      height: 28px;
      color: var(--ink2);
      font-size: 12.5px;
      padding: 0 10px;
      border-radius: 8px;
      border: 1px solid var(--border);
      background: transparent;
      cursor: pointer;
      font-family: inherit;
    }
    .model:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .model.disabled {
      opacity: 0.5;
      cursor: default;
    }
    .mdot {
      color: var(--aqua);
    }
    .mlabel {
      font-weight: 600;
      max-width: 180px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
  `;

  /** 短标签（对齐旧 setModelLabel：demo→Demo，去 deepseek- 前缀；空则 Auto）。 */
  private get label(): string {
    if (!this.current) return "Auto";
    if (this.current === "demo") return "Demo";
    return this.current.replace(/^deepseek-/, "").replace(/^gpt-/, "gpt-");
  }

  render() {
    return html`
      <div
        class="model ${this.disabled ? "disabled" : ""}"
        title="模型选择"
        @click=${(e: MouseEvent) => {
          if (this.disabled) return;
          e.stopPropagation();
          if (this.menuAnchor) {
            this.menuAnchor = null;
            return;
          }
          this.menuAnchor = (e.currentTarget as HTMLElement).getBoundingClientRect();
          void this.menu?.show();
        }}
      >
        <span class="mdot">◎</span>
        <span class="mlabel">${this.label}</span>
        <span>▾</span>
      </div>
      <cmx-agent-model-menu
        .anchor=${this.menuAnchor}
        ?open=${this.menuAnchor !== null}
        @cmx-agent-model-change=${(e: CustomEvent<{ model: string }>) => {
          this.menuAnchor = null;
          this.dispatchEvent(
            new CustomEvent("cmx-agent-model-change", {
              detail: e.detail,
              bubbles: true,
              composed: true
            })
          );
        }}
        @cmx-agent-open-model-config=${() => {
          this.menuAnchor = null;
          this.dispatchEvent(
            new CustomEvent("cmx-agent-open-model-config", { bubbles: true, composed: true })
          );
        }}
      ></cmx-agent-model-menu>
    `;
  }
}
