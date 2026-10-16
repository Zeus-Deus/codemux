import { describe, expect, it, vi } from "vitest";
import { registry } from "./registry";

describe("contribution registry", () => {
  it("returns one frozen empty snapshot for an empty area", () => {
    const a = registry.getArea("test.empty");
    expect(a).toEqual([]);
    expect(registry.getArea("test.empty")).toBe(a);
    expect(Object.isFrozen(a)).toBe(true);
  });

  it("orders, replaces by id and notifies only the touched area", () => {
    const touched = vi.fn();
    const other = vi.fn();
    const offA = registry.subscribeArea("test.a", touched);
    const offB = registry.subscribeArea("test.b", other);
    const d1 = registry.register({ id: "p:one", area: "test.a", order: 2 });
    const d2 = registry.register({ id: "p:two", area: "test.a", order: 1 });
    expect(registry.getArea("test.a").map((c) => c.id)).toEqual(["p:two", "p:one"]);
    const snap = registry.getArea("test.a");
    expect(registry.getArea("test.a")).toBe(snap);
    const d3 = registry.register({ id: "p:one", area: "test.a", order: 0, title: "new" });
    expect(registry.getArea("test.a")[0].title).toBe("new");
    expect(touched).toHaveBeenCalledTimes(3);
    expect(other).not.toHaveBeenCalled();
    // The replaced registration's disposer must not remove its replacement.
    d1();
    expect(registry.getArea("test.a").map((c) => c.id)).toEqual(["p:one", "p:two"]);
    d2();
    d3();
    expect(registry.getArea("test.a")).toEqual([]);
    offA();
    offB();
  });

  it("filters enabled:false and throwing when()", () => {
    const spy = vi.spyOn(console, "error").mockImplementation(() => {});
    const d = registry.registerMany([
      { id: "p:a", area: "test.f" },
      { id: "p:b", area: "test.f", enabled: false },
      { id: "p:c", area: "test.f", when: () => { throw new Error("x"); } },
    ]);
    expect(registry.getArea("test.f").map((c) => c.id)).toEqual(["p:a"]);
    d();
    spy.mockRestore();
  });
});
