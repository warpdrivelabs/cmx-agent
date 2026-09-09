import { LitElement, html, css } from "lit";
import { customElement, state } from "lit/decorators.js";
import { settingsStore } from "../../platform/stores/settings-store";
import type { Connector } from "../../protocol/response";

function fmtUptime(s?: number): string {
  if (!s) return "?";
  if (s < 3600) return Math.floor(s / 60) + "分";
  if (s < 86400) return Math.floor(s / 3600) + "时";
  return Math.floor(s / 86400) + "天";
}

/** 连接器视图（对齐旧 tpl-connectors + loadConnectors）：tab 内主区卡片网格。 */
@customElement("cmx-agent-connectors-view")
export class CmxAgentConnectorsView extends LitElement {
  @state() private connectors: Connector[] = [];
  @state() private loading = true;

  static styles = css`
    :host {
      flex: 1;
      display: flex;
      flex-direction: column;
      min-height: 0;
    }
    .conn-toolbar {
      display: flex;
      align-items: center;
      gap: 10px;
      padding: 12px 22px;
      border-bottom: 1px solid var(--border);
    }
    .ct {
      font-weight: 600;
      font-size: 14px;
    }
    .cm {
      color: var(--muted);
      font-size: 12px;
    }
    .sp {
      flex: 1;
    }
    .back {
      color: var(--ink2);
      font-size: 13px;
      padding: 5px 9px;
      border-radius: 7px;
      background: transparent;
      border: none;
      cursor: pointer;
      font-family: inherit;
    }
    .back:hover {
      background: var(--hover);
    }
    .connlist {
      flex: 1;
      overflow: auto;
      padding: 22px;
      display: grid;
      grid-template-columns: repeat(auto-fill, minmax(340px, 1fr));
      gap: 16px;
      align-content: start;
    }
    .hint {
      color: var(--muted);
      padding: 8px;
    }
    .conn {
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 14px;
      padding: 16px 18px;
    }
    .conn.offline {
      opacity: 0.62;
    }
    .ch {
      display: flex;
      align-items: center;
      gap: 10px;
      margin-bottom: 8px;
    }
    .ico {
      width: 36px;
      height: 36px;
      border-radius: 9px;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 17px;
      color: var(--ws-white);
      flex: 0 0 36px;
    }
    .conn.flow .ico {
      background: var(--blue);
    }
    .conn.onto .ico {
      background: var(--violet);
    }
    .conn.report .ico {
      background: var(--orange);
    }
    .cn {
      font-weight: 700;
      font-size: 15px;
    }
    .cs {
      font-size: 12px;
      color: var(--muted);
    }
    .dot {
      margin-left: auto;
      display: flex;
      align-items: center;
      gap: 6px;
      font-size: 12px;
      font-weight: 600;
    }
    .dot.on {
      color: var(--aqua);
    }
    .dot.off {
      color: var(--muted);
    }
    .dot .b {
      width: 8px;
      height: 8px;
      border-radius: 50%;
    }
    .dot.on .b {
      background: var(--aqua);
    }
    .dot.off .b {
      background: var(--muted);
    }
    .desc {
      font-size: 13px;
      color: var(--ink2);
      line-height: 1.5;
      margin-bottom: 12px;
    }
    .tools {
      display: flex;
      flex-wrap: wrap;
      gap: 7px;
    }
    .tchip {
      display: inline-flex;
      align-items: center;
      gap: 6px;
      background: var(--panel2);
      border: 1px solid var(--border2);
      border-radius: 8px;
      padding: 6px 11px;
      font-size: 12px;
      color: var(--ink2);
      cursor: pointer;
      font-family: "SF Mono", Menlo, monospace;
    }
    .tchip:hover {
      background: var(--hover);
      color: var(--ink);
      border-color: var(--aqua);
    }
    .offline .tchip {
      cursor: not-allowed;
      opacity: 0.7;
    }
    .meta {
      font-size: 11px;
      color: var(--muted);
      margin-top: 10px;
    }
  `;

  connectedCallback(): void {
    super.connectedCallback();
    void this.load();
  }

  private async load(): Promise<void> {
    this.loading = true;
    this.connectors = await settingsStore.loadConnectors();
    this.loading = false;
    // 同步侧栏 badge（在线/总数，对齐旧 conn-badge）
    const online = this.connectors.filter((c) => c.status?.online).length;
    this.dispatchEvent(
      new CustomEvent("cmx-agent-connectors-count", {
        detail: { online, total: this.connectors.length },
        bubbles: true,
        composed: true
      })
    );
  }

  private runTool(tool: string, online: boolean): void {
    this.dispatchEvent(
      new CustomEvent("cmx-agent-run-connector-tool", {
        detail: { tool, online },
        bubbles: true,
        composed: true
      })
    );
  }

  render() {
    return html`
      <div class="conn-toolbar">
        <span class="ct">专家 · 技能 · 连接器</span>
        <span class="cm">把运行中的 cmx 引擎接成智能体的工具</span>
        <div class="sp"></div>
        <button class="back" type="button" @click=${() => void this.load()}>⟳ 刷新</button>
      </div>
      <div class="connlist">
        ${
          this.loading
            ? html`<div class="hint">正在探测连接器健康…</div>`
            : this.connectors.map((c) => {
                const st = c.status;
                const online = st?.online === true;
                const ico = c.id === "flow" ? "🔀" : c.id === "onto" ? "◈" : "📊";
                const toolChips = (c.tools ?? []).map(
                  (t) => html`
                    <span
                      class="tchip"
                      @click=${() => this.runTool(t, online)}
                      title=${online ? "调用该工具" : "连接器离线"}
                      >🔧 ${t}</span
                    >
                  `
                );
                const statusHtml = online
                  ? html`<span class="dot on"><span class="b"></span>在线</span>`
                  : html`<span class="dot off"><span class="b"></span>离线</span>`;
                const metaHtml = online
                  ? `服务：${st?.service_name ?? c.service} · 运行 ${fmtUptime(st?.uptime_secs)} · 延迟 ${st?.latency_ms ?? "?"}ms`
                  : `服务未启动或不可达：请启动 ${c.service} (${c.base_url})`;
                return html`
                  <div class="conn ${c.id} ${online ? "" : "offline"}">
                    <div class="ch">
                      <div class="ico">${ico}</div>
                      <div>
                        <div class="cn">${c.name}</div>
                        <div class="cs">${c.service} · ${c.base_url}</div>
                      </div>
                      ${statusHtml}
                    </div>
                    <div class="desc">${c.description ?? ""}</div>
                    <div class="tools">${toolChips}</div>
                    <div class="meta">${metaHtml}</div>
                  </div>
                `;
              })
        }
      </div>
    `;
  }
}
