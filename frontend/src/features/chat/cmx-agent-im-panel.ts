import { LitElement, html, css } from "lit";
import { customElement, state } from "lit/decorators.js";

/** IM 面板（对齐旧 ui/index.html renderIM：Telegram 式会话列表，当前为演示数据）。 */
@customElement("cmx-agent-im-panel")
export class CmxAgentImPanel extends LitElement {
  @state() private keyword = "";

  static styles = css`
    :host {
      display: flex;
      flex-direction: column;
      flex: 1;
      min-height: 0;
    }
    .im-search {
      padding: 8px 0;
    }
    .im-search input {
      width: 100%;
      box-sizing: border-box;
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 9px;
      padding: 8px 11px;
      color: var(--ink);
      font-size: 13px;
      outline: none;
      font-family: inherit;
    }
    .im-search input:focus {
      border-color: var(--aqua);
    }
    .im-list {
      flex: 1;
      overflow: auto;
      margin: 0 -4px;
    }
    .im-item {
      display: flex;
      gap: 10px;
      padding: 9px 8px;
      border-radius: 9px;
      cursor: pointer;
    }
    .im-item:hover {
      background: var(--hover);
    }
    .av {
      width: 40px;
      height: 40px;
      border-radius: 50%;
      flex: 0 0 40px;
      display: flex;
      align-items: center;
      justify-content: center;
      font-size: 19px;
      color: var(--ws-white);
    }
    .mid {
      flex: 1;
      min-width: 0;
    }
    .r1 {
      display: flex;
      align-items: center;
      gap: 6px;
    }
    .nm {
      font-weight: 600;
      font-size: 13.5px;
      flex: 1;
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .tm {
      font-size: 11px;
      color: var(--muted);
      flex: 0 0 auto;
    }
    .r2 {
      display: flex;
      align-items: center;
      gap: 6px;
      margin-top: 2px;
    }
    .msg {
      flex: 1;
      font-size: 12.5px;
      color: var(--muted);
      white-space: nowrap;
      overflow: hidden;
      text-overflow: ellipsis;
    }
    .unread {
      min-width: 18px;
      height: 18px;
      padding: 0 5px;
      border-radius: 9px;
      background: var(--aqua);
      color: var(--ws-white);
      font-size: 11px;
      font-weight: 700;
      display: flex;
      align-items: center;
      justify-content: center;
      flex: 0 0 auto;
    }
  `;

  /** 演示数据（对齐旧 IM_DATA；IM 桥接入后替换为真实绑定会话）。 */
  private readonly data = [
    {
      nm: "财务审批群",
      av: "💰",
      c: "#eb6834",
      msg: "李经理：付款单已提交，请审批",
      tm: "09:42",
      unread: 3
    },
    {
      nm: "cmx-flow 助手",
      av: "🔀",
      c: "#2a78d6",
      msg: "采购付款审批流程已完成",
      tm: "09:15",
      unread: 0
    },
    {
      nm: "运维告警",
      av: "🚨",
      c: "#e34948",
      msg: "onto 服务延迟正常（4ms）",
      tm: "昨天",
      unread: 0
    },
    {
      nm: "张伟",
      av: "张",
      c: "#1baf7a",
      msg: "报表数据我核对过了，没问题",
      tm: "昨天",
      unread: 1
    },
    {
      nm: "产品讨论组",
      av: "💡",
      c: "#9085e9",
      msg: "你：下一版加语音输入",
      tm: "周一",
      unread: 0
    },
    { nm: "本体建模组", av: "◈", c: "#4a3aa7", msg: "新增了订单头对象类型", tm: "周一", unread: 0 }
  ];

  private get filtered() {
    const kw = this.keyword.trim().toLowerCase();
    if (!kw) return this.data;
    return this.data.filter(
      (m) => m.nm.toLowerCase().includes(kw) || m.msg.toLowerCase().includes(kw)
    );
  }

  render() {
    return html`
      <div class="im-search">
        <input
          type="text"
          placeholder="🔍 搜索联系人 / 群组"
          .value=${this.keyword}
          @input=${(e: InputEvent) => (this.keyword = (e.target as HTMLInputElement).value)}
        />
      </div>
      <div class="im-list">
        ${this.filtered.map(
          (m) => html`
            <div class="im-item">
              <div class="av" style="background:${m.c}">${m.av}</div>
              <div class="mid">
                <div class="r1"><span class="nm">${m.nm}</span><span class="tm">${m.tm}</span></div>
                <div class="r2">
                  <span class="msg">${m.msg}</span>
                  ${m.unread ? html`<span class="unread">${m.unread}</span>` : ""}
                </div>
              </div>
            </div>
          `
        )}
      </div>
    `;
  }
}
