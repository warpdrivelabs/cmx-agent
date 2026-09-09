import { isTauri, tauriInvoke } from "./tauri";

/**
 * 平台能力（方案 §7.3）：窗口操作为非协议能力，允许 app 层（titlebar）直接 import。
 * Web 壳下全部 no-op / "web"。
 */
export function getPlatform(): Promise<string> {
  if (!isTauri()) return Promise.resolve("web");
  return tauriInvoke<string>("platform");
}

export { isTauri };

type ResizeDirection =
  "North" | "South" | "East" | "West" | "NorthWest" | "NorthEast" | "SouthWest" | "SouthEast";

interface TauriWindowLike {
  minimize: () => Promise<void>;
  toggleMaximize: () => Promise<void>;
  close: () => Promise<void>;
  startResizeDragging: (direction: ResizeDirection) => Promise<void>;
  startDragging: () => Promise<void>;
}

/** 获取当前窗口句柄（Tauri 2 全局 getCurrentWindow；Web 返回 null）。 */
function currentWindow(): TauriWindowLike | null {
  const w = window as unknown as {
    __TAURI__?: { window?: { getCurrentWindow: () => TauriWindowLike } };
  };
  return w.__TAURI__?.window?.getCurrentWindow() ?? null;
}

export async function windowMinimize(): Promise<void> {
  await currentWindow()?.minimize();
}

export async function windowToggleMaximize(): Promise<void> {
  await currentWindow()?.toggleMaximize();
}

export async function windowClose(): Promise<void> {
  await currentWindow()?.close();
}

/** 8 向缩放热区（对齐现有 data-resize 方向值）。 */
export async function startResize(direction: ResizeDirection): Promise<void> {
  await currentWindow()?.startResizeDragging(direction);
}

/** 标题栏拖动（或直接在元素上用 data-tauri-drag-region 属性）。 */
export async function startDragging(): Promise<void> {
  await currentWindow()?.startDragging();
}
