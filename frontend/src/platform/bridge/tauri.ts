/**
 * Tauri 全局对象唯一访问点（方案 §7.1）：
 * - 沿用 withGlobalTauri，不引入 @tauri-apps/api；
 * - 全仓只有本文件可触碰 window.__TAURI__（eslint no-restricted-globals 守卫）；
 * - Web 壳下必须先做存在性检测。
 */

interface TauriCore {
  invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T>;
}

interface TauriEventApi {
  listen: (event: string, handler: (payload: { payload: unknown }) => void) => Promise<() => void>;
}

interface TauriGlobal {
  core: TauriCore;
  event?: TauriEventApi;
}

declare global {
  interface Window {
    __TAURI__?: TauriGlobal;
  }
}

export function isTauri(): boolean {
  return typeof window !== "undefined" && window.__TAURI__ !== undefined;
}

/** invoke 封装：仅本文件与 bridge 协议文件可使用。 */
export function tauriInvoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const tauri = window.__TAURI__;
  if (!tauri?.core) {
    return Promise.reject(new Error(`Tauri 不可用，无法调用 ${cmd}`));
  }
  return tauri.core.invoke<T>(cmd, args);
}

/** 事件订阅封装：返回取消订阅函数。 */
export function tauriListen(
  event: string,
  handler: (payload: unknown) => void
): Promise<() => void> {
  const tauri = window.__TAURI__;
  if (!tauri?.event) {
    return Promise.resolve(() => undefined);
  }
  return tauri.event.listen(event, (e) => handler(e.payload));
}
