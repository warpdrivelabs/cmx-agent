import { LitElement, html, css } from "lit";
import { customElement, property } from "lit/decorators.js";
import type { Policy } from "../../protocol/policy";

/** 权限档（对齐旧 index.html perm select：组合值 sandbox/approval，中文文案）。 */
const PERM_OPTIONS = [
  {
    value: "workspace-write/on-request",
    label: "默认权限：工作区可写 · 按需审批"
  },
  { value: "read-only/on-request", label: "只读 · 按需审批" },
  { value: "danger-full-access/never", label: "完全访问 · 从不打断（危险）" }
];

/**
 * composer 下方 selectors 行（对齐旧 .selectors）：🗂 工作空间（占位）+ 🛡 默认权限两旋钮。
 * 变更抛 cmx-agent-policy-change，容器调 settings-store（启动恢复由 store 负责）。
 */
@customElement("cmx-agent-policy-picker")
export class CmxAgentPolicyPicker extends LitElement {
  @property({ attribute: false }) policy: Policy = { sandbox: "read-only", approval: "on-request" };
  @property({ type: Boolean }) disabled = false;

  static styles = css`
    :host {
      display: inline-flex;
      gap: 14px;
    }
    .selbtn {
      display: flex;
      align-items: center;
      gap: 7px;
      padding: 7px 12px;
      border-radius: 9px;
      color: var(--ink2);
      font-size: 13px;
      border: 1px solid transparent;
    }
    .selbtn:hover {
      background: var(--panel2);
      border-color: var(--border);
    }
    select {
      background: none;
      border: none;
      color: var(--ink2);
      font-family: inherit;
      font-size: 13px;
      outline: none;
      cursor: pointer;
    }
    select:disabled {
      cursor: default;
      opacity: 0.6;
    }
    select option {
      background: var(--panel2);
    }
  `;

  private get permValue(): string {
    const match = PERM_OPTIONS.find(
      (o) =>
        o.value.split("/")[0] === this.policy.sandbox &&
        o.value.split("/")[1] === this.policy.approval
    );
    // 未匹配（如 unless-trusted 组合）时显示默认档
    return match ? match.value : PERM_OPTIONS[0].value;
  }

  private pick(v: string): void {
    const [sandbox, approval] = v.split("/");
    if (!sandbox || !approval) return;
    this.dispatchEvent(
      new CustomEvent("cmx-agent-policy-change", {
        detail: { policy: { sandbox, approval } },
        bubbles: true,
        composed: true
      })
    );
  }

  render() {
    return html`
      <div class="selbtn" title="工作空间 = 沙箱工作区根">
        🗂
        <select>
          <option value="default">选择工作空间：默认工作区</option>
          <option value="docs">文档工作区</option>
        </select>
      </div>
      <div class="selbtn" title="默认权限 = 两旋钮：沙箱能力 × 审批许可">
        🛡
        <select
          .disabled=${this.disabled}
          .value=${this.permValue}
          @change=${(e: Event) => this.pick((e.target as HTMLSelectElement).value)}
        >
          ${PERM_OPTIONS.map(
            (o) =>
              html`<option ?selected=${o.value === this.permValue} .value=${o.value}>
                ${o.label}
              </option>`
          )}
        </select>
      </div>
    `;
  }
}
