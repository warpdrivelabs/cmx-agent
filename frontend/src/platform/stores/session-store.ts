/**
 * 会话展示态唯一真源（方案 §8.0 / §8.2）：
 * 尾加载窗口 + seq 去重 + viewToken 竞态防护 + 被动通道分流 + 审批恢复。
 * 后端 SessionStore(JSONL) 是持久化真源，本 store 只是展示态缓存。
 */
import { call } from "../bridge/call";
import { sendStream } from "../bridge/stream";
import { onSessionEvent } from "../bridge/session-event";
import type { SessionEvent, SessionEventEnvelope } from "../../protocol/session-event";
import type { EventsWindow, SessionMeta } from "../../protocol/events-window";
import type {
  ApproveData,
  DeleteSessionData,
  GetEventsData,
  ListSessionsData
} from "../../protocol/response";
import type { SelectableStore } from "../store-controller";
import { storeBus } from "./bus";
import { streamStore } from "./stream-store";

/** 打开会话默认只取最近一屏（§8.2 约束 4：不得退化为全量）。 */
const TAIL_LIMIT = 50;
/** 渲染窗口上限：向前翻页累计超过此值提示"加载更早消息"（§8.0 渲染性能）。 */
const RENDER_WINDOW_MAX = 500;

interface SessionWindowState {
  events: SessionEvent[];
  total: number;
  start: number;
  title: string;
  loaded: boolean;
  pending: boolean;
  error: string | null;
}

export interface SessionState {
  sessions: SessionMeta[];
  currentSessionId: string | null;
  windowsBySession: Record<string, SessionWindowState>;
  /** per-session 输入草稿（单会话模型等价性配套 §8.2.6）。 */
  draftsBySession: Record<string, string>;
  /** per-session 滚动位置（切回恢复）。 */
  scrollBySession: Record<string, number>;
  /** 挂起审批（approval_requested 未 resolved），刷新后从事件流重放恢复。 */
  pendingApprovals: Array<{
    callId: string;
    sessionId: string;
    tool: string;
    reason: string;
    seq: number;
  }>;
  /** 非当前会话来新事件的未读标记（§8.2.6；选中即清除）。 */
  unreadIds: ReadonlySet<string>;
}

class SessionStore implements SelectableStore<SessionState> {
  private state: SessionState = {
    sessions: [],
    currentSessionId: null,
    windowsBySession: {},
    draftsBySession: {},
    scrollBySession: {},
    pendingApprovals: [],
    unreadIds: new Set<string>()
  };
  private listeners = new Set<() => void>();
  /** 会话切换令牌：递增，异步响应携带并比对，防止过期响应覆盖新会话（§8.0 竞态 1）。 */
  private viewToken = 0;
  private unsubscribeBus?: () => void;
  private refreshTimer?: number;
  private unsubscribeEvents?: () => void;

  getState(): SessionState {
    return this.state;
  }

  subscribe<TSelected>(
    selector: (state: SessionState) => TSelected,
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

  private set(patch: Partial<SessionState>): void {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  /** 应用启动时调用一次：订阅被动通道 + 登出联动。 */
  start(): void {
    this.unsubscribeEvents = onSessionEvent((envelope) => this.onPassiveEvent(envelope));
    this.unsubscribeBus = storeBus.on("auth:changed", ({ user }) => {
      if (user === null) {
        // 登出：中断展示态（进行中流的 invoke 不可中断，落地前由 viewToken 丢弃）
        this.reset();
      }
    });
  }

  stop(): void {
    this.unsubscribeEvents?.();
    this.unsubscribeBus?.();
  }

  /** 清空展示态（登出复用；测试隔离亦用）。持久化真源在后端。 */
  reset(): void {
    this.viewToken++;
    this.set({
      sessions: [],
      currentSessionId: null,
      windowsBySession: {},
      draftsBySession: {},
      scrollBySession: {},
      pendingApprovals: [],
      unreadIds: new Set<string>()
    });
  }

  async refreshSessions(): Promise<void> {
    const res = await call<ListSessionsData>({ cmd: "list_sessions" });
    if (res.ok && res.data) {
      this.set({ sessions: res.data.sessions });
    }
  }

  /** 切换会话：草稿/滚动保存与恢复 + 尾加载（带 viewToken）。 */
  async selectSession(sessionId: string | null): Promise<void> {
    this.set({ currentSessionId: sessionId });
    storeBus.emit("session:switched", { sessionId });
    if (!sessionId) return;
    if (this.state.unreadIds.size) {
      const next = new Set(this.state.unreadIds);
      next.delete(sessionId);
      this.set({ unreadIds: next });
    }
    const existing = this.state.windowsBySession[sessionId];
    if (existing?.loaded) return; // 已缓存：不重拉（Phase 3 验收 7）
    await this.loadEventsTail(sessionId);
  }

  saveDraft(sessionId: string, text: string): void {
    this.set({ draftsBySession: { ...this.state.draftsBySession, [sessionId]: text } });
  }

  saveScroll(sessionId: string, top: number): void {
    this.set({ scrollBySession: { ...this.state.scrollBySession, [sessionId]: top } });
  }

  /** 尾加载（打开会话 / 断线重连对账共用）。 */
  async loadEventsTail(sessionId: string): Promise<void> {
    const token = ++this.viewToken;
    this.patchWindow(sessionId, { pending: true, error: null });
    const res = await call<GetEventsData>({
      cmd: "get_events",
      session_id: sessionId,
      limit: TAIL_LIMIT
    });
    if (token !== this.viewToken) return; // 过期响应：丢弃（§8.0 竞态 1）
    if (!res.ok || !res.data) {
      this.patchWindow(sessionId, {
        pending: false,
        error: res.error?.message ?? "加载会话失败"
      });
      return;
    }
    this.applyWindow(sessionId, res.data);
    this.rebuildPendingApprovals(sessionId, res.data.events);
  }

  /** 向前翻页（滚动到顶）。 */
  async loadEventsBefore(sessionId: string): Promise<void> {
    const win = this.state.windowsBySession[sessionId];
    if (!win || win.start <= 0 || win.pending) return;
    const token = ++this.viewToken;
    this.patchWindow(sessionId, { pending: true });
    const res = await call<GetEventsData>({
      cmd: "get_events",
      session_id: sessionId,
      limit: TAIL_LIMIT,
      before: win.start
    });
    if (token !== this.viewToken || !res.ok || !res.data) {
      this.patchWindow(sessionId, { pending: false });
      return;
    }
    // 前页 + 当前窗口合并（按 seq 去重排序）
    const merged = dedupeBySeq([...res.data.events, ...win.events]).slice(0, RENDER_WINDOW_MAX);
    this.patchWindow(sessionId, {
      events: merged,
      start: res.data.start,
      total: res.data.total,
      pending: false,
      loaded: true
    });
  }

  /** 发消息：store 内走回合流（组件不直接调 bridge，§7.2）。 */
  async sendMessage(sessionId: string, text: string): Promise<void> {
    streamStore.begin(sessionId);
    let deltaText = "";
    try {
      await sendStream(sessionId, text, (ev) => {
        if (ev.kind === "stream_done" || ev.kind === "stream_error") {
          streamStore.pushDelta(sessionId, "");
          return;
        }
        if (ev.kind === "text_delta") {
          // 打字机增量：回合内累积为当前文本（终态清空），不落库
          deltaText += ev.text;
          streamStore.pushDelta(sessionId, deltaText);
          return;
        }
        this.appendEvent(sessionId, ev);
      });
    } finally {
      streamStore.end(sessionId);
    }
  }

  async createSession(id: string): Promise<SessionMeta | null> {
    const res = await call<SessionMeta>({ cmd: "create_session", id });
    if (res.ok && res.data) {
      await this.refreshSessions();
      return res.data;
    }
    return null;
  }

  async deleteSession(sessionId: string): Promise<boolean> {
    const res = await call<DeleteSessionData>({ cmd: "delete_session", session_id: sessionId });
    if (!res.ok) return false;
    const windows = { ...this.state.windowsBySession };
    delete windows[sessionId];
    const drafts = { ...this.state.draftsBySession };
    delete drafts[sessionId];
    const scrolls = { ...this.state.scrollBySession };
    delete scrolls[sessionId];
    this.set({
      windowsBySession: windows,
      draftsBySession: drafts,
      scrollBySession: scrolls,
      currentSessionId:
        this.state.currentSessionId === sessionId ? null : this.state.currentSessionId
    });
    await this.refreshSessions();
    return true;
  }

  async resolveApproval(
    callId: string,
    approved: boolean,
    all = false,
    sessionId = ""
  ): Promise<boolean> {
    const res = await call<ApproveData>({
      cmd: "approve",
      call_id: callId,
      approved,
      all,
      session_id: sessionId
    });
    if (res.ok) {
      this.set({
        pendingApprovals: this.state.pendingApprovals.filter((a) => a.callId !== callId)
      });
    }
    return res.ok;
  }

  /** 被动通道分流（§7.4：本地流式中的会话跳过，避免双重渲染）。 */
  private onPassiveEvent(envelope: SessionEventEnvelope): void {
    const { session_id: sid, event } = envelope;
    if (streamStore.isStreaming(sid)) return;
    const win = this.state.windowsBySession[sid];
    if (!win?.loaded) {
      // 未打开会话（如 IM 来源新会话）：节流刷新列表摘要 + 标未读（§8.2.6）
      this.scheduleRefresh();
      const next = new Set(this.state.unreadIds);
      next.add(sid);
      this.set({ unreadIds: next });
      return;
    }
    this.appendEvent(sid, event);
    if (sid !== this.state.currentSessionId) {
      const next = new Set(this.state.unreadIds);
      next.add(sid);
      this.set({ unreadIds: next });
      this.scheduleRefresh();
    }
  }

  /** 列表刷新节流（1s 合并，对齐旧 UI scheduleRefreshTasks）。 */
  private scheduleRefresh(): void {
    if (this.refreshTimer) return;
    this.refreshTimer = window.setTimeout(() => {
      this.refreshTimer = undefined;
      void this.refreshSessions();
    }, 1000);
  }

  /** 追加事件（seq 去重 + 审批挂起/解除联动）。 */
  appendEvent(sessionId: string, event: SessionEvent): void {
    const win = this.state.windowsBySession[sessionId];
    if (!win?.loaded) return; // 未打开的会话：列表层摘要处理，不建窗口
    if (win.events.some((e) => e.seq === event.seq)) return; // seq 去重（§8.2 约束 5）
    const events = [...win.events, event].slice(-RENDER_WINDOW_MAX);
    this.patchWindow(sessionId, {
      events,
      total: Math.max(win.total, event.seq),
      loaded: true
    });
    const kind = event.kind;
    if (kind.kind === "approval_requested") {
      this.set({
        pendingApprovals: [
          ...this.state.pendingApprovals.filter((a) => a.callId !== kind.call_id),
          {
            callId: kind.call_id,
            sessionId,
            tool: kind.tool,
            reason: kind.reason,
            seq: event.seq
          }
        ]
      });
    } else if (kind.kind === "approval_resolved") {
      this.set({
        pendingApprovals: this.state.pendingApprovals.filter((a) => a.callId !== kind.call_id)
      });
    }
  }

  /** 断线重连 / 刷新后的审批恢复：扫描窗口内未 resolved 的 approval_requested（§8.0 恢复 5）。 */
  private rebuildPendingApprovals(sessionId: string, events: SessionEvent[]): void {
    const resolved = new Set(
      events.flatMap((e) => (e.kind.kind === "approval_resolved" ? [e.kind.call_id] : []))
    );
    const pending = events.flatMap((e) =>
      e.kind.kind === "approval_requested" && !resolved.has(e.kind.call_id)
        ? [{ kind: e.kind, seq: e.seq }]
        : []
    );
    const others = this.state.pendingApprovals.filter((a) => a.sessionId !== sessionId);
    this.set({
      pendingApprovals: [
        ...others,
        ...pending.map(({ kind: k, seq }) => {
          return {
            callId: k.call_id,
            sessionId,
            tool: k.tool,
            reason: k.reason,
            seq
          };
        })
      ]
    });
  }

  private applyWindow(sessionId: string, data: EventsWindow): void {
    const existing = this.state.windowsBySession[sessionId];
    // 已有窗口（对流式追加过）与拉取窗口按 seq 合并，避免擦除实时事件
    const merged = existing?.loaded
      ? dedupeBySeq([...existing.events, ...data.events])
      : data.events;
    this.patchWindow(sessionId, {
      events: merged,
      total: data.total,
      start: data.start,
      title: data.title,
      loaded: true,
      pending: false,
      error: null
    });
  }

  private patchWindow(sessionId: string, patch: Partial<SessionWindowState>): void {
    const current: SessionWindowState = this.state.windowsBySession[sessionId] ?? {
      events: [],
      total: 0,
      start: 0,
      title: "",
      loaded: false,
      pending: false,
      error: null
    };
    this.set({
      windowsBySession: {
        ...this.state.windowsBySession,
        [sessionId]: { ...current, ...patch }
      }
    });
  }
}

function dedupeBySeq(events: SessionEvent[]): SessionEvent[] {
  const bySeq = new Map<number, SessionEvent>();
  for (const e of events) {
    bySeq.set(e.seq, e);
  }
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

export const sessionStore = new SessionStore();
