import { isTauri, tauriListen } from "./tauri";
import type { SessionEventEnvelope } from "../../protocol/session-event";

/**
 * 被动实时通道（方案 §7.4）：任意来源（本地 / IM 桥 / 后续 webhook）的会话事件。
 * IM 遥控「飞书发消息实时显示」的关键通路。仅 platform/stores 可 import。
 */
export function onSessionEvent(subscriber: (envelope: SessionEventEnvelope) => void): () => void {
  if (isTauri()) {
    // Tauri：全局 session_event 事件（payload 即 EventEnvelope）
    let unlisten: (() => void) | undefined;
    let closed = false;
    void tauriListen("session_event", (payload) => {
      subscriber(payload as SessionEventEnvelope);
    }).then((un) => {
      if (closed) {
        un();
      } else {
        unlisten = un;
      }
    });
    return () => {
      closed = true;
      unlisten?.();
    };
  }

  // Web：GET /api/subscribe SSE（Web 壳新增路由），EventSource 自带断线重连
  const source = new EventSource("/api/subscribe");
  source.onmessage = (message) => {
    try {
      subscriber(JSON.parse(message.data) as SessionEventEnvelope);
    } catch {
      // 非 JSON 帧（keep-alive）忽略
    }
  };
  return () => source.close();
}
