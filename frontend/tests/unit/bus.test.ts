import { describe, expect, it, vi } from "vitest";
import { storeBus } from "../../src/platform/stores/bus";

describe("store 领域事件总线", () => {
  it("订阅者收到事件负载", () => {
    const cb = vi.fn();
    const off = storeBus.on("theme:changed", cb);

    storeBus.emit("theme:changed", { tone: "dark", skin: "plain" });

    expect(cb).toHaveBeenCalledWith({ tone: "dark", skin: "plain" });
    off();
  });

  it("取消订阅后不再收到事件", () => {
    const cb = vi.fn();
    const off = storeBus.on("theme:changed", cb);
    off();

    storeBus.emit("theme:changed", { tone: "light", skin: "none" });

    expect(cb).not.toHaveBeenCalled();
  });

  it("多个订阅者互不影响", () => {
    const cb1 = vi.fn();
    const cb2 = vi.fn();
    const off1 = storeBus.on("theme:changed", cb1);
    storeBus.on("theme:changed", cb2);

    storeBus.emit("theme:changed", { tone: "dark", skin: "plain" });
    off1();
    storeBus.emit("theme:changed", { tone: "light", skin: "plain" });

    expect(cb1).toHaveBeenCalledTimes(1);
    expect(cb2).toHaveBeenCalledTimes(2);
  });
});
