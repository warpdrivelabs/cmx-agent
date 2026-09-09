import { LitElement, html, css } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Button.js";
import type { SessionEvent } from "../../protocol/session-event";
import "./cmx-agent-event-renderer";
import "../common/cmx-agent-empty-state";

/**
 * 消息时间线（容器属性进）：只渲染 user/model 气泡 + 事件卡；
 * 流式 delta 合并到末条（§8.0：一帧一提交，不逐 token 重渲染整表）。
 */
@customElement("cmx-agent-message-list")
export class CmxAgentMessageList extends LitElement {
  @property({ attribute: false }) events: SessionEvent[] = [];
  @property({ attribute: false }) streamingText: string | null = null;
  @property() sessionId = "";
  @property({ type: Boolean }) pending = false;
  @property({ type: Boolean }) canLoadEarlier = false;
  @state() private earlierLoading = false;

  static styles = css`
    :host {
      display: flex;
      flex-direction: column;
      flex: 1;
      min-height: 0;
      overflow-y: auto;
      padding: var(--cmx-agent-space-lg);
      gap: var(--cmx-agent-space-sm);
    }
    .msg {
      max-width: 82%;
      padding: var(--cmx-agent-space-sm) var(--cmx-agent-space-md);
      border-radius: var(--cmx-agent-border-radius-lg);
      white-space: pre-wrap;
      word-break: break-word;
      line-height: 1.6;
      font-size: var(--cmx-agent-font-size);
    }
    .msg.user {
      align-self: flex-end;
      background: var(--cmx-agent-color-primary-bg);
      color: var(--cmx-agent-color-text);
    }
    .msg.model {
      align-self: flex-start;
      background: var(--cmx-agent-bg-container);
      border: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border-secondary);
      color: var(--cmx-agent-color-text);
    }
    .typing {
      align-self: flex-start;
      color: var(--cmx-agent-color-text-tertiary);
      padding: var(--cmx-agent-space-xs) var(--cmx-agent-space-md);
    }
    .load-earlier {
      align-self: center;
    }
  `;

  private renderEvent(ev: SessionEvent) {
    const k = ev.kind;
    switch (k.kind) {
      case "user_message":
        return html`<div class="msg user">${k.text}</div>`;
      case "model_message":
        return k.text ? html`<div class="msg model">${k.text}</div>` : html``;
      default:
        return html`<cmx-agent-event-renderer
          .event=${ev}
          .sessionId=${this.sessionId}
        ></cmx-agent-event-renderer>`;
    }
  }

  render() {
    if (!this.events.length && !this.pending) {
      return html`<cmx-agent-empty-state
        heading="开始新对话"
        hint="在下方输入框发消息"
      ></cmx-agent-empty-state>`;
    }
    return html`
      ${
        this.canLoadEarlier
          ? html`<ui5-button
              design="Transparent"
              class="load-earlier"
              .disabled=${this.earlierLoading}
              @click=${() => {
                this.earlierLoading = true;
                this.dispatchEvent(
                  new CustomEvent("cmx-agent-load-earlier", { bubbles: true, composed: true })
                );
                window.setTimeout(() => (this.earlierLoading = false), 600);
              }}
            >
              ${this.earlierLoading ? "加载中…" : "加载更早消息"}
            </ui5-button>`
          : ""
      }
      ${this.events.map((ev) => this.renderEvent(ev))}
      ${
        this.streamingText !== null
          ? html`<div class="msg model">${this.streamingText}</div>`
          : this.pending
            ? html`<div class="typing">正在思考…</div>`
            : ""
      }
    `;
  }
}
