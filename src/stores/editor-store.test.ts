import { describe, it, expect, beforeEach, afterEach, vi } from "vitest";

import { useEditorStore } from "./editor-store";

const TAB = "right-panel:ws-1:/work/codemux/docs/INDEX.md";

function tab() {
  return useEditorStore.getState().getTab(TAB);
}

beforeEach(() => {
  useEditorStore.setState({ tabs: {} });
});

describe("editor store dirty flags", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  function spyOnStorageWrites() {
    const storage = useEditorStore.persist.getOptions().storage;
    if (!storage) throw new Error("Editor persistence is unavailable");
    return vi.spyOn(storage, "setItem");
  }

  it.each([false, true])("skips repeated dirty=%s updates without persisting or notifying", (dirty) => {
    const { initTab, setBaselineContent, requestReveal, setDirty } = useEditorStore.getState();
    initTab(TAB, { filePath: "/work/codemux/docs/INDEX.md" });
    setBaselineContent(TAB, "saved content");
    requestReveal(TAB, 41, 2, 5);
    setDirty(TAB, dirty);
    const before = useEditorStore.getState();
    const writes = spyOnStorageWrites();
    const listener = vi.fn();
    const unsubscribe = useEditorStore.subscribe(listener);

    try {
      for (let i = 0; i < 100; i++) setDirty(TAB, dirty);

      expect.soft(writes.mock.calls.length).toBe(0);
      expect.soft(listener.mock.calls.length).toBe(0);
      expect(useEditorStore.getState()).toBe(before);
    } finally {
      unsubscribe();
    }
  });

  it("persists and notifies once per dirty transition while preserving tab contents", () => {
    const { initTab, setBaselineContent, requestReveal, setDirty } = useEditorStore.getState();
    initTab(TAB, { filePath: "/work/codemux/docs/INDEX.md" });
    initTab("other");
    setBaselineContent(TAB, "saved content");
    requestReveal(TAB, 41, 2, 5);
    const before = useEditorStore.getState();
    const writes = spyOnStorageWrites();
    const listener = vi.fn();
    const unsubscribe = useEditorStore.subscribe(listener);

    try {
      for (const [index, dirty] of [true, false].entries()) {
        setDirty(TAB, dirty);

        expect(tab()).toEqual({ ...before.tabs[TAB], isDirty: dirty });
        expect(useEditorStore.getState().tabs.other).toBe(before.tabs.other);
        expect(writes).toHaveBeenCalledTimes(index + 1);
        expect(listener).toHaveBeenCalledTimes(index + 1);
      }
      expect(localStorage.getItem("codemux-editor")).toBe(JSON.stringify({
        state: { tabs: {
          [TAB]: { filePath: "/work/codemux/docs/INDEX.md", baselineContent: "", isDirty: false },
          other: { filePath: null, baselineContent: "", isDirty: false },
        } },
        version: 0,
      }));
    } finally {
      unsubscribe();
    }
  });

  it.each([false, true])("creates a missing tab with defaults for dirty=%s", (dirty) => {
    const writes = spyOnStorageWrites();
    const listener = vi.fn();
    const unsubscribe = useEditorStore.subscribe(listener);

    try {
      useEditorStore.getState().setDirty(TAB, dirty);

      expect(tab()).toEqual({ filePath: null, baselineContent: "", isDirty: dirty });
      expect(writes).toHaveBeenCalledOnce();
      expect(listener).toHaveBeenCalledOnce();
    } finally {
      unsubscribe();
    }
  });
});

describe("editor store reveal requests", () => {
  it("bumps the nonce so the same citation clicked twice re-centres", () => {
    const { requestReveal } = useEditorStore.getState();
    requestReveal(TAB, 41);
    expect(tab()?.revealRequest).toEqual({ line: 41, nonce: 1 });

    requestReveal(TAB, 41);
    expect(tab()?.revealRequest).toEqual({ line: 41, nonce: 2 });
  });

  it("normalizes fractional and out-of-range positions", () => {
    useEditorStore.getState().requestReveal(TAB, 0.4, 0);
    expect(tab()?.revealRequest).toEqual({ line: 1, column: 1, nonce: 1 });
  });

  it("carries an end column only when it closes a non-empty range", () => {
    const { requestReveal } = useEditorStore.getState();
    requestReveal(TAB, 3, 5, 11);
    expect(tab()?.revealRequest).toEqual({ line: 3, column: 5, endColumn: 11, nonce: 1 });

    requestReveal(TAB, 3, 5, 5);
    expect(tab()?.revealRequest).toEqual({ line: 3, column: 5, nonce: 2 });
  });

  it("consumes the applied request so a remount cannot replay it", () => {
    const { requestReveal, clearReveal } = useEditorStore.getState();
    requestReveal(TAB, 42);
    clearReveal(TAB, 1);
    expect(tab()?.revealRequest).toBeUndefined();
    // The rest of the tab survives the consumption.
    expect(tab()).toMatchObject({ filePath: null, isDirty: false });
  });

  it("keeps a newer request that landed between the reveal and its clear", () => {
    const { requestReveal, clearReveal } = useEditorStore.getState();
    requestReveal(TAB, 42);
    requestReveal(TAB, 500);
    clearReveal(TAB, 1);
    expect(tab()?.revealRequest).toEqual({ line: 500, nonce: 2 });
  });

  it("ignores a clear for an unknown tab", () => {
    expect(() => useEditorStore.getState().clearReveal("missing", 1)).not.toThrow();
    expect(useEditorStore.getState().getTab("missing")).toBeUndefined();
  });
});
