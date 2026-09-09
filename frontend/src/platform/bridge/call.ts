import { isTauri, tauriInvoke } from "./tauri";
import type { AppRequest } from "../../protocol/request";
import type { AppResponse } from "../../protocol/response";

/**
 * 统一命令通道（方案 §7.1）。
 * 仅 platform/stores 可 import（eslint no-restricted-imports 守卫）；组件一律走 store action。
 */
export async function call<TData>(req: AppRequest): Promise<AppResponse<TData>> {
  if (isTauri()) {
    const payload = JSON.stringify(req);
    const out = await tauriInvoke<string>("agent", { payload });
    return JSON.parse(out) as AppResponse<TData>;
  }
  const res = await fetch("/api", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(req)
  });
  return (await res.json()) as AppResponse<TData>;
}
