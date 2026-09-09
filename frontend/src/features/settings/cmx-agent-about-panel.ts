import { LitElement, html } from "lit";
import { customElement } from "lit/decorators.js";
import { getPlatform, isTauri } from "../../platform/bridge/platform";
import "../common/cmx-agent-panel-card";

@customElement("cmx-agent-about-panel")
export class CmxAgentAboutPanel extends LitElement {
  private platform = isTauri() ? "Tauri 桌面壳" : "Web 桌面壳";

  connectedCallback(): void {
    super.connectedCallback();
    void getPlatform().then((p) => {
      if (p && p !== "web") this.platform = `Tauri 桌面壳（${p}）`;
    });
  }

  render() {
    return html`
      <cmx-agent-panel-card heading="关于">
        <p><strong>cmx 企业桌面智能体</strong> · WebComponents 版</p>
        <p>壳：${this.platform} · 核心：cmx-agent-app（同核多壳）</p>
        <p>前端：Vite + TypeScript + Lit + UI5 WebComponents · 主题：AntD 风格 L2</p>
      </cmx-agent-panel-card>
    `;
  }
}
