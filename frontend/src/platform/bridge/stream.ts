import { isTauri, tauriInvoke, tauriListen } from "./tauri";
import type { SessionEvent, StreamTerminator } from "../../protocol/session-event";

/**
 * 回合流通道（方案 §7.2）：per-turn 瞬态，只覆盖"本窗口主动发起的这一回合"。
 * 跨来源被动订阅见 session-event.ts。仅 platform/stores 可 import。
 */
export type StreamEvent = SessionEvent | StreamTerminator;

export async function sendStream(
  sessionId: string,
  text: string,
  onEvent: (event: StreamEvent) => void
): Promise<void> {
  if (isTauri()) {
    const channel = `agentstream-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
    const unlisten = await tauriListen(channel, (payload) => {
      onEvent(payload as StreamEvent);
    });
    try {
      // Tauri 命令参数为 camelCase（serde 自动转换）
      await tauriInvoke("send_stream", { sessionId, text, channel });
    } finally {
      void unlisten();
    }
    return;
  }

  // Web：POST /api/stream，手动解析 SSE 帧（\n\n 分帧，取 data: 行）
  const res = await fetch("/api/stream", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ session_id: sessionId, text })
  });
  if (!res.ok || !res.body) {
    throw new Error(`流式请求失败：HTTP ${res.status}`);
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { value, done } = await reader.read();
    if (done) break;
    buffer += decoder.decode(value, { stream: true });
    let separator: number;
    while ((separator = buffer.indexOf("\n\n")) !== -1) {
      const frame = buffer.slice(0, separator);
      buffer = buffer.slice(separator + 2);
      const dataLine = frame.split("\n").find((line) => line.startsWith("data:"));
      if (dataLine) {
        try {
          onEvent(JSON.parse(dataLine.slice(5).trim()) as StreamEvent);
        } catch {
          // 非 JSON 帧（keep-alive 注释等）忽略
        }
      }
    }
  }
}
