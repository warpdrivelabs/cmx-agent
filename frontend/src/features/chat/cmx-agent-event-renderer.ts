import { LitElement, html } from "lit";
import { customElement, property } from "lit/decorators.js";
import type { SessionEvent } from "../../protocol/session-event";
import "./cmx-agent-tool-event-card";
import "./cmx-agent-approval-card";

/** 事件 → 卡片唯一 dispatch（§9.5.3）：按 kind 查表，新增事件类型只注册一行。 */
@customElement("cmx-agent-event-renderer")
export class CmxAgentEventRenderer extends LitElement {
  @property({ attribute: false }) event!: SessionEvent;
  @property() sessionId = "";

  render() {
    const k = this.event?.kind;
    if (!k) return html``;
    switch (k.kind) {
      case "tool_invoked":
        return html`<cmx-agent-tool-event-card
          .call=${k.call}
          state="running"
        ></cmx-agent-tool-event-card>`;
      case "tool_result":
        return html`<cmx-agent-tool-event-card
          .result=${{ call_id: k.call_id, ok: k.ok, output: k.output }}
          state="done"
        ></cmx-agent-tool-event-card>`;
      case "guard_decision":
        return html`<cmx-agent-tool-event-card
          .guard=${{ guard: k.guard, phase: k.phase, decision: k.decision }}
          state="guard"
        ></cmx-agent-tool-event-card>`;
      case "approval_requested":
        return html`<cmx-agent-approval-card
          .request=${{ call_id: k.call_id, tool: k.tool, reason: k.reason, seq: this.event.seq }}
          .sessionId=${this.sessionId}
        ></cmx-agent-approval-card>`;
      case "approval_resolved":
        return html``;
      case "note":
        return html`<div class="note">${k.text}</div>`;
      default:
        return html``; // turn_started/ended/user/model 由 message-list 直接渲染
    }
  }

  static styles = [];
}
