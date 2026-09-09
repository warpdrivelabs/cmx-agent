import { describe, expect, it } from "vitest";
import fixtures from "./protocol.fixtures.json";
import type { AppRequest } from "../../src/protocol/request";
import type { AppResponse } from "../../src/protocol/response";
import type { SessionEventEnvelope } from "../../src/protocol/session-event";
import type { EventsWindow } from "../../src/protocol/events-window";

/** 协议快照：TS 类型结构与 Rust 真实输出对齐（结构级断言，Phase 3 起补全命令覆盖）。 */
describe("协议 fixture 快照", () => {
  it("get_events 响应结构可安全窄化为 EventsWindow", () => {
    const res = fixtures.responses.get_events as AppResponse<EventsWindow>;
    expect(res.ok).toBe(true);
    expect(res.data?.events).toHaveLength(2);
    expect(res.data?.total).toBe(2);
    expect(res.data?.start).toBe(0);
    expect(res.data?.events[0]?.kind).toBe("turn_started");
  });

  it("总线信封结构可安全窄化为 SessionEventEnvelope", () => {
    const env = fixtures.bus_envelope as SessionEventEnvelope;
    expect(env.session_id).toBe("s1");
    expect(env.event.seq).toBe(1);
    expect(env.event.kind).toBe("turn_started");
  });

  it("请求命令名合法（TS 联合编译期保证 + 运行时抽检）", () => {
    const reqs = Object.values(fixtures.requests) as AppRequest[];
    const validCmds = new Set([
      "create_session",
      "send",
      "get_events",
      "list_sessions",
      "delete_session",
      "list_connectors",
      "list_plugins",
      "install_plugin",
      "uninstall_plugin",
      "toggle_plugin",
      "list_models",
      "set_model",
      "set_policy",
      "get_model_config",
      "set_model_config",
      "login",
      "current_user",
      "logout",
      "approve",
      "im_bind_gen_code",
      "im_list_bindings",
      "im_unbind"
    ]);
    for (const req of reqs) {
      expect(validCmds.has(req.cmd)).toBe(true);
    }
  });
});
