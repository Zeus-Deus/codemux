import { describe, it, expect } from "vitest";
import type { PaneNodeSnapshot } from "@/tauri/types";
import { findChatPane, hasAnyPane } from "./pane-tree";

const terminal = (id: string): PaneNodeSnapshot => ({
  kind: "terminal",
  pane_id: id,
  session_id: `session-${id}`,
  title: id,
});

const browser = (id: string): PaneNodeSnapshot => ({
  kind: "browser",
  pane_id: id,
  browser_id: `browser-${id}`,
  title: id,
});

const chat = (id: string): PaneNodeSnapshot => ({
  kind: "agent_chat",
  pane_id: id,
  title: id,
  thread_id: null,
  provider: null,
  cwd: null,
});

const split = (id: string, ...children: PaneNodeSnapshot[]): PaneNodeSnapshot => ({
  kind: "split",
  pane_id: id,
  direction: "horizontal",
  child_sizes: children.map(() => 1 / children.length),
  children,
});

describe("findChatPane", () => {
  it("returns the root when it is itself a chat pane", () => {
    const root = chat("chat-1");
    const found = findChatPane(root);
    expect(found).toBe(root);
  });

  it("returns null when the tree has no chat pane", () => {
    const root = split("s1", terminal("t1"), browser("b1"), terminal("t2"));
    expect(findChatPane(root)).toBeNull();
  });

  it("finds a chat pane inside a shallow split", () => {
    const c = chat("chat-inside");
    const root = split("s1", terminal("t1"), c);
    expect(findChatPane(root)).toBe(c);
  });

  it("finds a chat pane nested deep under multiple splits", () => {
    const c = chat("deep-chat");
    const root = split(
      "s-root",
      terminal("t1"),
      split("s-mid", browser("b1"), split("s-inner", terminal("t2"), c)),
    );
    expect(findChatPane(root)).toBe(c);
  });

  it("returns the first chat pane in left-to-right traversal when multiple exist", () => {
    const first = chat("first");
    const second = chat("second");
    const root = split("s1", split("s-left", first), split("s-right", second));
    expect(findChatPane(root)).toBe(first);
  });
});

describe("hasAnyPane", () => {
  it("returns false for null root (no surface)", () => {
    expect(hasAnyPane(null)).toBe(false);
  });

  it("returns true for a terminal leaf", () => {
    expect(hasAnyPane(terminal("t"))).toBe(true);
  });

  it("returns true for a browser leaf", () => {
    expect(hasAnyPane(browser("b"))).toBe(true);
  });

  it("returns true for a chat leaf", () => {
    expect(hasAnyPane(chat("c"))).toBe(true);
  });

  it("returns false for an empty split (no children)", () => {
    const empty: PaneNodeSnapshot = {
      kind: "split",
      pane_id: "s-empty",
      direction: "horizontal",
      child_sizes: [],
      children: [],
    };
    expect(hasAnyPane(empty)).toBe(false);
  });

  it("returns false for a split whose children are all empty splits", () => {
    const inner: PaneNodeSnapshot = {
      kind: "split",
      pane_id: "s-inner",
      direction: "horizontal",
      child_sizes: [],
      children: [],
    };
    const outer: PaneNodeSnapshot = {
      kind: "split",
      pane_id: "s-outer",
      direction: "vertical",
      child_sizes: [1],
      children: [inner],
    };
    expect(hasAnyPane(outer)).toBe(false);
  });

  it("returns true for a nested mixed tree containing at least one leaf", () => {
    const root = split(
      "s-root",
      split("s-left", {
        kind: "split",
        pane_id: "s-empty",
        direction: "horizontal",
        child_sizes: [],
        children: [],
      }),
      split("s-right", terminal("t-1")),
    );
    expect(hasAnyPane(root)).toBe(true);
  });
});
