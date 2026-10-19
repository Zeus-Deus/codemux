import { beforeEach, describe, expect, it } from "vitest";

import { clampPeekSize, DEFAULT_PEEK_SIZE, MIN_PEEK_SIZE, useBrowserPeekStore } from "./browser-peek-store";

beforeEach(() => {
  useBrowserPeekStore.setState({ openWorkspaceId: null, pluginPeek: null, size: DEFAULT_PEEK_SIZE });
});

it("keeps browser and plugin peeks mutually exclusive and ignores stale status callbacks", () => {
  const store = useBrowserPeekStore.getState();
  store.openPlugin("demo:live", "ws-1");
  const old = useBrowserPeekStore.getState().pluginPeek!;
  store.open("ws-1");
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
  store.openPlugin("demo:live", "ws-1");
  expect(useBrowserPeekStore.getState().openWorkspaceId).toBeNull();
  store.setPluginStatus(old.generation, "error");
  expect(useBrowserPeekStore.getState().pluginPeek?.status).toBe("connecting");
  store.closeAll();
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
});

it("persists shared size without reopening any peek after rehydration", async () => {
  useBrowserPeekStore.getState().setSize({ width: 520, height: 330 });
  useBrowserPeekStore.getState().openPlugin("demo:live", "ws-private");
  const saved = localStorage.getItem("codemux:browser-peek")!;
  expect(JSON.parse(saved).state).toEqual({ size: { width: 520, height: 330 } });
  useBrowserPeekStore.setState({ pluginPeek: null, openWorkspaceId: null, size: DEFAULT_PEEK_SIZE });
  localStorage.setItem("codemux:browser-peek", saved);
  await useBrowserPeekStore.persist.rehydrate();
  expect(useBrowserPeekStore.getState().size).toEqual({ width: 520, height: 330 });
  expect(useBrowserPeekStore.getState().pluginPeek).toBeNull();
  expect(useBrowserPeekStore.getState().openWorkspaceId).toBeNull();
});

describe("clampPeekSize", () => {
  const viewport = { width: 2000, height: 1000 };

  it("keeps a size inside the bounds", () => {
    expect(clampPeekSize({ width: 600, height: 400 }, viewport)).toEqual({ width: 600, height: 400 });
  });

  it("never shrinks below the readable minimum", () => {
    expect(clampPeekSize({ width: 100, height: 50 }, viewport)).toEqual(MIN_PEEK_SIZE);
  });

  it("never grows past 60% of the window", () => {
    expect(clampPeekSize({ width: 5000, height: 5000 }, viewport)).toEqual({ width: 1200, height: 600 });
  });

  it("lets the minimum win on a window too small for both", () => {
    expect(clampPeekSize({ width: 500, height: 500 }, { width: 400, height: 300 })).toEqual(MIN_PEEK_SIZE);
  });
});
