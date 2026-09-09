import { LitElement, html, css } from "lit";
import { customElement, property, state } from "lit/decorators.js";

import type { Policy } from "../../protocol/policy";
import "./cmx-agent-policy-picker";
import "./cmx-agent-model-picker";

/**
 * 输入区（展示+领域事件）：对齐旧 ui/index.html 的 .composer.chat-composer——
 * 深色大输入框 + crow（＋ / @ / ／ 工具位 · ◎Auto 模型 · 🎙 · 绿色圆形↑）+
 * 下方 selectors 行（两旋钮权限，旧版 home .selectors 布局）。
 * 发送/草稿/两旋钮/模型切换全部抛事件，容器接 store。
 */
@customElement("cmx-agent-composer")
export class CmxAgentComposer extends LitElement {
  @property() draft = "";
  @property({ type: Boolean }) streaming = false;
  @property({ type: Boolean }) disabled = false;
  /** home 首屏大输入框（旧 .composer.big：min-height 150px）。 */
  @property({ type: Boolean, reflect: true }) big = false;
  @property({ attribute: false }) policy: Policy = { sandbox: "read-only", approval: "on-request" };
  @property({ attribute: false }) models: string[] = [];
  @property() currentModel = "";
  @state() private text = "";

  static styles = css`
    :host {
      display: block;
      width: 100%;
    }
    .composer {
      width: 100%;
      max-width: 820px;
      background: var(--panel2);
      border: 1px solid var(--border2);
      border-radius: 16px;
      padding: 16px 18px;
    }
    .composer.big {
      min-height: 150px;
      display: flex;
      flex-direction: column;
    }
    /* 会话态外框（旧 .chatfoot）：上分隔线 + 输入框加宽居中 */
    .chatfoot {
      padding: 14px 26px 20px;
      border-top: 1px solid var(--border);
      background: var(--bg);
    }
    .chatfoot .composer {
      max-width: 900px;
      margin: 0 auto;
    }
    /* 会话输入框：更大、更专业 */
    .chat-composer {
      padding: 14px 16px 12px;
      border-radius: 18px;
      transition:
        border-color 0.15s,
        box-shadow 0.15s;
    }
    .chat-composer:focus-within {
      border-color: var(--aqua);
      box-shadow: 0 0 0 3px rgba(31, 177, 130, 0.18);
    }
    textarea {
      width: 100%;
      box-sizing: border-box;
      background: none;
      border: none;
      outline: none;
      color: var(--ink);
      font-size: 15px;
      font-family: inherit;
      line-height: 1.55;
      resize: none;
      min-height: 26px;
      max-height: 200px;
      overflow-y: auto;
    }
    .composer.big textarea {
      line-height: 1.5;
      flex: 1;
      min-height: 64px;
    }
    textarea::placeholder {
      color: var(--muted);
    }
    .crow {
      display: flex;
      align-items: center;
      height: 32px;
      margin-top: 8px;
      padding-top: 8px;
      gap: 6px;
      border-top: 1px solid var(--border);
      box-sizing: content-box;
    }
    .composer.big .crow {
      border-top: none;
      margin-top: 10px;
      gap: 10px;
      height: auto;
      padding-top: 0;
    }
    .sp {
      flex: 1;
    }
    .tool-ic {
      display: flex;
      align-items: center;
      justify-content: center;
      width: 28px;
      height: 28px;
      border-radius: 8px;
      color: var(--ink2);
      font-size: 14px;
      background: transparent;
      border: 1px solid transparent;
      cursor: pointer;
      padding: 0;
      font-family: inherit;
    }
    .tool-ic:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .send {
      width: 30px;
      height: 30px;
      border-radius: 9px;
      background: var(--aqua);
      color: var(--ws-white);
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 15px;
      cursor: pointer;
      border: none;
      padding: 0;
      font-family: inherit;
    }
    .send:hover:not(:disabled) {
      filter: brightness(1.1);
    }
    .send:disabled {
      opacity: 0.35;
      cursor: default;
    }
    /* 两旋钮行（旧 .selectors 布局）：仅 home 首屏显示 */
    .selectors {
      display: flex;
      gap: 14px;
      margin-top: 12px;
      max-width: 820px;
      width: 100%;
    }
    :host(:not([big])) .selectors {
      display: none;
    }
  `;

  protected updated(changed: Map<string, unknown>): void {
    if (changed.has("draft") && !this.text) {
      this.text = this.draft; // 切回会话恢复草稿（一次）
    }
  }

  /** 外部预填（对齐旧 QUICK 点击行为：设置文本并聚焦，不直接发送）。 */
  setText(value: string): void {
    this.text = value;
    const ta = this.shadowRoot?.querySelector("textarea");
    if (ta) {
      ta.style.height = "auto";
      ta.style.height = `${Math.min(ta.scrollHeight, 200)}px`;
    }
    this.dispatchEvent(
      new CustomEvent("cmx-agent-draft", { detail: { text: value }, bubbles: true, composed: true })
    );
  }

  override focus(options?: FocusOptions): void {
    super.focus(options);
    this.shadowRoot?.querySelector("textarea")?.focus();
  }

  private send(): void {
    const text = this.text.trim();
    if (!text || this.streaming || this.disabled) return;
    this.text = "";
    this.dispatchEvent(
      new CustomEvent("cmx-agent-send", { detail: { text }, bubbles: true, composed: true })
    );
  }

  private sync(value: string): void {
    this.text = value;
    this.dispatchEvent(
      new CustomEvent("cmx-agent-draft", { detail: { text: value }, bubbles: true, composed: true })
    );
  }

  render() {
    return html`
      ${
        this.big
          ? html`${this.composerTemplate()}${this.selectorsTemplate()}`
          : html`<div class="chatfoot">${this.composerTemplate()}</div>`
      }
    `;
  }

  private composerTemplate() {
    return html`
      <div class="composer chat-composer ${this.big ? "big" : ""}">
        <textarea
          .value=${this.text}
          placeholder=${
            this.big
              ? "今天帮你做些什么？  @ 引用文件 · / 调用技能 · ⏎ 发送 · ⇧⏎ 换行"
              : "继续对话…  @ 引用文件 · / 调用技能 · ⏎ 发送 · ⇧⏎ 换行"
          }
          @input=${(e: InputEvent) => {
            this.sync((e.target as HTMLTextAreaElement).value);
            const el = e.target as HTMLTextAreaElement;
            el.style.height = "auto";
            el.style.height = `${Math.min(el.scrollHeight, 200)}px`;
          }}
          @keydown=${(e: KeyboardEvent) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              this.send();
            }
          }}
        ></textarea>
        <div class="crow">
          <button class="tool-ic" title="附件（占位）" type="button"><span>＋</span></button>
          <button class="tool-ic" title="引用文件（占位）" type="button"><span>@</span></button>
          <button class="tool-ic" title="调用技能（占位）" type="button"><span>/</span></button>
          <div class="sp"></div>
          <cmx-agent-model-picker
            .models=${this.models}
            .current=${this.currentModel}
            .disabled=${this.streaming}
            @cmx-agent-open-model-config=${() =>
              this.dispatchEvent(
                new CustomEvent("cmx-agent-open-model-config", {
                  bubbles: true,
                  composed: true
                })
              )}
            @cmx-agent-model-change=${(e: CustomEvent<{ model: string }>) =>
              this.dispatchEvent(
                new CustomEvent("cmx-agent-model-change", {
                  detail: e.detail,
                  bubbles: true,
                  composed: true
                })
              )}
          ></cmx-agent-model-picker>
          <button class="tool-ic" title="语音输入（占位）" type="button"><span>🎙</span></button>
          <button
            class="send"
            type="button"
            title=${this.streaming ? "生成中…" : "发送 ⏎"}
            .disabled=${this.streaming || this.disabled || !this.text.trim()}
            @click=${() => this.send()}
          >
            ↑
          </button>
        </div>
      </div>
    `;
  }

  private selectorsTemplate() {
    return html`
      <div class="selectors">
        <cmx-agent-policy-picker
          .policy=${this.policy}
          .disabled=${this.streaming}
          @cmx-agent-policy-change=${(e: CustomEvent<{ policy: Policy }>) =>
            this.dispatchEvent(
              new CustomEvent("cmx-agent-policy-change", {
                detail: e.detail,
                bubbles: true,
                composed: true
              })
            )}
        ></cmx-agent-policy-picker>
      </div>
    `;
  }
}
