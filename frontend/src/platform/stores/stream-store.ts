/**
 * 回合瞬态（方案 §8.0）：进行中回合的打字机 delta 与活跃标记。
 * 回合结束（stream_done / stream_error）即清空；崩溃恢复不依赖它。
 * 本地流式中的会话经 STREAMING 语义跳过被动通道重复渲染（对齐现有 index.html 行为）。
 */
import type { SelectableStore } from "../store-controller";

export interface StreamState {
  /** 本地正在流式的会话 id 集合（被动通道据此跳过，避免双重渲染）。 */
  streamingSessionIds: ReadonlySet<string>;
  /** 进行中回合的最新 delta 文本（打字机态，rAF 批量提交由 session-store 渲染层消费）。 */
  lastDelta: { sessionId: string; text: string } | null;
}

class StreamStore implements SelectableStore<StreamState> {
  private state: StreamState = {
    streamingSessionIds: new Set(),
    lastDelta: null
  };
  private listeners = new Set<() => void>();

  getState(): StreamState {
    return this.state;
  }

  subscribe<TSelected>(
    selector: (state: StreamState) => TSelected,
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

  private set(patch: Partial<StreamState>): void {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  begin(sessionId: string): void {
    const next = new Set(this.state.streamingSessionIds);
    next.add(sessionId);
    this.set({ streamingSessionIds: next });
  }

  end(sessionId: string): void {
    const next = new Set(this.state.streamingSessionIds);
    next.delete(sessionId);
    this.set({
      streamingSessionIds: next,
      lastDelta: this.state.lastDelta?.sessionId === sessionId ? null : this.state.lastDelta
    });
  }

  isStreaming(sessionId: string): boolean {
    return this.state.streamingSessionIds.has(sessionId);
  }

  pushDelta(sessionId: string, text: string): void {
    this.set({ lastDelta: { sessionId, text } });
  }
}

export const streamStore = new StreamStore();
