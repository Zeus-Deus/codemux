import { describe, it, expect, beforeEach } from "vitest";

import { useEditorStore } from "./editor-store";

const TAB = "right-panel:ws-1:/work/codemux/docs/INDEX.md";

function tab() {
  return useEditorStore.getState().getTab(TAB);
}

beforeEach(() => {
  useEditorStore.setState({ tabs: {} });
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
