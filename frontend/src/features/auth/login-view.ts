import { LitElement, html, css, nothing } from "lit";
import { customElement, state } from "lit/decorators.js";
import { StoreController } from "../../platform/store-controller";
import { authStore } from "../../platform/stores/auth-store";
import { navigate } from "../../router";

/**
 * 登录页（容器）：完整还原旧 login.html 的「深空主题」视觉——
 * 星云 / 银河 / 闪烁星 / 冲刺星 / 流星动画 + 品牌区 + 深色表单区。
 * 色值全部经 seed.css 品牌 token（本组件零裸值）；表单用原生 input（品牌页视觉深度定制，
 * UI5 控件无法承载该定制——规范豁免点）。登录逻辑走 authStore（统一前门）。
 */
@customElement("cmx-agent-login-view")
export class CmxAgentLoginView extends LitElement {
  private auth = new StoreController(this, authStore);
  @state() private username = "";
  @state() private password = "";

  static styles = css`
    :host {
      display: block;
      width: 100%;
      height: 100%;
      min-height: 620px;
    }
    .login-page {
      width: 100%;
      height: 100vh;
      display: flex;
      overflow: hidden;
      font-family:
        -apple-system, "PingFang SC", "Hiragino Sans GB", "Microsoft YaHei", Arial, sans-serif;
    }

    /* ── 左：品牌区（深空） ── */
    .brand-panel {
      color: var(--cmx-agent-brand-white-95);
      isolation: isolate;
      background: linear-gradient(
        160deg,
        var(--cmx-agent-brand-space-1) 0%,
        var(--cmx-agent-brand-space-2) 45%,
        var(--cmx-agent-brand-space-3) 100%
      );
      flex-direction: column;
      justify-content: space-between;
      width: 54%;
      max-width: 880px;
      padding: 44px 56px;
      display: flex;
      position: relative;
      overflow: hidden;
    }
    .brand-bg {
      z-index: -2;
      position: absolute;
      inset: 0;
      overflow: hidden;
    }
    .space-nebula {
      position: absolute;
      border-radius: 50%;
      filter: blur(64px);
      opacity: 0.5;
      animation: space-nebula-drift 28s ease-in-out infinite alternate;
    }
    .space-nebula--a {
      width: 72%;
      height: 58%;
      top: -12%;
      left: -18%;
      background: radial-gradient(
        circle,
        color-mix(in srgb, var(--cmx-agent-brand-green) 34%, transparent) 0%,
        color-mix(in srgb, var(--cmx-agent-brand-green) 14%, transparent) 42%,
        transparent 72%
      );
    }
    .space-nebula--b {
      width: 58%;
      height: 52%;
      bottom: -18%;
      right: -12%;
      background: radial-gradient(
        circle,
        color-mix(in srgb, var(--cmx-agent-brand-cyan) 26%, transparent) 0%,
        color-mix(in srgb, var(--cmx-agent-brand-green) 12%, transparent) 45%,
        transparent 70%
      );
      animation-delay: -10s;
    }
    .space-nebula--c {
      width: 42%;
      height: 38%;
      top: 38%;
      left: 28%;
      background: radial-gradient(
        circle,
        color-mix(in srgb, var(--cmx-agent-brand-green-bright) 16%, transparent) 0%,
        transparent 68%
      );
      animation-delay: -16s;
    }
    @keyframes space-nebula-drift {
      0% {
        transform: translate(0, 0) scale(1);
      }
      100% {
        transform: translate(4%, 3%) scale(1.07);
      }
    }
    .space-galaxy {
      position: absolute;
      width: 200px;
      height: 200px;
      top: 10%;
      left: 6%;
      border-radius: 50%;
      background:
        radial-gradient(
          ellipse 90% 35% at 50% 50%,
          color-mix(in srgb, var(--cmx-agent-brand-white-95) 10%, transparent) 0%,
          transparent 55%
        ),
        radial-gradient(
          circle at 50% 50%,
          color-mix(in srgb, var(--cmx-agent-brand-green-bright) 14%, transparent) 0%,
          transparent 62%
        );
      filter: blur(1px);
      opacity: 0.75;
      animation: space-galaxy-spin 140s linear infinite;
    }
    @keyframes space-galaxy-spin {
      to {
        transform: rotate(360deg);
      }
    }
    .space-stars,
    .space-rush,
    .space-shooters {
      z-index: -1;
      pointer-events: none;
      position: absolute;
      inset: 0;
      overflow: hidden;
    }
    .space-star {
      position: absolute;
      border-radius: 50%;
      background: var(--cmx-agent-brand-white-95);
      box-shadow: 0 0 4px var(--cmx-agent-brand-star-glow);
      animation: space-star-twinkle ease-in-out infinite;
    }
    @keyframes space-star-twinkle {
      0%,
      100% {
        opacity: var(--star-o-min, 0.25);
      }
      50% {
        opacity: var(--star-o-max, 0.9);
      }
    }
    .space-rush-star {
      position: absolute;
      width: var(--star-size, 2px);
      height: var(--star-size, 2px);
      border-radius: 50%;
      background: radial-gradient(
        circle,
        var(--cmx-agent-brand-white-95) 0%,
        var(--cmx-agent-brand-star-core) 45%,
        transparent 100%
      );
      box-shadow: 0 0 8px var(--cmx-agent-brand-star-glow-strong);
      animation: space-rush-fly linear infinite;
    }
    @keyframes space-rush-fly {
      0% {
        transform: translate(-50%, -50%) scale(0.04);
        opacity: 0;
      }
      10% {
        opacity: 1;
      }
      100% {
        transform: translate(calc(-50% + var(--dx)), calc(-50% + var(--dy)))
          scale(var(--star-scale, 2));
        opacity: 0;
      }
    }
    .space-shooter {
      position: absolute;
      width: 72px;
      height: 1px;
      background: linear-gradient(
        90deg,
        transparent,
        color-mix(in srgb, var(--cmx-agent-brand-white-95) 15%, transparent) 20%,
        color-mix(in srgb, var(--cmx-agent-brand-white-85) 85%, transparent) 55%,
        transparent
      );
      transform: rotate(var(--shoot-angle, -12deg));
      animation: space-shooter-fly linear infinite;
      opacity: 0;
    }
    @keyframes space-shooter-fly {
      0% {
        transform: rotate(var(--shoot-angle, -12deg)) translateX(-24vw);
        opacity: 0;
      }
      6% {
        opacity: 0.75;
      }
      100% {
        transform: rotate(var(--shoot-angle, -12deg)) translateX(130vw);
        opacity: 0;
      }
    }
    @media (prefers-reduced-motion: reduce) {
      .space-nebula,
      .space-galaxy,
      .space-star,
      .space-rush-star,
      .space-shooter {
        animation: none !important;
      }
    }

    .brand-header {
      align-items: center;
      height: 44px;
      display: flex;
      gap: 12px;
    }
    .brand-logo {
      height: 38px;
      width: auto;
      border-radius: 9px;
      flex-shrink: 0;
      animation: 0.8s both fadeInDown;
    }
    .brand-wordmark {
      font-size: 21px;
      font-weight: 700;
      letter-spacing: 0.5px;
      line-height: 1;
      color: var(--cmx-agent-brand-white-95);
      animation: 0.8s 0.05s both fadeInDown;
    }
    .brand-wordmark__accent {
      -webkit-text-fill-color: transparent;
      background: linear-gradient(
        90deg,
        var(--cmx-agent-brand-cyan),
        var(--cmx-agent-brand-green) 60%,
        var(--cmx-agent-brand-green-bright)
      );
      -webkit-background-clip: text;
      background-clip: text;
    }
    .brand-content {
      flex-direction: column;
      flex: 1;
      justify-content: center;
      max-width: 520px;
      margin-top: -32px;
      display: flex;
    }
    .brand-eyebrow {
      letter-spacing: 3px;
      color: color-mix(in srgb, var(--cmx-agent-brand-cyan) 90%, transparent);
      background: color-mix(in srgb, var(--cmx-agent-brand-cyan) 8%, transparent);
      border: 1px solid color-mix(in srgb, var(--cmx-agent-brand-cyan) 20%, transparent);
      border-radius: 2px;
      align-items: center;
      gap: 10px;
      width: fit-content;
      margin-bottom: 26px;
      padding: 6px 14px;
      font-family: "SF Mono", Consolas, monospace;
      font-size: 12px;
      animation: 0.7s 0.1s both fadeInUp;
      display: inline-flex;
    }
    .brand-eyebrow__dot {
      background: var(--cmx-agent-brand-cyan);
      border-radius: 50%;
      width: 6px;
      height: 6px;
      animation: 2s infinite pulse;
      box-shadow: 0 0 8px var(--cmx-agent-brand-cyan);
    }
    @keyframes pulse {
      0%,
      100% {
        opacity: 1;
      }
      50% {
        opacity: 0.3;
      }
    }
    .brand-title {
      letter-spacing: 1px;
      white-space: nowrap;
      margin: 0 0 22px;
      font-size: 52px;
      font-weight: 800;
      line-height: 1.1;
    }
    .brand-title__text {
      animation: 0.8s 0.2s both fadeInUp;
      display: inline-block;
    }
    .brand-title__accent {
      -webkit-text-fill-color: transparent;
      background: linear-gradient(
        90deg,
        var(--cmx-agent-brand-cyan),
        var(--cmx-agent-brand-green) 60%,
        var(--cmx-agent-brand-green-bright)
      );
      -webkit-background-clip: text;
      background-clip: text;
    }
    .brand-desc {
      color: color-mix(in srgb, var(--cmx-agent-brand-white-95) 55%, transparent);
      letter-spacing: 1px;
      margin: 0 0 42px;
      font-size: 15px;
      line-height: 1.9;
      animation: 0.8s 0.5s both fadeInUp;
    }
    .brand-features {
      flex-direction: column;
      gap: 16px;
      margin: 0;
      padding: 0;
      list-style: none;
      display: flex;
    }
    .brand-features li {
      backdrop-filter: blur(8px);
      background: var(--cmx-agent-brand-glass);
      border: 1px solid var(--cmx-agent-brand-glass-border);
      border-left: 2px solid var(--cmx-agent-brand-green);
      border-radius: 2px;
      align-items: center;
      gap: 18px;
      padding: 13px 18px;
      transition: all 0.3s;
      animation: 0.7s both fadeInUp;
      display: flex;
    }
    .brand-features li:nth-child(1) {
      animation-delay: 0.6s;
    }
    .brand-features li:nth-child(2) {
      animation-delay: 0.72s;
    }
    .brand-features li:nth-child(3) {
      animation-delay: 0.84s;
    }
    .brand-features li:hover {
      background: color-mix(in srgb, var(--cmx-agent-brand-green) 8%, transparent);
      border-left-color: var(--cmx-agent-brand-cyan);
      transform: translate(6px);
    }
    .feature-index {
      color: color-mix(in srgb, var(--cmx-agent-brand-green) 70%, transparent);
      font-family: "SF Mono", Consolas, monospace;
      font-size: 18px;
      font-weight: 700;
    }
    .feature-text {
      flex-direction: column;
      gap: 2px;
      display: flex;
    }
    .feature-text strong {
      color: color-mix(in srgb, var(--cmx-agent-brand-white-95) 95%, transparent);
      font-size: 15px;
      font-weight: 600;
    }
    .feature-text em {
      color: color-mix(in srgb, var(--cmx-agent-brand-white-95) 45%, transparent);
      font-size: 13px;
      font-style: normal;
    }
    .brand-footer {
      color: color-mix(in srgb, var(--cmx-agent-brand-white-95) 35%, transparent);
      letter-spacing: 1px;
      align-items: center;
      gap: 8px;
      font-size: 12px;
      animation: 1s 0.9s both fadeIn;
      display: flex;
    }

    /* ── 右：表单区 ── */
    .form-panel {
      color: var(--cmx-agent-brand-text);
      background-color: var(--cmx-agent-brand-form-tint);
      background-image:
        radial-gradient(
          circle at 100% 0,
          color-mix(in srgb, var(--cmx-agent-brand-green) 9%, transparent),
          transparent 52%
        ),
        radial-gradient(
          circle at 0 100%,
          color-mix(in srgb, var(--cmx-agent-brand-cyan) 8%, transparent),
          transparent 48%
        ),
        linear-gradient(
          180deg,
          var(--cmx-agent-brand-form-1) 0%,
          var(--cmx-agent-brand-form-2) 100%
        );
      flex: 1;
      justify-content: center;
      align-items: center;
      padding: 48px;
      display: flex;
      position: relative;
    }
    .form-panel::before {
      content: "";
      position: absolute;
      inset: 0;
      pointer-events: none;
      background-image:
        linear-gradient(
          90deg,
          color-mix(in srgb, var(--cmx-agent-brand-green) 3%, transparent) 1px,
          transparent 1px
        ),
        linear-gradient(
          color-mix(in srgb, var(--cmx-agent-brand-green) 3%, transparent) 1px,
          transparent 1px
        );
      background-size: 40px 40px;
      -webkit-mask-image: radial-gradient(90% 80% at 70% 20%, black 20%, transparent 72%);
      mask-image: radial-gradient(90% 80% at 70% 20%, black 20%, transparent 72%);
      opacity: 0.45;
    }
    .form-wrapper {
      width: 100%;
      max-width: 380px;
      animation: 0.8s 0.3s both fadeInUp;
      position: relative;
      z-index: 1;
    }
    .form-card__head {
      margin-bottom: 34px;
    }
    .form-title {
      letter-spacing: 1px;
      color: var(--cmx-agent-brand-text);
      margin: 0 0 10px;
      font-size: 28px;
      font-weight: 700;
    }
    .form-subtitle {
      color: var(--cmx-agent-brand-text-sub);
      letter-spacing: 0.5px;
      margin: 0;
      font-size: 14px;
    }
    .login-form__item {
      margin-bottom: 20px;
    }
    .login-form label {
      display: block;
      font-size: 13px;
      font-weight: 600;
      margin-bottom: 8px;
      color: var(--cmx-agent-brand-text);
    }
    .login-form input {
      width: 100%;
      height: 44px;
      padding: 0 14px;
      font-size: 14px;
      background: var(--cmx-agent-brand-input-bg);
      border: 1px solid var(--cmx-agent-brand-input-border);
      border-radius: 6px;
      outline: none;
      transition: all 0.25s;
      color: var(--cmx-agent-brand-text);
      font-family: inherit;
    }
    .login-form input::placeholder {
      color: var(--cmx-agent-brand-input-placeholder);
    }
    .login-form input:hover {
      border-color: var(--cmx-agent-brand-green);
      background: var(--cmx-agent-brand-input-bg-hover);
    }
    .login-form input:focus {
      border-color: var(--cmx-agent-brand-green);
      background: var(--cmx-agent-brand-input-bg-hover);
      box-shadow:
        inset 0 0 0 1px var(--cmx-agent-brand-green),
        0 4px 16px color-mix(in srgb, var(--cmx-agent-brand-green) 18%, transparent);
    }
    .login-submit {
      letter-spacing: 8px;
      text-indent: 8px;
      background: linear-gradient(
        135deg,
        var(--cmx-agent-brand-green) 0%,
        var(--cmx-agent-brand-green-bright) 100%
      );
      border: none;
      border-radius: 6px;
      width: 100%;
      height: 48px;
      margin-top: 6px;
      font-size: 16px;
      font-weight: 600;
      color: var(--cmx-agent-brand-white-95);
      cursor: pointer;
      transition: all 0.3s;
      box-shadow: 0 6px 20px color-mix(in srgb, var(--cmx-agent-brand-green) 35%, transparent);
    }
    .login-submit:hover:not(:disabled) {
      background: linear-gradient(
        135deg,
        var(--cmx-agent-brand-green-bright) 0%,
        var(--cmx-agent-brand-green-hover) 100%
      );
      transform: translateY(-1px);
      box-shadow: 0 8px 26px color-mix(in srgb, var(--cmx-agent-brand-green) 50%, transparent);
    }
    .login-submit:active:not(:disabled) {
      transform: translateY(0);
      box-shadow: 0 4px 12px color-mix(in srgb, var(--cmx-agent-brand-green) 30%, transparent);
    }
    .login-submit:disabled {
      opacity: 0.65;
      cursor: default;
    }
    .error {
      margin-top: 16px;
      color: var(--cmx-agent-color-error);
      font-size: 13px;
      min-height: 18px;
    }
    .form-hint {
      margin-top: 22px;
      color: var(--cmx-agent-brand-input-placeholder);
      font-size: 12px;
      line-height: 1.7;
      text-align: center;
    }
    @keyframes fadeIn {
      from {
        opacity: 0;
      }
      to {
        opacity: 1;
      }
    }
    @keyframes fadeInDown {
      from {
        opacity: 0;
        transform: translateY(-12px);
      }
      to {
        opacity: 1;
        transform: translateY(0);
      }
    }
    @keyframes fadeInUp {
      from {
        opacity: 0;
        transform: translateY(20px);
      }
      to {
        opacity: 1;
        transform: translateY(0);
      }
    }
    @media screen and (max-width: 1040px) {
      .brand-panel {
        width: 46%;
        padding: 36px 40px;
      }
      .brand-title {
        font-size: 44px;
      }
      .brand-features {
        display: none;
      }
    }
    @media screen and (max-width: 820px) {
      .login-page {
        flex-direction: column;
        height: auto;
        min-height: 100vh;
      }
      .brand-panel {
        width: 100%;
        max-width: none;
        min-height: 240px;
      }
      .brand-content {
        max-width: none;
        margin-top: 0;
      }
      .brand-title {
        margin-bottom: 14px;
        font-size: 34px;
      }
      .brand-desc {
        margin-bottom: 0;
      }
      .brand-footer {
        display: none;
      }
    }
  `;

  protected firstUpdated(): void {
    this.seedSpaceStars(".space-stars--far", 52, 1, 1.6);
    this.seedSpaceStars(".space-stars--mid", 28, 1.6, 2.8);
    this.seedRushStars();
    this.seedShooters();
  }

  private seedSpaceStars(selector: string, count: number, sizeMin: number, sizeMax: number): void {
    const host = this.renderRoot.querySelector(selector);
    if (!host) return;
    for (let i = 0; i < count; i++) {
      const el = document.createElement("span");
      el.className = "space-star";
      const size = sizeMin + Math.random() * (sizeMax - sizeMin);
      el.style.width = `${size}px`;
      el.style.height = `${size}px`;
      el.style.left = `${Math.random() * 100}%`;
      el.style.top = `${Math.random() * 100}%`;
      el.style.setProperty("--star-o-min", (0.12 + Math.random() * 0.28).toFixed(2));
      el.style.setProperty("--star-o-max", (0.45 + Math.random() * 0.55).toFixed(2));
      el.style.animationDuration = `${2.2 + Math.random() * 4.5}s`;
      el.style.animationDelay = `${-Math.random() * 6}s`;
      host.appendChild(el);
    }
  }

  private seedRushStars(): void {
    const host = this.renderRoot.querySelector(".space-rush");
    if (!host) return;
    for (let i = 0; i < 58; i++) {
      const el = document.createElement("span");
      el.className = "space-rush-star";
      const angle = Math.random() * Math.PI * 2;
      const dist = 32 + Math.random() * 58;
      el.style.left = `${75 + Math.random() * 8 - 4}%`;
      el.style.top = `${45 + Math.random() * 8 - 4}%`;
      el.style.setProperty("--dx", `${Math.cos(angle) * dist}vmax`);
      el.style.setProperty("--dy", `${Math.sin(angle) * dist}vmax`);
      el.style.setProperty("--star-size", `${(1 + Math.random() * 2.2).toFixed(1)}px`);
      el.style.setProperty("--star-scale", (1.4 + Math.random() * 2.8).toFixed(2));
      el.style.animationDuration = `${1.6 + Math.random() * 2.6}s`;
      el.style.animationDelay = `${-Math.random() * 4.5}s`;
      host.appendChild(el);
    }
  }

  private seedShooters(): void {
    const host = this.renderRoot.querySelector(".space-shooters");
    if (!host) return;
    for (let i = 0; i < 7; i++) {
      const el = document.createElement("span");
      el.className = "space-shooter";
      el.style.setProperty("--shoot-angle", `${-18 + Math.random() * 36}deg`);
      el.style.top = `${8 + Math.random() * 72}%`;
      el.style.left = `${-12 + Math.random() * 18}%`;
      el.style.animationDuration = `${3.8 + Math.random() * 5.5}s`;
      el.style.animationDelay = `${-Math.random() * 12}s`;
      host.appendChild(el);
    }
  }

  private async onSubmit(): Promise<void> {
    const ok = await authStore.login(this.username, this.password);
    if (ok) {
      this.password = "";
      navigate("#/chat");
    }
  }

  protected render() {
    const a = this.auth.state;
    const year = new Date().getFullYear();
    return html`
      <div class="login-page">
        <aside class="brand-panel">
          <div class="brand-bg">
            <div class="space-nebula space-nebula--a"></div>
            <div class="space-nebula space-nebula--b"></div>
            <div class="space-nebula space-nebula--c"></div>
            <div class="space-galaxy"></div>
          </div>
          <div class="space-stars space-stars--far" aria-hidden="true"></div>
          <div class="space-stars space-stars--mid" aria-hidden="true"></div>
          <div class="space-rush" aria-hidden="true"></div>
          <div class="space-shooters" aria-hidden="true"></div>
          <header class="brand-header">
            <img src="cmx.png" class="brand-logo" alt="cmx" />
            <span class="brand-wordmark"
              >cmx <span class="brand-wordmark__accent">Agent</span></span
            >
          </header>
          <div class="brand-content">
            <div class="brand-eyebrow">
              <span class="brand-eyebrow__dot"></span> DESKTOP INTELLIGENCE
            </div>
            <h1 class="brand-title">
              <span class="brand-title__text"
                >企业桌面<span class="brand-title__accent">智能体</span></span
              >
            </h1>
            <p class="brand-desc">
              复刻并超越 WorkBuddy · 同核多壳架构<br />
              形（前门）/ 核（回合循环 + 五层守卫）/ 体（cmx 引擎连接器）
            </p>
            <ul class="brand-features">
              <li>
                <span class="feature-index">01</span>
                <div class="feature-text">
                  <strong>本地沙箱守卫</strong>
                  <em>工作区可写 · 高危拦截 · 人在环审批</em>
                </div>
              </li>
              <li>
                <span class="feature-index">02</span>
                <div class="feature-text">
                  <strong>cmx 引擎连接器</strong>
                  <em>flow / onto / report 变成智能体的双手</em>
                </div>
              </li>
              <li>
                <span class="feature-index">03</span>
                <div class="feature-text">
                  <strong>多会话工作台</strong>
                  <em>浏览器式多标签，每任务独立上下文</em>
                </div>
              </li>
            </ul>
          </div>
          <footer class="brand-footer"><span>© ${year} Pansoft · CloudMatrix</span></footer>
        </aside>
        <main class="form-panel">
          <div class="form-wrapper">
            <div class="form-card">
              <div class="form-card__head">
                <h2 class="form-title">欢迎回来</h2>
                <p class="form-subtitle">登录 cmx 企业桌面智能体，开启你的工作</p>
              </div>
              <form
                class="login-form"
                autocomplete="on"
                @submit=${(e: SubmitEvent) => {
                  e.preventDefault();
                  void this.onSubmit();
                }}
              >
                <div class="login-form__item">
                  <label for="username">用户名</label>
                  <input
                    id="username"
                    name="username"
                    type="text"
                    autocomplete="username"
                    placeholder="请输入用户名"
                    required
                    autofocus
                    .value=${this.username}
                    @input=${(e: InputEvent) => (this.username = (e.target as HTMLInputElement).value)}
                  />
                </div>
                <div class="login-form__item">
                  <label for="password">密码</label>
                  <input
                    id="password"
                    name="password"
                    type="password"
                    autocomplete="current-password"
                    placeholder="请输入密码"
                    required
                    .value=${this.password}
                    @input=${(e: InputEvent) => (this.password = (e.target as HTMLInputElement).value)}
                  />
                </div>
                <button class="login-submit" type="submit" .disabled=${a.pending}>
                  ${a.pending ? "登录中…" : "登 录"}
                </button>
                <div class="error" role="alert">${a.error ?? ""}</div>
              </form>
              <div class="form-hint">
                对接 cmx 门户统一认证（/api/auth）· 默认服务 127.0.0.1:8080
              </div>
            </div>
          </div>
        </main>
      </div>
      ${nothing}
    `;
  }
}
