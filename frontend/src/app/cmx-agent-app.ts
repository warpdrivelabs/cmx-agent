import { html, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { CmxAgentElement } from "../platform/element";
import { StoreController } from "../platform/store-controller";
import { themeStore } from "../platform/stores/theme-store";
import { authStore } from "../platform/stores/auth-store";
import { navigate, onRoute, type Route } from "../router";
import "../features/auth/login-view";
import "../features/chat/chat-view";
import "../features/settings/settings-view";
import "../features/common/cmx-agent-toast";

/** 应用根组件（无 shadow DOM，全局 token/base.css 生效）：路由分流 + 登录门 + 登出。 */
@customElement("cmx-agent-app")
export class CmxAgentApp extends CmxAgentElement {
  private theme = new StoreController(this, themeStore);
  private auth = new StoreController(this, authStore);
  private offRoute?: () => void;
  @state() private route: Route = { name: "chat" };
  @state() private booted = false;

  protected createRenderRoot(): HTMLElement {
    return this;
  }

  connectedCallback(): void {
    super.connectedCallback();
    void authStore.init().then(() => {
      this.booted = true;
      if (!authStore.getState().user) navigate("#/login");
    });
    this.offRoute = onRoute((route) => {
      this.route = route;
      if (this.booted && !this.auth.state.user && route.name !== "login") {
        navigate("#/login");
      }
    });
  }

  disconnectedCallback(): void {
    this.offRoute?.();
    super.disconnectedCallback();
  }

  private async logout(): Promise<void> {
    await authStore.logout();
    navigate("#/login");
  }

  protected render() {
    const tone = this.theme.state.tone;
    const user = this.auth.state.user;
    const view =
      this.route.name === "login" || !this.booted
        ? html`<cmx-agent-login-view></cmx-agent-login-view>`
        : this.route.name === "settings"
          ? html`<cmx-agent-settings-view
              .section=${this.route.section ?? "model"}
            ></cmx-agent-settings-view>`
          : html`<cmx-agent-chat-view></cmx-agent-chat-view>`;
    return html`
      <div class="app">
        ${
          this.route.name !== "login" && this.booted
            ? html`<header class="app-header">
                <span class="app-title" title="回到对话" @click=${() => navigate("#/chat")}
                  >cmx 企业桌面智能体</span
                >
                <span class="app-actions">
                  <button class="hdr-btn" title="设置" @click=${() => navigate("#/settings")}>
                    ⚙
                  </button>
                  <button
                    class="hdr-btn"
                    title="切换亮暗主题"
                    @click=${() => themeStore.toggleTone()}
                  >
                    ${tone === "light" ? "🌙" : "☀️"}
                  </button>
                  ${
                    user
                      ? html`<span class="user">${user.display_name ?? user.username ?? ""}</span>
                          <button
                            class="hdr-btn"
                            title="退出登录"
                            @click=${() => void this.logout()}
                          >
                            退出
                          </button>`
                      : nothing
                  }
                </span>
              </header>`
            : nothing
        }
        <main class="app-main">${view}</main>
      </div>
      <cmx-agent-toast></cmx-agent-toast>
    `;
  }
}
