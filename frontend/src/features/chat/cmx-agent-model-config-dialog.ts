import { LitElement, html, css, nothing } from "lit";
import { customElement, property, state } from "lit/decorators.js";
import { settingsStore } from "../../platform/stores/settings-store";
import { toast } from "../common/cmx-agent-toast";
import type { ProviderConfigData, ProviderEntry } from "../../protocol/response";

const MCFG_PRESETS: Record<string, string> = {
  mlamp: "https://llmgw-bz.mlamp.cn/v1",
  deepseek: "https://api.deepseek.com/v1",
  openai: "https://api.openai.com/v1"
};
const MCFG_CANDIDATES: Record<string, string[]> = {
  mlamp: [
    "mlamp/deepseek-v4-flash",
    "mlamp/qwen3-coder-next-fp8",
    "mlamp/deepseek-v4-pro",
    "mlamp/glm-5.2",
    "mlamp/kimi-k3",
    "mlamp/qwen3.8-27b",
    "mlamp/minimax-h3"
  ],
  deepseek: ["deepseek-v4-flash", "deepseek-v4-pro", "deepseek-r1", "deepseek-chat"],
  openai: ["gpt-4o", "gpt-4o-mini", "o1", "o1-mini"]
};

/**
 * 模型配置面板（1:1 对齐旧 mcfg-wide 重制版）：
 * 左列 provider 列表（激活点/内置徽标/＋新增）+ 右侧详情表单（名称/预设/URL/Key 锁/模型/temp/超时）
 * + 底部 删除（内置禁用）/取消/保存。
 */
@customElement("cmx-agent-model-config-dialog")
export class CmxAgentModelConfigDialog extends LitElement {
  /** 反射到 attribute，驱动 :host([open]) 显示遮罩。 */
  @property({ type: Boolean, reflect: true }) open = false;
  @state() private providers: ProviderEntry[] = [];
  @state() private selId: string | null = null;
  @state() private isNew = false;
  @state() private name = "";
  @state() private baseUrl = "";
  @state() private apiKey = "";
  @state() private keyUnlocked = false;
  @state() private model = "";
  @state() private temp = 0.2;
  @state() private timeout = 60000;
  @state() private preset = "";
  @state() private candidates: string[] = [];
  @state() private hint = "";
  @state() private builtin = false;
  @state() private saving = false;

  static styles = css`
    :host {
      position: fixed;
      inset: 0;
      background: rgba(0, 0, 0, 0.45);
      z-index: 200;
      display: none;
      align-items: center;
      justify-content: center;
    }
    :host([open]) {
      display: flex;
    }
    .box {
      background: var(--panel);
      border: 1px solid var(--border2);
      border-radius: 14px;
      padding: 22px 24px 18px;
      width: min(760px, 94vw);
      max-height: 82vh;
      box-shadow: 0 16px 48px rgba(0, 0, 0, 0.5);
      display: flex;
      flex-direction: column;
    }
    .head {
      display: flex;
      align-items: center;
      justify-content: space-between;
      margin-bottom: 16px;
      font-weight: 700;
      font-size: 15px;
    }
    .head button {
      background: none;
      border: none;
      color: var(--muted);
      font-size: 18px;
      cursor: pointer;
      padding: 2px 6px;
      border-radius: 5px;
    }
    .head button:hover {
      background: var(--hover);
      color: var(--ink);
    }
    .body {
      display: flex;
      gap: 16px;
      flex: 1;
      min-height: 0;
      overflow: hidden;
    }
    .list {
      width: 200px;
      flex: 0 0 auto;
      overflow-y: auto;
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 6px;
      display: flex;
      flex-direction: column;
      gap: 4px;
    }
    .list-item {
      padding: 8px 10px;
      border-radius: 8px;
      cursor: pointer;
      font-size: 13px;
      display: flex;
      justify-content: space-between;
      align-items: center;
      gap: 6px;
    }
    .list-item:hover {
      background: var(--hover);
    }
    .list-item.on {
      background: var(--hover);
      color: var(--aqua);
      font-weight: 600;
    }
    .lp-name {
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
    }
    .lp-badge {
      font-size: 10px;
      color: var(--muted);
      border: 1px solid var(--border2);
      border-radius: 4px;
      padding: 1px 4px;
      flex: 0 0 auto;
    }
    .lp-dot {
      width: 6px;
      height: 6px;
      border-radius: 50%;
      background: var(--aqua);
      flex: 0 0 auto;
    }
    .lp-sub {
      font-size: 10.5px;
      color: var(--muted);
      margin-top: 2px;
      overflow: hidden;
      text-overflow: ellipsis;
      white-space: nowrap;
      font-weight: 400;
    }
    .list-new {
      margin-top: 4px;
      border: 1px dashed var(--border2);
      border-radius: 8px;
      text-align: center;
      padding: 7px 0;
      cursor: pointer;
      font-size: 12.5px;
      color: var(--muted);
    }
    .list-new:hover {
      color: var(--aqua);
      border-color: var(--aqua);
    }
    .detail {
      flex: 1;
      min-width: 0;
      overflow-y: auto;
      padding-right: 2px;
    }
    .row {
      display: flex;
      flex-direction: column;
      gap: 4px;
      margin-bottom: 13px;
    }
    .row label {
      font-size: 11.5px;
      color: var(--muted);
      font-weight: 600;
    }
    .row input[type="text"],
    .row input[type="number"],
    .row input[type="password"],
    .row select {
      background: var(--bg);
      border: 1px solid var(--border2);
      border-radius: 8px;
      padding: 7px 10px;
      color: var(--ink);
      font-size: 13px;
      width: 100%;
      box-sizing: border-box;
      outline: none;
      font-family: inherit;
    }
    .row input:focus,
    .row select:focus {
      border-color: var(--aqua);
    }
    .key-wrap {
      display: flex;
      gap: 6px;
      align-items: center;
    }
    .key-wrap input {
      flex: 1;
    }
    .key-wrap button {
      background: var(--bg);
      border: 1px solid var(--border2);
      border-radius: 8px;
      padding: 6px 10px;
      color: var(--muted);
      cursor: pointer;
      font-size: 15px;
      flex: 0 0 auto;
    }
    .key-wrap button:hover {
      color: var(--aqua);
      border-color: var(--aqua);
    }
    .temp-row {
      display: flex;
      align-items: center;
      gap: 10px;
    }
    .temp-row input[type="range"] {
      flex: 1;
      accent-color: var(--aqua);
    }
    .temp-val {
      min-width: 36px;
      text-align: right;
      font-size: 12px;
      color: var(--muted);
      font-variant-numeric: tabular-nums;
    }
    .hint {
      font-size: 11.5px;
      color: var(--muted);
      line-height: 1.5;
      margin-top: 2px;
    }
    .actions {
      display: flex;
      gap: 8px;
      margin-top: 18px;
      padding-top: 14px;
      border-top: 1px solid var(--border);
    }
    .actions .sp {
      flex: 1;
    }
    .actions button {
      padding: 7px 18px;
      border-radius: 8px;
      font-size: 13px;
      cursor: pointer;
      border: 1px solid var(--border2);
      background: var(--bg);
      color: var(--ink);
      font-family: inherit;
    }
    .actions .danger {
      color: var(--red);
      border-color: var(--red);
      background: none;
    }
    .actions .danger:hover {
      background: rgba(229, 72, 77, 0.08);
    }
    .actions .danger:disabled {
      opacity: 0.35;
      cursor: not-allowed;
    }
    .actions .save {
      background: var(--aqua);
      border-color: var(--aqua);
      color: var(--ws-send-ink);
      font-weight: 600;
    }
    .actions .save:hover {
      opacity: 0.88;
    }
  `;

  /** 打开：拉 provider 快照，默认选中激活条目（旧 openModelConfig）。 */
  async openDialog(): Promise<void> {
    this.lockKey();
    this.providers = await settingsStore.listProviders();
    this.selId =
      (this.providers.find((p) => p.active) ?? this.providers[0] ?? { id: "" }).id || null;
    this.isNew = false;
    await this.fillDetail(this.selId);
    this.open = true;
  }

  private close(): void {
    this.open = false;
  }

  private lockKey(): void {
    this.keyUnlocked = false;
    this.apiKey = "";
  }

  /** 右侧详情回填（旧 mcfgFillDetail）：id 空 = 新建态清空表单。 */
  private async fillDetail(id: string | null): Promise<void> {
    this.lockKey();
    if (!id) {
      this.name = "";
      this.baseUrl = "";
      this.apiKey = "";
      this.model = "";
      this.temp = 0.2;
      this.timeout = 60000;
      this.preset = "";
      this.candidates = [];
      this.builtin = false;
      this.hint = "新条目保存后不会自动激活，用上方 ◎ 菜单切换。";
      return;
    }
    const d: ProviderConfigData | null = await settingsStore.getProviderConfig(id);
    if (!d) {
      toast("读取配置失败", "error");
      return;
    }
    this.name = d.name ?? "";
    this.baseUrl = d.base_url ?? "";
    this.apiKey = d.api_key_masked ?? "";
    this.model = d.model ?? "";
    this.temp = typeof d.temperature === "number" ? d.temperature : 0.2;
    this.timeout = d.timeout_ms || 60000;
    this.applyPresetByUrls(d.base_url ?? "");
    this.candidates = d.candidates ?? [];
    this.builtin = Boolean(d.builtin);
    this.hint = d.builtin
      ? "内置预设：可填 Key / 换模型，不可删除。"
      : d.active
        ? "当前激活。"
        : "未激活，保存后用 ◎ 菜单切换。";
  }

  private selectProvider(id: string): void {
    if (id === this.selId && !this.isNew) return;
    this.selId = id;
    this.isNew = false;
    void this.fillDetail(id);
  }

  private newProvider(): void {
    this.isNew = true;
    this.selId = null;
    void this.fillDetail(null);
  }

  private toggleKeyLock(): void {
    if (this.keyUnlocked) {
      this.keyUnlocked = false;
      this.apiKey = ""; // 锁回后清空，防止明文残留
    } else {
      this.keyUnlocked = true;
      this.apiKey = "";
    }
  }

  private applyPreset(key: string): void {
    this.preset = key;
    if (!key) return;
    this.baseUrl = MCFG_PRESETS[key] ?? "";
    this.candidates = MCFG_CANDIDATES[key] ?? [];
  }

  private applyPresetByUrls(url: string): void {
    const match = Object.entries(MCFG_PRESETS).find(([, v]) =>
      url.includes(v.replace(/^https?:\/\//, "").split("/")[0])
    );
    this.preset = match ? match[0] : "";
    if (match) this.candidates = MCFG_CANDIDATES[match[0]] ?? this.candidates;
  }

  private async save(): Promise<void> {
    const name = this.name.trim();
    const baseUrl = this.baseUrl.trim();
    const model = this.model.trim();
    if (this.isNew && !name) {
      toast("名称不能为空", "error");
      return;
    }
    if (!baseUrl) {
      toast("Base URL 不能为空", "error");
      return;
    }
    if (!model) {
      toast("模型名不能为空", "error");
      return;
    }
    this.saving = true;
    const r = await settingsStore.saveProviderConfig({
      id: this.isNew ? undefined : (this.selId ?? undefined),
      name: this.isNew || name ? name : undefined,
      base_url: baseUrl,
      model,
      temperature: this.temp,
      timeout_ms: this.timeout,
      api_key_action: this.keyUnlocked ? "set" : "keep",
      api_key_value: this.keyUnlocked ? this.apiKey.trim() : undefined
    });
    this.saving = false;
    if (r.ok) {
      toast("已保存 · " + (r.note ?? model));
      this.dispatchEvent(
        new CustomEvent("cmx-agent-model-config-saved", { bubbles: true, composed: true })
      );
      // 保存后回到该条目（新建则选中新 id），刷新列表（旧 saveModelConfig 行为）
      this.selId = r.id ?? this.selId;
      this.isNew = false;
      this.providers = await settingsStore.listProviders();
      await this.fillDetail(this.selId);
    } else {
      toast("保存失败：" + (r.error ?? "未知"), "error");
    }
  }

  private async deleteProvider(): Promise<void> {
    if (!this.selId || this.isNew) return;
    const p = this.providers.find((x) => x.id === this.selId);
    if (!p || p.builtin) return;
    const r = await settingsStore.deleteProvider(this.selId);
    if (r.ok) {
      toast(r.note ?? "已删除");
      this.providers = await settingsStore.listProviders();
      this.selId =
        (this.providers.find((x) => x.active) ?? this.providers[0] ?? { id: "" }).id || null;
      await this.fillDetail(this.selId);
    } else {
      toast("删除失败：" + (r.error ?? "未知"), "error");
    }
  }

  render() {
    return html`
      <div class="box" @click=${(e: Event) => e.stopPropagation()}>
        <div class="head">
          ⚙ 模型配置
          <button type="button" title="关闭" @click=${() => this.close()}>✕</button>
        </div>
        <div class="body">
          <div class="list">
            ${this.providers.map((p) => {
              const on = p.id === this.selId && !this.isNew;
              return html`
                <div class="list-item ${on ? "on" : ""}" @click=${() => this.selectProvider(p.id)}>
                  <div style="min-width:0">
                    <div class="lp-name">${p.name ?? p.id}</div>
                    <div class="lp-sub">${p.model ?? "—"}</div>
                  </div>
                  ${p.active ? html`<span class="lp-dot" title="当前激活"></span>` : nothing}
                  ${p.builtin ? html`<span class="lp-badge">内置</span>` : nothing}
                </div>
              `;
            })}
            <div class="list-new" @click=${() => this.newProvider()}>＋ 新增 Provider</div>
          </div>
          <div class="detail">
            <div class="row">
              <label>名称</label>
              <input
                type="text"
                placeholder="如 我的Kimi / 公司网关"
                .value=${this.name}
                @input=${(e: Event) => (this.name = (e.target as HTMLInputElement).value)}
              />
            </div>
            <div class="row">
              <label>预设 Provider（新增时快速填充）</label>
              <select
                .value=${this.preset}
                @change=${(e: Event) => this.applyPreset((e.target as HTMLSelectElement).value)}
              >
                <option value="">自定义</option>
                <option value="mlamp">MLamp（llmgw-bz.mlamp.cn/v1）</option>
                <option value="deepseek">DeepSeek（api.deepseek.com/v1）</option>
                <option value="openai">OpenAI（api.openai.com/v1）</option>
              </select>
            </div>
            <div class="row">
              <label>Base URL</label>
              <input
                type="text"
                placeholder="https://api.xxx.com/v1"
                .value=${this.baseUrl}
                @input=${(e: Event) => (this.baseUrl = (e.target as HTMLInputElement).value)}
              />
            </div>
            <div class="row">
              <label>API Key</label>
              <div class="key-wrap">
                <input
                  type=${this.keyUnlocked ? "text" : "password"}
                  ?readonly=${!this.keyUnlocked}
                  placeholder="（脱敏；点 🔓 解锁编辑）"
                  .value=${this.apiKey}
                  @input=${(e: Event) => (this.apiKey = (e.target as HTMLInputElement).value)}
                />
                <button
                  type="button"
                  title="解锁/锁定 API Key"
                  @click=${() => this.toggleKeyLock()}
                >
                  ${this.keyUnlocked ? "🔓" : "🔒"}
                </button>
              </div>
            </div>
            <div class="row">
              <label>模型</label>
              <input
                type="text"
                list="mcfg-model-list"
                placeholder="如 mlamp/deepseek-v4-flash"
                .value=${this.model}
                @input=${(e: Event) => (this.model = (e.target as HTMLInputElement).value)}
              />
              <datalist id="mcfg-model-list">
                ${this.candidates.map((m) => html`<option value="${m}"></option>`)}
              </datalist>
            </div>
            <div class="row">
              <label>Temperature</label>
              <div class="temp-row">
                <input
                  type="range"
                  min="0"
                  max="1"
                  step="0.01"
                  .value=${String(this.temp)}
                  @input=${(e: Event) => (this.temp = parseFloat((e.target as HTMLInputElement).value))}
                />
                <span class="temp-val">${this.temp.toFixed(2)}</span>
              </div>
            </div>
            <div class="row">
              <label>超时 (ms)</label>
              <input
                type="number"
                min="5000"
                max="300000"
                step="1000"
                .value=${String(this.timeout)}
                @input=${(e: Event) =>
                  (this.timeout = parseInt((e.target as HTMLInputElement).value, 10) || 60000)}
              />
            </div>
            <div class="hint">${this.hint}</div>
          </div>
        </div>
        <div class="actions">
          <button
            class="danger"
            type="button"
            .disabled=${this.builtin || this.isNew || !this.selId}
            @click=${() => void this.deleteProvider()}
          >
            删除
          </button>
          <span class="sp"></span>
          <button type="button" @click=${() => this.close()}>取消</button>
          <button
            class="save"
            type="button"
            .disabled=${this.saving}
            @click=${() => void this.save()}
          >
            ${this.saving ? "保存中…" : "保存"}
          </button>
        </div>
      </div>
    `;
  }
}
