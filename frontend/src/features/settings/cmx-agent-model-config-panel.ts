import { LitElement, html, css } from "lit";
import { customElement, state } from "lit/decorators.js";
import { StoreController } from "../../platform/store-controller";
import { settingsStore } from "../../platform/stores/settings-store";
import "@ui5/webcomponents/dist/Button.js";
import "@ui5/webcomponents/dist/Input.js";
import "@ui5/webcomponents/dist/Label.js";
import { toast } from "../common/cmx-agent-toast";
import "../common/cmx-agent-panel-card";
import "../common/cmx-agent-loading-block";

/** 模型配置面板（容器→store）：Key 脱敏锁定编辑。 */
@customElement("cmx-agent-model-config-panel")
export class CmxAgentModelConfigPanel extends LitElement {
  private settings = new StoreController(this, settingsStore);
  @state() private apiKey = "";
  @state() private keyUnlocked = false;
  @state() private saving = false;

  static styles = css`
    .field {
      display: flex;
      flex-direction: column;
      gap: var(--cmx-agent-space-xs);
      margin-bottom: var(--cmx-agent-space-md);
    }
    .key-row {
      display: flex;
      gap: var(--cmx-agent-space-sm);
    }
    .key-row ui5-input {
      flex: 1;
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void settingsStore.loadModelConfig();
  }

  private async save(): Promise<void> {
    const cfg = this.settings.state.modelConfig;
    if (!cfg) return;
    this.saving = true;
    const ok = await settingsStore.saveModelConfig(
      cfg,
      this.keyUnlocked && this.apiKey ? this.apiKey : undefined
    );
    this.saving = false;
    if (ok) {
      toast("模型配置已保存并热切换");
      this.apiKey = "";
      this.keyUnlocked = false;
    } else {
      toast(this.settings.state.error ?? "保存失败", "error");
    }
  }

  render() {
    const cfg = this.settings.state.modelConfig;
    return html`
      <cmx-agent-panel-card heading="模型配置">
        <cmx-agent-loading-block .active=${!cfg} text="加载配置…"></cmx-agent-loading-block>
        ${
          cfg
            ? html`
                <div class="field">
                  <ui5-label>Base URL</ui5-label>
                  <ui5-input
                    .value=${cfg.base_url}
                    @input=${(e: InputEvent) => (cfg.base_url = (e.target as HTMLInputElement).value)}
                  ></ui5-input>
                </div>
                <div class="field">
                  <ui5-label>模型</ui5-label>
                  <ui5-input
                    .value=${cfg.model}
                    @input=${(e: InputEvent) => (cfg.model = (e.target as HTMLInputElement).value)}
                  ></ui5-input>
                </div>
                <div class="field">
                  <ui5-label>API Key</ui5-label>
                  <div class="key-row">
                    <ui5-input
                      type="Password"
                      placeholder=${cfg.has_api_key ? "（已配置，脱敏）" : "未配置"}
                      .value=${this.apiKey}
                      ?readonly=${!this.keyUnlocked}
                      @input=${(e: InputEvent) => (this.apiKey = (e.target as HTMLInputElement).value)}
                    ></ui5-input>
                    <ui5-button @click=${() => (this.keyUnlocked = !this.keyUnlocked)}>
                      ${this.keyUnlocked ? "🔒 锁定" : "🔓 解锁"}
                    </ui5-button>
                  </div>
                </div>
                <div class="field">
                  <ui5-label>Temperature</ui5-label>
                  <ui5-input
                    .value=${String(cfg.temperature ?? "")}
                    @input=${(e: InputEvent) => {
                      const v = Number((e.target as HTMLInputElement).value);
                      cfg.temperature = Number.isFinite(v) ? v : undefined;
                    }}
                  ></ui5-input>
                </div>
                <ui5-button
                  slot="actions"
                  design="Emphasized"
                  .disabled=${this.saving}
                  @click=${() => void this.save()}
                >
                  ${this.saving ? "保存中…" : "保存"}
                </ui5-button>
              `
            : ""
        }
      </cmx-agent-panel-card>
    `;
  }
}
