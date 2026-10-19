import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { Owner, ownedDisposer } from "./lifetime";
import { registry } from "./registry";
import { createPeekHost } from "./peek";
import { PEEK_AREA } from "@/sdk/areas";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";

const workspace = vi.hoisted(() => ({ id: "ws-1" as string | null }));
vi.mock("@/stores/app-store", () => ({
  selectActiveWorkspaceId: () => workspace.id,
  useAppStore: { getState: () => ({}) },
}));

let owner: Owner;
beforeEach(() => {
  owner = new Owner("demo");
  workspace.id = "ws-1";
  useBrowserPeekStore.getState().closeAll();
  owner.track(registry.register({ id: "demo:live", area: PEEK_AREA, render: () => null }));
});
afterEach(() => owner.end());

function api() {
  const bound = owner;
  return createPeekHost(() => bound, (fn, attach) => ownedDisposer(bound, attach(fn)));
}

it("opens registered peeks only for the active workspace and preserves an already open view", () => {
  const peek = api();
  expect(peek.open("missing")).toBe(false);
  expect(peek.open("demo:live", { workspaceId: "ws-2" })).toBe(false);
  useBrowserPeekStore.getState().open("ws-1");
  expect(peek.open("demo:live")).toBe(true);
  const first = useBrowserPeekStore.getState().pluginPeek;
  expect(useBrowserPeekStore.getState().openWorkspaceId).toBeNull();
  expect(peek.isOpen("demo:live")).toBe(true);
  peek.open("demo:live");
  expect(useBrowserPeekStore.getState().pluginPeek).toBe(first);

  workspace.id = "ws-2";
  expect(peek.open("demo:live")).toBe(true);
  expect(useBrowserPeekStore.getState().pluginPeek?.workspaceId).toBe("ws-2");
  expect(useBrowserPeekStore.getState().pluginPeek?.generation).not.toBe(first?.generation);
  workspace.id = null;
  expect(peek.open("demo:live")).toBe(false);
});

it("reports browser replacement and cleans subscriptions and views on unload", () => {
  const peek = api();
  const changed = vi.fn();
  peek.subscribe(changed);
  peek.open("demo:live");
  expect(changed).toHaveBeenLastCalledWith({ id: "demo:live", workspaceId: "ws-1" });
  useBrowserPeekStore.getState().open("ws-1");
  expect(changed).toHaveBeenLastCalledWith(null);
  peek.open("demo:live");
  const calls = changed.mock.calls.length;
  owner.end();
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
  expect(changed).toHaveBeenCalledTimes(calls);
  expect(peek.open("demo:live")).toBe(false);
});

it("toggles only its requested id and closes without an overlay mounted", () => {
  const peek = api();
  expect(peek.toggle("demo:live")).toBe(true);
  peek.close("another:live");
  expect(peek.isOpen("demo:live")).toBe(true);
  expect(peek.toggle("demo:live")).toBe(true);
  expect(peek.isOpen("demo:live")).toBe(false);
  peek.open("demo:live");
  owner.end();
  expect(peek.isOpen("demo:live")).toBe(false);
});

it("does not let an old incarnation close its replacement's view", () => {
  const old = api();
  owner.end();
  owner = new Owner("demo");
  owner.track(registry.register({ id: "demo:live", area: PEEK_AREA, render: () => null }));
  const current = api();
  current.open("demo:live");
  old.close("demo:live");
  expect(current.isOpen("demo:live")).toBe(true);
  expect(old.toggle("demo:live")).toBe(false);
});
