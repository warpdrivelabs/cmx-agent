import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import { navigate } from "../../router";
import "./cmx-agent-settings-nav";
import "./cmx-agent-model-config-panel";
import "./cmx-agent-policy-panel";
import "./cmx-agent-im-binding-panel";
import "./cmx-agent-plugin-panel";
import "./cmx-agent-connector-panel";
import "./cmx-agent-about-panel";

/** 设置视图（容器）：hash 子路由 #/settings/:section 切面板。 */
@customElement("cmx-agent-settings-view")
export class CmxAgentSettingsView extends LitElement {
  @property() section = "model";

  static styles = css`
    :host {
      display: flex;
      height: 100%;
      min-height: 0;
      background: var(--cmx-agent-bg-layout);
    }
    nav {
      width: 200px;
      flex: none;
      border-right: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border);
      background: var(--cmx-agent-bg-container);
      padding: var(--cmx-agent-space-md) var(--cmx-agent-space-sm);
    }
    article {
      flex: 1;
      min-width: 0;
      overflow-y: auto;
      padding: var(--cmx-agent-space-lg);
    }
    .panel {
      max-width: 720px;
    }
  `;

  private readonly sections = [
    { id: "model", label: "模型配置" },
    { id: "policy", label: "沙箱与审批" },
    { id: "im", label: "IM 绑定" },
    { id: "plugins", label: "插件" },
    { id: "connectors", label: "连接器" },
    { id: "about", label: "关于" }
  ] as const;

  render() {
    const known = this.sections.some((s) => s.id === this.section);
    const active = known ? this.section : "model";
    return html`
      <nav>
        <cmx-agent-settings-nav
          .items=${this.sections.map((s) => ({ ...s }))}
          .active=${active}
          @cmx-agent-nav=${(e: CustomEvent<{ id: string }>) =>
            navigate(`#/settings/${e.detail.id}`)}
        ></cmx-agent-settings-nav>
      </nav>
      <article>
        <div class="panel">
          ${
            active === "model"
              ? html`<cmx-agent-model-config-panel></cmx-agent-model-config-panel>`
              : active === "policy"
                ? html`<cmx-agent-policy-panel></cmx-agent-policy-panel>`
                : active === "im"
                  ? html`<cmx-agent-im-binding-panel></cmx-agent-im-binding-panel>`
                  : active === "plugins"
                    ? html`<cmx-agent-plugin-panel></cmx-agent-plugin-panel>`
                    : active === "connectors"
                      ? html`<cmx-agent-connector-panel></cmx-agent-connector-panel>`
                      : html`<cmx-agent-about-panel></cmx-agent-about-panel>`
          }
        </div>
      </article>
    `;
  }
}
