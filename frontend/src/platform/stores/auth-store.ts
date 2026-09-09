import { call } from "../bridge/call";
import type { CurrentUserData, LoginData } from "../../protocol/response";
import { storeBus } from "./bus";
import type { SelectableStore } from "../store-controller";
import type { AuthUser } from "../../protocol/response";

/** 登录三态（方案 §8.0：异步 action 统一 pending / error / data）。 */
export interface AuthState {
  user: AuthUser | null;
  initialized: boolean;
  pending: boolean;
  error: string | null;
}

class AuthStore implements SelectableStore<AuthState> {
  private state: AuthState = {
    user: null,
    initialized: false,
    pending: false,
    error: null
  };
  private listeners = new Set<() => void>();

  getState(): AuthState {
    return this.state;
  }

  subscribe<TSelected>(
    selector: (state: AuthState) => TSelected,
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

  private set(patch: Partial<AuthState>): void {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  /** 启动时恢复会话（路由分流：#/login 或 #/chat）。 */
  async init(): Promise<void> {
    const res = await call<CurrentUserData>({ cmd: "current_user" });
    this.set({
      user: res.ok && res.data ? res.data.user : null,
      initialized: true
    });
    storeBus.emit("auth:changed", { user: this.state.user });
  }

  async login(username: string, password: string): Promise<boolean> {
    this.set({ pending: true, error: null });
    const res = await call<LoginData>({ cmd: "login", username, password });
    if (!res.ok) {
      this.set({
        pending: false,
        error: res.error?.message ?? "登录失败"
      });
      return false;
    }
    this.set({
      pending: false,
      error: null,
      user: res.data?.user ?? null
    });
    storeBus.emit("auth:changed", { user: this.state.user });
    return true;
  }

  async logout(): Promise<void> {
    await call({ cmd: "logout" });
    this.set({ user: null });
    storeBus.emit("auth:changed", { user: null });
  }
}

export const authStore = new AuthStore();
