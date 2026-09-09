import { beforeEach, describe, expect, it, vi } from "vitest";

const callMock = vi.fn();
const sendStreamMock = vi.fn();
let passiveHandler: ((env: unknown) => void) | null = null;
const onSessionEventMock = vi.fn((cb: (env: unknown) => void) => {
  passiveHandler = cb;
  return () => {
    passiveHandler = null;
  };
});

vi.mock("../../src/platform/bridge/call", () => ({
  call: (req: unknown) => callMock(req)
}));
vi.mock("../../src/platform/bridge/stream", () => ({
  sendStream: (sid: string, text: string, cb: (e: unknown) => void) => sendStreamMock(sid, text, cb)
}));
vi.mock("../../src/platform/bridge/session-event", () => ({
  onSessionEvent: (cb: (env: unknown) => void) => onSessionEventMock(cb)
}));

import { sessionStore } from "../../src/platform/stores/session-store";
import type { SessionEvent } from "../../src/protocol/session-event";
import type { EventsWindow } from "../../src/protocol/events-window";

/** 扁平事件构造（对齐 serde tag="kind" 实际序列化）。 */
function ev(
  seq: number,
  kind: SessionEvent["kind"],
  props: Record<string, unknown> = {}
): SessionEvent {
  return { seq, ts: "2026-09-09T00:00:00Z", kind, ...props } as SessionEvent;
}

function windowOf(events: SessionEvent[], total: number, start: number): EventsWindow {
  return { events, total, start, title: "t" };
}

function okData<T>(data: T): { ok: true; data: T } {
  return { ok: true, data };
}

/** 等待 store 的微任务（await call 的落地）。 */
async function tick(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

describe("session-store 8.0 契约", () => {
  beforeEach(() => {
    callMock.mockReset();
    sendStreamMock.mockReset();
    passiveHandler = null;
    sessionStore.reset();
    sessionStore.stop();
    sessionStore.start();
  });

  it("尾加载：打开会话带 limit，不退化为全量", async () => {
    callMock.mockResolvedValueOnce(okData(windowOf([ev(1, "note", { text: "x" })], 1, 0)));
    await sessionStore.selectSession("s1");
    expect(callMock).toHaveBeenCalledWith(
      expect.objectContaining({ cmd: "get_events", session_id: "s1", limit: 50 })
    );
    const win = sessionStore.getState().windowsBySession["s1"];
    expect(win?.loaded).toBe(true);
    expect(win?.events).toHaveLength(1);
  });

  it("viewToken 竞态：过期响应不覆盖新会话", async () => {
    let resolveS1: (v: unknown) => void = () => undefined;
    callMock.mockImplementationOnce(
      () =>
        new Promise((r) => {
          resolveS1 = r;
        })
    );
    callMock.mockResolvedValueOnce(okData(windowOf([ev(2, "note", { text: "b" })], 1, 0)));
    const p1 = sessionStore.selectSession("s1");
    const p2 = sessionStore.selectSession("s2");
    await tick();
    // s1 的响应在 s2 之后到达：必须被丢弃
    resolveS1(okData(windowOf([ev(9, "note", { text: "a" })], 9, 0)));
    await Promise.all([p1, p2]);
    expect(sessionStore.getState().windowsBySession["s2"]?.events[0]?.seq).toBe(2);
    expect(sessionStore.getState().windowsBySession["s1"]?.loaded).toBeFalsy();
  });

  it("seq 去重：被动通道与主动流重复到达只保留一条", async () => {
    callMock.mockResolvedValueOnce(okData(windowOf([ev(1, "note", { text: "x" })], 1, 0)));
    await sessionStore.selectSession("s1");
    sessionStore.appendEvent("s1", ev(2, "user_message", { text: "hi" }));
    sessionStore.appendEvent("s1", ev(2, "user_message", { text: "hi" }));
    const win = sessionStore.getState().windowsBySession["s1"];
    expect(win?.events.filter((e) => e.seq === 2)).toHaveLength(1);
  });

  it("被动分流：本地流式中的会话跳过（避免双重渲染）", async () => {
    callMock.mockResolvedValueOnce(okData(windowOf([], 0, 0)));
    await sessionStore.selectSession("s1");
    sendStreamMock.mockImplementation(
      async (_sid: string, _text: string, onEvent: (e: unknown) => void) => {
        // 模拟：回合进行中，同一事件经被动总线也到达
        passiveHandler?.({
          session_id: "s1",
          event: ev(5, "user_message", { text: "dup" })
        });
        onEvent(ev(5, "user_message", { text: "dup" }));
      }
    );
    await sessionStore.sendMessage("s1", "dup");
    const win = sessionStore.getState().windowsBySession["s1"];
    expect(win?.events.filter((e) => e.seq === 5)).toHaveLength(1);
  });

  it("审批恢复：尾加载后未 resolved 的 approval_requested 重建挂起", async () => {
    callMock.mockResolvedValueOnce(
      okData(
        windowOf(
          [
            ev(1, "approval_requested", { call_id: "c1", tool: "shell", reason: "r" }),
            ev(2, "approval_resolved", { call_id: "c2", approved: true, by: "me" }),
            ev(3, "approval_requested", { call_id: "c3", tool: "fs_write", reason: "r" })
          ],
          3,
          0
        )
      )
    );
    await sessionStore.selectSession("s1");
    const ids = sessionStore
      .getState()
      .pendingApprovals.map((a) => a.callId)
      .sort();
    expect(ids).toEqual(["c1", "c3"]); // c2 已 resolved 不恢复
  });

  it("草稿：per-session 保存互不干扰", () => {
    sessionStore.saveDraft("s1", "a");
    sessionStore.saveDraft("s2", "b");
    expect(sessionStore.getState().draftsBySession["s1"]).toBe("a");
    expect(sessionStore.getState().draftsBySession["s2"]).toBe("b");
  });
});
