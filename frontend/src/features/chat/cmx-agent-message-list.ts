import { LitElement, html, css, nothing, type TemplateResult } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { unsafeHTML } from "lit/directives/unsafe-html.js";
import "@ui5/webcomponents/dist/Button.js";
import type { SessionEvent } from "../../protocol/session-event";
import { renderMarkdown } from "./markdown";
import { toast } from "../common/cmx-agent-toast";
import "./cmx-agent-event-renderer";
import "../common/cmx-agent-empty-state";

/** AI 气泡底部操作条按钮（复制/分享/点赞/差评），SVG 对齐旧 ui/index.html TOOL_ACT_BTNS。 */
const ACT_BTNS = html`
  <button
    class="tcact"
    data-act="copy"
    title="复制"
    aria-label="复制"
    @click=${(e: Event) => actCopy(e.currentTarget as HTMLButtonElement)}
  >
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <rect x="9" y="9" width="13" height="13" rx="2" ry="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </svg>
  </button>
  <button
    class="tcact"
    data-act="share"
    title="分享"
    aria-label="分享"
    @click=${(e: Event) => actShare(e.currentTarget as HTMLButtonElement)}
  >
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <circle cx="18" cy="5" r="3" />
      <circle cx="6" cy="12" r="3" />
      <circle cx="18" cy="19" r="3" />
      <line x1="8.59" y1="13.51" x2="15.42" y2="17.49" />
      <line x1="15.41" y1="6.51" x2="8.59" y2="10.49" />
    </svg>
  </button>
  <button
    class="tcact"
    data-act="like"
    title="点赞"
    aria-label="点赞"
    @click=${(e: Event) => actLike(e.currentTarget as HTMLButtonElement)}
  >
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path
        d="M14 9V5a3 3 0 0 0-3-3l-4 9v11h11.28a2 2 0 0 0 2-1.7l1.38-9a2 2 0 0 0-2-2.3zM7 22H4a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2h3"
      />
    </svg>
  </button>
  <button
    class="tcact"
    data-act="dislike"
    title="差评"
    aria-label="差评"
    @click=${(e: Event) => actDislike(e.currentTarget as HTMLButtonElement)}
  >
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
    >
      <path
        d="M10 15v4a3 3 0 0 0 3 3l4-9V2H5.72a2 2 0 0 0-2 1.7l-1.38 9a2 2 0 0 0 2 2.3zm7-13h2.67A2.31 2.31 0 0 1 22 4v7a2.31 2.31 0 0 1-2.33 2H17"
      />
    </svg>
  </button>
`;

function panelText(btn: HTMLElement): string {
  const box = btn.closest(".bubble");
  if (!box) return "";
  const clone = box.cloneNode(true) as HTMLElement;
  clone.querySelectorAll(".tcacts").forEach((a) => a.remove());
  return (clone.textContent ?? "").trim();
}

async function copyText(txt: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(txt);
    return true;
  } catch {
    try {
      const ta = document.createElement("textarea");
      ta.value = txt;
      document.body.appendChild(ta);
      ta.select();
      document.execCommand("copy");
      ta.remove();
      return true;
    } catch {
      return false;
    }
  }
}

async function actCopy(btn: HTMLElement): Promise<void> {
  const ok = await copyText(panelText(btn));
  toast(ok ? "已复制到剪贴板" : "复制失败", ok ? "info" : "error");
}

async function actShare(btn: HTMLElement): Promise<void> {
  const txt = panelText(btn);
  if (navigator.share) {
    try {
      await navigator.share({ text: txt });
      return;
    } catch (e) {
      if (e && (e as Error).name === "AbortError") return;
    }
  }
  const ok = await copyText(txt);
  toast(ok ? "已复制，可粘贴分享" : "分享失败", ok ? "info" : "error");
}

function actLike(btn: HTMLElement): void {
  const box = btn.closest(".bubble");
  const dis = box?.querySelector('[data-act="dislike"]');
  const on = btn.classList.toggle("liked");
  if (on && dis) dis.classList.remove("disliked");
  toast(on ? "已点赞 👍" : "已取消点赞");
}

function actDislike(btn: HTMLElement): void {
  const box = btn.closest(".bubble");
  const lk = box?.querySelector('[data-act="like"]');
  const on = btn.classList.toggle("disliked");
  if (on && lk) lk.classList.remove("liked");
  toast(on ? "已记录反馈，谢谢" : "已取消差评");
}

/**
 * 消息时间线（容器属性进）：DOM/CSS 对齐旧 ui/index.html——
 * .row（AI 行 avatar.a 蓝徽标 + bubble.md）/ .row.user（avatar.u + 蓝底气泡）/
 * AI 气泡底部操作条 / 回合分隔线 meta / 三跳点 typing。
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
  private lastEventCount = 0;

  static styles = css`
    :host {
      flex: 1;
      display: flex;
      flex-direction: column;
      min-height: 0;
      overflow-y: auto;
      padding: 22px;
      gap: 11px;
      background: var(--bg);
    }
    .row {
      display: flex;
      gap: 10px;
    }
    .row.user {
      align-self: flex-end;
      flex-direction: row-reverse;
      max-width: 80%;
    }
    .row:not(.user) {
      align-self: stretch;
    }
    .row:not(.user) .bubble {
      flex: 1 1 auto;
      min-width: 0;
    }
    .avatar {
      width: 30px;
      height: 30px;
      border-radius: 8px;
      flex: 0 0 30px;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 12px;
      font-weight: 700;
      color: var(--ws-white);
    }
    .avatar.u {
      background: var(--magenta);
    }
    .avatar.a {
      background: var(--blue);
    }
    .bubble {
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 12px;
      padding: 10px 14px;
      font-size: 14px;
      line-height: 1.55;
      white-space: pre-wrap;
      word-break: break-word;
      color: var(--ink);
    }
    .bubble.md {
      white-space: normal;
    }
    .bubble.md > :first-child {
      margin-top: 0;
    }
    .bubble.md > :last-child {
      margin-bottom: 0;
    }
    .bubble.md .md-h {
      margin: 14px 0 8px;
      line-height: 1.3;
      font-weight: 700;
    }
    .bubble.md h1.md-h {
      font-size: 20px;
    }
    .bubble.md h2.md-h {
      font-size: 18px;
      border-bottom: 1px solid var(--border);
      padding-bottom: 5px;
    }
    .bubble.md h3.md-h {
      font-size: 16px;
    }
    .bubble.md h4.md-h {
      font-size: 14.5px;
    }
    .bubble.md h5.md-h,
    .bubble.md h6.md-h {
      font-size: 13.5px;
      color: var(--ink2);
    }
    .bubble.md .md-p {
      margin: 8px 0;
    }
    .bubble.md .md-ul,
    .bubble.md .md-ol {
      margin: 8px 0;
      padding-left: 22px;
    }
    .bubble.md .md-ul li,
    .bubble.md .md-ol li {
      margin: 3px 0;
    }
    .bubble.md .md-quote {
      margin: 8px 0;
      padding: 4px 12px;
      border-left: 3px solid var(--aqua);
      color: var(--ink2);
      background: var(--panel2);
      border-radius: 0 8px 8px 0;
    }
    .bubble.md .md-hr {
      border: none;
      border-top: 1px solid var(--border);
      margin: 12px 0;
    }
    .bubble.md code {
      font-family: "SF Mono", Menlo, monospace;
      font-size: 12.5px;
      background: var(--panel2);
      border: 1px solid var(--border);
      border-radius: 5px;
      padding: 1px 5px;
    }
    .bubble.md .md-pre {
      margin: 10px 0;
      background: var(--term-bg, var(--bg));
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 12px 14px;
      overflow: auto;
    }
    .bubble.md .md-pre code {
      background: none;
      border: none;
      padding: 0;
      font-size: 12.5px;
      line-height: 1.5;
      color: var(--ink);
      white-space: pre;
    }
    .bubble.md a {
      color: var(--aqua);
      text-decoration: none;
    }
    .bubble.md a:hover {
      text-decoration: underline;
    }
    .bubble.md .md-table {
      border-collapse: collapse;
      margin: 10px 0;
      font-size: 13px;
      display: block;
      overflow-x: auto;
    }
    .bubble.md .md-table th,
    .bubble.md .md-table td {
      border: 1px solid var(--border2);
      padding: 6px 11px;
      text-align: left;
    }
    .bubble.md .md-table th {
      background: var(--panel2);
      font-weight: 700;
      color: var(--ink);
    }
    .bubble.md .md-table tr:nth-child(even) td {
      background: var(--panel2);
    }
    .row.user .bubble {
      background: var(--blue);
      color: var(--ws-white);
      border-color: transparent;
    }
    .meta {
      font-size: 11px;
      color: var(--muted);
      align-self: center;
      padding: 2px 0;
    }
    .meta.apauto {
      color: var(--gold);
      opacity: 0.85;
    }
    /* AI 文本气泡底部操作行 */
    .tcacts.bubble-acts {
      display: flex;
      gap: 1px;
      margin: 8px -4px -2px;
      padding-top: 2px;
    }
    .tcact {
      display: inline-flex;
      align-items: center;
      justify-content: center;
      width: 24px;
      height: 22px;
      border-radius: 6px;
      color: var(--muted);
      background: transparent;
      border: 1px solid transparent;
      cursor: pointer;
      padding: 0;
      transition:
        color 0.12s,
        background 0.12s;
    }
    .tcact:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .tcact svg {
      display: block;
      width: 15px;
      height: 15px;
    }
    .tcact.liked {
      color: var(--aqua);
    }
    .tcact.disliked {
      color: var(--red);
    }
    /* 打字指示器：AI 气泡里三个跳动圆点 */
    .bubble.typing {
      display: inline-flex;
      align-items: center;
      gap: 5px;
      padding: 13px 16px;
    }
    .bubble.typing .dot {
      width: 7px;
      height: 7px;
      border-radius: 50%;
      background: var(--aqua);
      opacity: 0.4;
      animation: typing 1.2s infinite ease-in-out;
    }
    .bubble.typing .dot:nth-child(2) {
      animation-delay: 0.18s;
    }
    .bubble.typing .dot:nth-child(3) {
      animation-delay: 0.36s;
    }
    @keyframes typing {
      0%,
      60%,
      100% {
        opacity: 0.35;
        transform: translateY(0);
      }
      30% {
        opacity: 1;
        transform: translateY(-4px);
      }
    }
    .load-earlier {
      align-self: center;
      margin: 2px 0 12px;
      padding: 6px 16px;
      border-radius: 999px;
      font-size: 12.5px;
      color: var(--ink2);
      background: var(--panel);
      border: 1px solid var(--border2);
      cursor: pointer;
    }
    .load-earlier:hover {
      border-color: var(--aqua);
      color: var(--aqua);
    }
  `;

  protected updated(changed: Map<string, unknown>): void {
    // 旧版行为：每次事件渲染后滚到底（log.scrollTop=1e9）；加载更早消息时保持位置
    if (changed.has("events") || changed.has("streamingText") || changed.has("pending")) {
      const count = this.events.length;
      if (count !== this.lastEventCount && changed.has("events") && count > this.lastEventCount) {
        const grew = count - this.lastEventCount;
        if (grew === 1 && count > 1) {
          // 追加式新增（历史向前翻页不触发）：贴底
          this.scrollTop = this.scrollHeight;
        } else if (this.lastEventCount === 0 || grew > 1) {
          this.scrollTop = this.scrollHeight; // 首次打开 / 窗口重置：直接到底
        }
      } else if (changed.has("streamingText") || changed.has("pending")) {
        this.scrollTop = this.scrollHeight; // 打字机 / 等待指示器期间持续贴底
      }
      this.lastEventCount = count;
    }
  }

  private aiRow(body: TemplateResult): TemplateResult {
    return html`<div class="row">
      <div class="avatar a">AI</div>
      ${body}
    </div>`;
  }

  private renderEvent(ev: SessionEvent): TemplateResult {
    switch (ev.kind) {
      case "user_message":
        return html`<div class="row user">
          <div class="avatar u">你</div>
          <div class="bubble">${ev.text}</div>
        </div>`;
      case "model_message":
        return ev.text
          ? this.aiRow(
              html`<div class="bubble md">
                ${unsafeHTML(renderMarkdown(ev.text))}
                <div class="tcacts bubble-acts">${ACT_BTNS}</div>
              </div>`
            )
          : html``;
      case "turn_ended":
        return html`<div class="meta">
          — 回合 #${ev.turn} 结束（${ev.reason}，${ev.steps} 步）—
        </div>`;
      case "note":
        return html`<div class="meta">${ev.text}</div>`;
      case "approval_resolved":
        // 旧版把结果写回审批卡；此处以低调审计行呈现（approve all 自动放行同样可见）
        return ev.approved && ev.by.startsWith("auto")
          ? html`<div class="meta apauto">🔓 已自动允许（本对话全部允许）</div>`
          : html`<div class="meta">
              ${ev.approved ? "✓ 已允许" : "✕ 已拒绝"}${ev.by !== "user" ? `（${ev.by}）` : ""}
            </div>`;
      default:
        return html`<cmx-agent-event-renderer
          .event=${ev}
          .sessionId=${this.sessionId}
        ></cmx-agent-event-renderer>`;
    }
  }

  render(): TemplateResult {
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
          ? this.aiRow(
              html`<div class="bubble md">${unsafeHTML(renderMarkdown(this.streamingText))}</div>`
            )
          : this.pending
            ? this.aiRow(
                html`<div class="bubble typing">
                  <span class="dot"></span><span class="dot"></span><span class="dot"></span>
                </div>`
              )
            : nothing
      }
    `;
  }
}
