import { LitElement, html, css } from "lit";
import { customElement, state } from "lit/decorators.js";

const TOAST_EVENT = "cmx-agent-toast";

/** 全局操作反馈（§2.3）：任意代码 dispatchEvent(cmx-agent-toast) 即可弹提示。 */
@customElement("cmx-agent-toast")
export class CmxAgentToast extends LitElement {
  @state() private message = "";
  @state() private tone: "info" | "error" = "info";
  private timer?: number;

  static styles = css`
    :host {
      position: fixed;
      left: 50%;
      bottom: var(--cmx-agent-space-xl);
      transform: translateX(-50%);
      z-index: 1000;
      pointer-events: none;
    }
    .toast {
      padding: var(--cmx-agent-space-sm) var(--cmx-agent-space-lg);
      border-radius: var(--cmx-agent-border-radius);
      background: var(--cmx-agent-bg-elevated);
      color: var(--cmx-agent-color-text);
      box-shadow: var(--cmx-agent-box-shadow);
      font-size: var(--cmx-agent-font-size);
    }
    .toast.error {
      border: var(--cmx-agent-line-width) solid var(--cmx-agent-color-error);
      color: var(--cmx-agent-color-error);
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    window.addEventListener(TOAST_EVENT, this.onToast as EventListener);
  }

  disconnectedCallback(): void {
    window.removeEventListener(TOAST_EVENT, this.onToast as EventListener);
    super.disconnectedCallback();
  }

  private onToast = (e: CustomEvent<{ message: string; tone?: "info" | "error" }>) => {
    this.message = e.detail.message;
    this.tone = e.detail.tone ?? "info";
    window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => (this.message = ""), 2600);
  };

  render() {
    return html`${this.message ? html`<div class="toast ${this.tone}">${this.message}</div>` : ""}`;
  }
}

export function toast(message: string, tone: "info" | "error" = "info"): void {
  window.dispatchEvent(new CustomEvent(TOAST_EVENT, { detail: { message, tone } }));
}
