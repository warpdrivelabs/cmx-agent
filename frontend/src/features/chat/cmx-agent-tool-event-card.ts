import { LitElement, html, css } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import type { ToolCall } from "../../protocol/session-event";

/** 工具事件卡片：调用中 / 结果 / 守卫裁决三态（输出可折叠）。 */
@customElement("cmx-agent-tool-event-card")
export class CmxAgentToolEventCard extends LitElement {
  @property({ attribute: false }) call?: ToolCall;
  @property({ attribute: false })
  result?: { call_id: string; ok: boolean; output: unknown };
  @property({ attribute: false })
  guard?: { guard: string; phase: string; decision: Record<string, unknown> };
  @property() state: "running" | "done" | "guard" = "running";
  @state() private expanded = false;

  static styles = css`
    :host {
      display: block;
      margin: var(--cmx-agent-space-sm) 0;
    }
    .card {
      border: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border);
      border-left: 3px solid var(--cmx-agent-color-info);
      border-radius: var(--cmx-agent-border-radius);
      background: var(--cmx-agent-bg-container);
      padding: var(--cmx-agent-space-sm) var(--cmx-agent-space-md);
      font-size: var(--cmx-agent-font-size-sm);
    }
    .card.err {
      border-left-color: var(--cmx-agent-color-error);
    }
    .head {
      display: flex;
      align-items: center;
      gap: var(--cmx-agent-space-sm);
      cursor: pointer;
      color: var(--cmx-agent-color-text-secondary);
    }
    .name {
      font-family: ui-monospace, monospace;
      color: var(--cmx-agent-color-text);
    }
    pre {
      margin: var(--cmx-agent-space-sm) 0 0;
      padding: var(--cmx-agent-space-sm);
      background: var(--cmx-agent-bg-layout);
      border-radius: var(--cmx-agent-border-radius-sm);
      overflow: auto;
      max-height: 220px;
      white-space: pre-wrap;
      word-break: break-all;
      color: var(--cmx-agent-color-text);
    }
  `;

  private get headline(): string {
    if (this.call) return this.call.name;
    if (this.result) return `结果 · ${this.result.call_id}`;
    return this.guard ? `守卫 · ${this.guard.guard}` : "";
  }

  private get bodyText(): string {
    if (this.call) return JSON.stringify(this.call.arguments, null, 2);
    if (this.result)
      return typeof this.result.output === "string"
        ? this.result.output
        : JSON.stringify(this.result.output, null, 2);
    return this.guard ? JSON.stringify(this.guard.decision, null, 2) : "";
  }

  render() {
    const isErr = this.result && !this.result.ok;
    return html`
      <div class="card ${isErr ? "err" : ""}">
        <div class="head" @click=${() => (this.expanded = !this.expanded)}>
          <span>${this.state === "running" ? "⏳" : isErr ? "❌" : this.guard ? "🛡" : "✅"}</span>
          <span class="name">${this.headline}</span>
          <span>${this.expanded ? "▾" : "▸"}</span>
        </div>
        ${this.expanded ? html`<pre>${this.bodyText}</pre>` : ""}
      </div>
    `;
  }
}
