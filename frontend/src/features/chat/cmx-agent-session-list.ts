import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Button.js";
import "@ui5/webcomponents/dist/Icon.js";
import type { SessionMeta } from "../../protocol/events-window";
import "../common/cmx-agent-filter-list";
import "../common/cmx-agent-empty-state";

/** 会话列表（展示组件）：属性进事件出；流式/未读标记（§8.2.6 等价性配套）。 */
@customElement("cmx-agent-session-list")
export class CmxAgentSessionList extends LitElement {
  @property({ attribute: false }) sessions: SessionMeta[] = [];
  @property() currentSessionId: string | null = null;
  @property({ attribute: false }) streamingIds: ReadonlySet<string> = new Set();
  @property({ attribute: false }) unreadIds: ReadonlySet<string> = new Set();
  @property() keyword = "";

  static styles = css`
    :host {
      display: block;
      height: 100%;
    }
    .item {
      display: flex;
      align-items: center;
      gap: var(--cmx-agent-space-sm);
      padding: var(--cmx-agent-space-sm) var(--cmx-agent-space-md);
      border-radius: var(--cmx-agent-border-radius);
      cursor: pointer;
      color: var(--cmx-agent-color-text);
    }
    .item:hover {
      background: var(--cmx-agent-bg-hover);
    }
    .item.active {
      background: var(--cmx-agent-color-primary-bg);
    }
    .title {
      flex: 1;
      min-width: 0;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
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
      background: var(--cmx-agent-color-success);
      animation: pulse 1.2s infinite;
    }
    @keyframes pulse {
      50% {
        opacity: 0.3;
      }
    }
    .del {
      display: none;
      border: none;
      background: none;
      color: var(--cmx-agent-color-text-tertiary);
      cursor: pointer;
      font-size: var(--cmx-agent-font-size-sm);
    }
    .item:hover .del {
      display: inline;
    }
  `;

  private get filtered(): SessionMeta[] {
    const kw = this.keyword.trim().toLowerCase();
    if (!kw) return this.sessions;
    return this.sessions.filter(
      (s) => s.id.toLowerCase().includes(kw) || (s.title ?? "").toLowerCase().includes(kw)
    );
  }

  render() {
    return html`
      <cmx-agent-filter-list
        placeholder="搜索会话"
        @cmx-agent-filter=${(e: CustomEvent<{ keyword: string }>) =>
          (this.keyword = e.detail.keyword)}
      >
        ${
          this.filtered.length === 0
            ? html`<cmx-agent-empty-state heading="暂无会话"></cmx-agent-empty-state>`
            : this.filtered.map(
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
                    ${this.streamingIds.has(s.id) ? html`<span class="dot"></span>` : html`<span>💬</span>`}
                    <span class="title">${s.title || s.id}</span>
                    ${this.unreadIds.has(s.id) ? html`<span class="badge">新</span>` : ""}
                    <button
                      class="del"
                      title="删除会话"
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
      </cmx-agent-filter-list>
    `;
  }
}
