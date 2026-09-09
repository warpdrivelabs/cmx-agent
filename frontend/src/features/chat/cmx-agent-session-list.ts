import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Button.js";
import "@ui5/webcomponents/dist/Icon.js";
import type { SessionMeta } from "../../protocol/events-window";

/** 会话列表（展示组件）：属性进事件出；流式/未读标记（§8.2.6 等价性配套）。 */
@customElement("cmx-agent-session-list")
export class CmxAgentSessionList extends LitElement {
  @property({ attribute: false }) sessions: SessionMeta[] = [];
  @property() currentSessionId: string | null = null;
  @property({ attribute: false }) streamingIds: ReadonlySet<string> = new Set();
  @property({ attribute: false }) unreadIds: ReadonlySet<string> = new Set();

  static styles = css`
    :host {
      display: block;
      height: 100%;
    }
    /* 任务列表（旧 .tasklist / .task） */
    .list {
      flex: 1;
      overflow: auto;
      margin: 0 -2px;
    }
    .item {
      display: flex;
      align-items: center;
      gap: 8px;
      padding: 9px 12px;
      border-radius: 8px;
      cursor: pointer;
      color: var(--ink2);
      font-size: 13.5px;
    }
    .item:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .item.active {
      background: var(--surface);
      color: var(--ink);
    }
    .title {
      flex: 1;
      min-width: 0;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .when {
      font-size: 11px;
      color: var(--muted);
      flex: 0 0 auto;
    }
    .badge {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      min-width: 18px;
      height: 18px;
      padding: 0 4px;
      border-radius: 9px;
      background: var(--cmx-agent-color-error);
      color: var(--cmx-agent-color-text-inverse);
      font-size: 11px;
    }
    .dot {
      width: 8px;
      height: 8px;
      border-radius: 50%;
      background: var(--aqua);
      animation: pulse 1.2s infinite;
    }
    @keyframes pulse {
      50% {
        opacity: 0.3;
      }
    }
    .del {
      opacity: 0;
      color: var(--muted);
      font-size: 14px;
      padding: 0 2px;
      background: none;
      border: none;
      cursor: pointer;
    }
    .item:hover .del {
      opacity: 0.7;
    }
    .del:hover {
      color: var(--red);
      opacity: 1;
    }
    .empty-tasks {
      color: var(--muted);
      font-size: 12.5px;
      padding: 10px 12px;
      line-height: 1.7;
    }
  `;

  /** updated_at → 相对时间（对齐旧 ago()：今天/昨天/N天前）。 */
  private fmt(iso: string): string {
    const days = (Date.now() - new Date(iso).getTime()) / 86400000;
    if (days < 1) return "今天";
    if (days < 2) return "昨天";
    return `${Math.floor(days)}天前`;
  }

  render() {
    return html`
      <div class="list">
        ${
          this.sessions.length === 0
            ? html`<div class="empty-tasks">
                还没有任务。<br />
                点上方「新建任务」或在首页直接下达指令。
              </div>`
            : this.sessions.map(
                (s) => html`
                  <div
                    class="item ${s.id === this.currentSessionId ? "active" : ""}"
                    @click=${() =>
                      this.dispatchEvent(
                        new CustomEvent("cmx-agent-select-session", {
                          detail: { sessionId: s.id },
                          bubbles: true,
                          composed: true
                        })
                      )}
                  >
                    ${this.streamingIds.has(s.id) ? html`<span class="dot"></span>` : ""}
                    <span class="title">${s.title || s.id}</span>
                    <span class="when">${this.fmt(s.updated_at)}</span>
                    ${this.unreadIds.has(s.id) ? html`<span class="badge">新</span>` : ""}
                    <button
                      class="del"
                      title="删除"
                      @click=${(e: Event) => {
                        e.stopPropagation();
                        this.dispatchEvent(
                          new CustomEvent("cmx-agent-delete-session", {
                            detail: { sessionId: s.id },
                            bubbles: true,
                            composed: true
                          })
                        );
                      }}
                    >
                      ✕
                    </button>
                  </div>
                `
              )
        }
      </div>
    `;
  }
}
