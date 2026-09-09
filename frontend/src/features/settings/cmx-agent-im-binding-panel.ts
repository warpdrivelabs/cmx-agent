import { LitElement, html, css } from "lit";
import { customElement } from "lit/decorators.js";
import { StoreController } from "../../platform/store-controller";
import { settingsStore } from "../../platform/stores/settings-store";
import "@ui5/webcomponents/dist/Button.js";
import { toast } from "../common/cmx-agent-toast";
import "../common/cmx-agent-panel-card";

/** IM 绑定面板（容器→store）。 */
@customElement("cmx-agent-im-binding-panel")
export class CmxAgentImBindingPanel extends LitElement {
  private settings = new StoreController(this, settingsStore);

  static styles = css`
    .code {
      display: flex;
      align-items: center;
      gap: var(--cmx-agent-space-md);
      margin-bottom: var(--cmx-agent-space-lg);
      padding: var(--cmx-agent-space-md);
      background: var(--cmx-agent-bg-layout);
      border-radius: var(--cmx-agent-border-radius);
    }
    .code b {
      font-family: ui-monospace, monospace;
      font-size: var(--cmx-agent-font-size-lg);
      letter-spacing: 2px;
      color: var(--cmx-agent-color-primary);
    }
    .row {
      display: flex;
      align-items: center;
      gap: var(--cmx-agent-space-sm);
      padding: var(--cmx-agent-space-sm) 0;
      border-bottom: var(--cmx-agent-line-width) solid var(--cmx-agent-color-border-secondary);
    }
    .row:last-child {
      border-bottom: none;
    }
    .meta {
      color: var(--cmx-agent-color-text-tertiary);
      font-size: var(--cmx-agent-font-size-sm);
    }
    .spacer {
      flex: 1;
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void settingsStore.loadImBindings();
  }

  render() {
    const s = this.settings.state;
    return html`
      <cmx-agent-panel-card heading="IM 绑定（飞书 / 微信 / 钉钉遥控）">
        <div class="code">
          <ui5-button
            design="Emphasized"
            @click=${() =>
              void settingsStore.genImCode().then((code) => {
                if (code) toast("验证码已生成，请发送给 IM 机器人");
                else toast(s.error ?? "生成失败", "error");
              })}
          >
            生成验证码
          </ui5-button>
          ${s.imCode ? html`把验证码发给机器人完成绑定：<b>${s.imCode}</b>` : ""}
        </div>
        ${
          s.imBindings.length === 0
            ? html`<p>暂无绑定</p>`
            : s.imBindings.map(
                (b) => html`
                  <div class="row">
                    <strong>${b.provider}</strong>
                    <span class="meta">${b.open_id}</span>
                    <span class="spacer"></span>
                    <ui5-button
                      design="Transparent"
                      @click=${() =>
                        void settingsStore.unbindIm(b.provider, b.open_id).then((ok) => {
                          if (ok) toast("已解绑");
                          else toast(s.error ?? "解绑失败", "error");
                        })}
                      >解绑</ui5-button
                    >
                  </div>
                `
              )
        }
      </cmx-agent-panel-card>
    `;
  }
}
