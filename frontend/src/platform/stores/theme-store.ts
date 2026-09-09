import { setTheme } from "../ui5/theme";
import { storeBus } from "./bus";
import type { SelectableStore } from "../store-controller";

export type Tone = "light" | "dark";
export type Skin = "plain" | "none";

export interface ThemeState {
  tone: Tone;
  skin: Skin;
}

const STORAGE_KEY = "cmx-agent.theme";

/**
 * 主题唯一入口（方案 §8.0 / §13.3）：
 * tone 双写——data-cmx-skin-tone 切 seed 块 + UI5 setTheme 换变量底座；
 * skin 只影响 Lit 质感层，不驱动 UI5 主题。
 */
class ThemeStore implements SelectableStore<ThemeState> {
  private state: ThemeState;
  private listeners = new Set<() => void>();

  constructor() {
    this.state = this.readPersisted();
  }

  getState(): ThemeState {
    return this.state;
  }

  subscribe<TSelected>(
    selector: (state: ThemeState) => TSelected,
    cb: (next: TSelected) => void
  ): () => void {
    let last = selector(this.state);
    const wrap = () => {
      const next = selector(this.state);
      if (next !== last) {
        last = next;
        cb(next);
      }
    };
    this.listeners.add(wrap);
    return () => this.listeners.delete(wrap);
  }

  init(): void {
    this.applyToDocument(this.state);
  }

  applyTheme(patch: Partial<ThemeState>): void {
    this.state = { ...this.state, ...patch };
    this.applyToDocument(this.state);
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(this.state));
    } catch {
      // localStorage 不可用（隐私模式等）时静默降级为会话内记忆
    }
    storeBus.emit("theme:changed", { tone: this.state.tone, skin: this.state.skin });
    this.listeners.forEach((l) => l());
  }

  toggleTone(): void {
    this.applyTheme({ tone: this.state.tone === "light" ? "dark" : "light" });
  }

  private readPersisted(): ThemeState {
    try {
      const raw = localStorage.getItem(STORAGE_KEY);
      if (raw) {
        const parsed = JSON.parse(raw) as ThemeState;
        if (parsed.tone === "light" || parsed.tone === "dark") {
          return { tone: parsed.tone, skin: parsed.skin === "none" ? "none" : "plain" };
        }
      }
    } catch {
      // 忽略损坏的持久化数据
    }
    return { tone: "light", skin: "plain" };
  }

  private applyToDocument(state: ThemeState): void {
    const root = document.documentElement;
    root.dataset.cmxSkin = state.skin;
    root.dataset.cmxSkinTone = state.tone;
    // tone 双写：UI5 变量底座 + seed 块（见 13.3 联动机制）
    setTheme(state.tone === "dark" ? "sap_horizon_dark" : "sap_horizon");
  }
}

export const themeStore = new ThemeStore();
