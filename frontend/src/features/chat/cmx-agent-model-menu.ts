import { LitElement, html, css, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { settingsStore } from "../../platform/stores/settings-store";
import { toast } from "../common/cmx-agent-toast";
import type { ProviderEntry } from "../../protocol/response";

/**
 * 模型选择弹层（对齐旧 openModelMenu 重制版）：
 * 每 provider 一行（点击整体切换），激活条目下缩进列候选模型，底部「⚙ 配置模型…」。
 */
@customElement("cmx-agent-model-menu")
export class CmxAgentModelMenu extends LitElement {
  /** 锚点（composer 模型按钮视口矩形）；null 时隐藏。 */
  @property({ attribute: false }) anchor: DOMRect | null = null;
  @state() private providers: ProviderEntry[] = [];
  @state() private loading = false;

  static styles = css`
    :host {
      position: fixed;
      min-width: 240px;
      max-height: 60vh;
      overflow: auto;
      background: var(--panel);
      border: 1px solid var(--border2);
      border-radius: 10px;
      padding: 5px;
      box-shadow: 0 8px 28px rgba(0, 0, 0, 0.35);
      z-index: 90;
      display: none;
    }
    :host([open]) {
      display: block;
    }
    .mi {
      display: flex;
      align-items: center;
      gap: 10px;
      padding: 9px 11px;
      border-radius: 7px;
      font-size: 13.5px;
      color: var(--ink2);
      cursor: pointer;
    }
    .mi:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .mm-head {
      font-weight: 600;
    }
    .mm-head .mm-b {
      color: var(--muted);
      font-weight: 400;
      font-size: 11.5px;
    }
    .mm-item {
      padding-left: 26px;
    }
    .mm-item.on {
      color: var(--aqua);
    }
    .mm-ck {
      width: 14px;
      text-align: center;
      color: var(--aqua);
    }
    .mm-cfg {
      margin-top: 4px;
      border-top: 1px solid var(--border);
      padding-top: 6px;
    }
  `;

  async show(): Promise<void> {
    this.loading = true;
    this.providers = await settingsStore.listProviders();
    this.loading = false;
  }

  private pickProvider(id: string): void {
    void settingsStore.setActiveProvider(id).then((r) => {
      if (r.ok) {
        toast(r.note ?? "已切换");
        void this.show(); // 刷新激活标记
        this.dispatchEvent(
          new CustomEvent("cmx-provider-changed", { bubbles: true, composed: true })
        );
      } else {
        toast(r.error ?? "切换失败", "error");
      }
    });
  }

  private pickModel(model: string, pid?: string): void {
    this.dispatchEvent(
      new CustomEvent("cmx-agent-model-change", {
        detail: { model, pid },
        bubbles: true,
        composed: true
      })
    );
  }

  render() {
    return html`
      ${
        this.loading
          ? html`<div class="mi">加载中…</div>`
          : this.providers.map((p) => {
              const on = Boolean(p.active);
              return html`
                <div class="mi mm-head" @click=${() => this.pickProvider(p.id)}>
                  <span>${on ? "✓" : "   "}${p.name ?? p.id}</span>
                  <span class="mm-b">${p.model ?? "—"}${p.configured_key ? "" : " · 未填Key"}</span>
                </div>
                ${
                  on
                    ? (p.candidates ?? []).map(
                        (c) => html`
                          <div
                            class="mi mm-item ${c.model === p.model ? "on" : ""}"
                            @click=${() => this.pickModel(c.model, p.id)}
                          >
                            <span class="mm-ck">${c.model === p.model ? "✓" : ""}</span>
                            ${c.label ?? c.model}
                          </div>
                        `
                      )
                    : nothing
                }
              `;
            })
      }
      <div
        class="mi mm-item mm-cfg"
        @click=${() =>
          this.dispatchEvent(
            new CustomEvent("cmx-agent-open-model-config", { bubbles: true, composed: true })
          )}
      >
        ⚙ 配置模型…
      </div>
    `;
  }
}
