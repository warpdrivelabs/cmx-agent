import { LitElement, html, css } from "lit";
import { customElement } from "lit/decorators.js";
import "@ui5/webcomponents/dist/Button.js";
import { StoreController } from "../../platform/store-controller";
import { sessionStore } from "../../platform/stores/session-store";
import { streamStore } from "../../platform/stores/stream-store";
import { settingsStore } from "../../platform/stores/settings-store";
import { navigate, onRoute, type Route } from "../../router";
import { toast } from "../common/cmx-agent-toast";
import type { Policy } from "../../protocol/policy";
import "./cmx-agent-session-list";
import "./cmx-agent-message-list";
import "./cmx-agent-composer";
import "../common/cmx-agent-error-note";

/** 聊天工作台（容器组件）：唯一感知 store 的位置，领域事件全部在此接 store。 */
@customElement("cmx-agent-chat-view")
export class CmxAgentChatView extends LitElement {
  private sessions = new StoreController(this, sessionStore);
  private stream = new StoreController(this, streamStore);
  private settings = new StoreController(this, settingsStore);
  private offRoute?: () => void;

  static styles = css`
    :host {
      display: flex;
      height: 100%;
      min-height: 0;
      background: var(--cmx-agent-bg-layout);
    }
    aside {
      width: 260px;
      flex: none;
      display: flex;
      flex-direction: column;
      border-right: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border);
      background: var(--cmx-agent-bg-container);
      padding: var(--cmx-agent-space-sm);
    }
    .new-btn {
      margin-bottom: var(--cmx-agent-space-sm);
    }
    main {
      flex: 1;
      min-width: 0;
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .titlebar {
      padding: var(--cmx-agent-space-sm) var(--cmx-agent-space-lg);
      border-bottom: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border-secondary);
      color: var(--cmx-agent-color-text-secondary);
      font-size: var(--cmx-agent-font-size);
      background: var(--cmx-agent-bg-container);
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void sessionStore.refreshSessions();
    void settingsStore.loadModels();
    this.offRoute = onRoute((route: Route) => {
      if (route.name === "chat" && route.sessionId) {
        void sessionStore.selectSession(route.sessionId);
      }
    });
  }

  disconnectedCallback(): void {
    this.offRoute?.();
    super.disconnectedCallback();
  }

  private async newSession(): Promise<void> {
    const id = `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
    const created = await sessionStore.createSession(id);
    if (created) {
      navigate(`#/chat/${encodeURIComponent(id)}`);
    } else {
      toast("新建会话失败", "error");
    }
  }

  private async send(text: string): Promise<void> {
    let sid = this.sessions.state.currentSessionId;
    if (!sid) {
      sid = `s-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
      await sessionStore.createSession(sid);
      navigate(`#/chat/${encodeURIComponent(sid)}`);
    }
    const currentSid = sid;
    void sessionStore.sendMessage(currentSid, text).then(() => {
      void sessionStore.refreshSessions();
    });
  }

  render() {
    const s = this.sessions.state;
    const st = this.stream.state;
    const se = this.settings.state;
    const win = s.currentSessionId ? s.windowsBySession[s.currentSessionId] : undefined;
    const streamingHere = s.currentSessionId
      ? st.streamingSessionIds.has(s.currentSessionId)
      : false;
    const delta = st.lastDelta?.sessionId === s.currentSessionId ? st.lastDelta.text : null;
    return html`
      <aside>
        <ui5-button design="Emphasized" class="new-btn" @click=${() => void this.newSession()}>
          ＋ 新建会话
        </ui5-button>
        <cmx-agent-session-list
          .sessions=${s.sessions}
          .currentSessionId=${s.currentSessionId}
          .streamingIds=${st.streamingSessionIds}
          .unreadIds=${s.unreadIds}
          @cmx-agent-select-session=${(e: CustomEvent<{ sessionId: string }>) =>
            navigate(`#/chat/${encodeURIComponent(e.detail.sessionId)}`)}
          @cmx-agent-delete-session=${(e: CustomEvent<{ sessionId: string }>) =>
            void sessionStore.deleteSession(e.detail.sessionId)}
        ></cmx-agent-session-list>
      </aside>
      <main>
        <div class="titlebar">${win?.title || s.currentSessionId || "新会话"}</div>
        <cmx-agent-error-note .message=${win?.error ?? ""}></cmx-agent-error-note>
        <cmx-agent-message-list
          .events=${win?.events ?? []}
          .sessionId=${s.currentSessionId ?? ""}
          .pending=${Boolean(win?.pending) && !streamingHere}
          .streamingText=${delta}
          .canLoadEarlier=${Boolean(win && win.start > 0 && win.loaded)}
          @cmx-agent-load-earlier=${() =>
            s.currentSessionId && void sessionStore.loadEventsBefore(s.currentSessionId)}
          @cmx-agent-approve=${(
            e: CustomEvent<{
              callId: string;
              approved: boolean;
              all: boolean;
              sessionId: string;
            }>
          ) => {
            void sessionStore
              .resolveApproval(e.detail.callId, e.detail.approved, e.detail.all, e.detail.sessionId)
              .then((ok) => toast(ok ? "已提交审批" : "审批提交失败", ok ? "info" : "error"));
          }}
        ></cmx-agent-message-list>
        <cmx-agent-composer
          .draft=${s.currentSessionId ? (s.draftsBySession[s.currentSessionId] ?? "") : ""}
          .streaming=${streamingHere}
          .policy=${se.policy}
          .models=${se.models}
          .currentModel=${se.currentModel}
          @cmx-agent-send=${(e: CustomEvent<{ text: string }>) => void this.send(e.detail.text)}
          @cmx-agent-draft=${(e: CustomEvent<{ text: string }>) =>
            s.currentSessionId && sessionStore.saveDraft(s.currentSessionId, e.detail.text)}
          @cmx-agent-policy-change=${(e: CustomEvent<{ policy: Policy }>) =>
            void settingsStore
              .setPolicy(e.detail.policy.sandbox, e.detail.policy.approval)
              .then((ok) => ok || toast("策略切换失败", "error"))}
          @cmx-agent-model-change=${(e: CustomEvent<{ model: string }>) =>
            void settingsStore
              .setModel(e.detail.model)
              .then((ok) => ok || toast("模型切换失败", "error"))}
        ></cmx-agent-composer>
      </main>
    `;
  }
}
