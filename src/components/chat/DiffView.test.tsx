/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { TooltipProvider } from "@/components/ui/tooltip";
import { DiffView, computeLineDiff } from "./DiffView";

afterEach(() => cleanup());

describe("computeLineDiff", () => {
  it("keeps common lines as context and flags the changed line", () => {
    const rows = computeLineDiff("a\nb\nc", "a\nX\nc");
    expect(rows).toEqual([
      { type: "context", text: "a" },
      { type: "remove", text: "b" },
      { type: "add", text: "X" },
      { type: "context", text: "c" },
    ]);
  });

  it("treats an empty old text as an all-added block", () => {
    const rows = computeLineDiff("", "one\ntwo");
    expect(rows).toEqual([
      { type: "add", text: "one" },
      { type: "add", text: "two" },
    ]);
  });
});

describe("computeLineDiff past the LCS table limit", () => {
  const lines = (prefix: string, count: number) =>
    Array.from({ length: count }, (_, i) => `${prefix}${i}`);
  // A full table for these inputs would be ~400M cells; both cases must
  // finish without building one.
  const size = 20_000;

  it("diffs a small change inside a huge edit by its middle alone", () => {
    const head = lines("head", size);
    const tail = lines("tail", size);
    const rows = computeLineDiff(
      [...head, "a", "b", "c", ...tail].join("\n"),
      [...head, "a", "X", "c", ...tail].join("\n"),
    );
    expect(rows).toHaveLength(head.length + 4 + tail.length);
    expect(rows.slice(head.length, head.length + 4)).toEqual([
      { type: "context", text: "a" },
      { type: "remove", text: "b" },
      { type: "add", text: "X" },
      { type: "context", text: "c" },
    ]);
    expect(rows.filter((r) => r.type !== "context")).toHaveLength(2);
  });

  it("shows a huge rewritten middle as removals then additions", () => {
    const before = lines("old", size);
    const after = lines("new", size);
    const rows = computeLineDiff(
      ["top", ...before, "bottom"].join("\n"),
      ["top", ...after, "bottom"].join("\n"),
    );
    expect(rows[0]).toEqual({ type: "context", text: "top" });
    expect(rows.slice(1, 1 + size).every((r) => r.type === "remove")).toBe(true);
    expect(rows.slice(1 + size, 1 + 2 * size).every((r) => r.type === "add")).toBe(true);
    expect(rows[rows.length - 1]).toEqual({ type: "context", text: "bottom" });
    expect(rows).toHaveLength(2 * size + 2);
  });
});

describe("DiffView", () => {
  it("renders the filename, +N/−N counts and diff rows", () => {
    render(
      <DiffView
        filename="gateway/run.py"
        oldText="cur = cur.__cause__"
        newText='cur = getattr(cur, "__cause__", None)'
        copyText='cur = getattr(cur, "__cause__", None)'
      />,
      { wrapper: TooltipProvider },
    );
    expect(screen.getByText("gateway/run.py")).toBeInTheDocument();
    expect(screen.getByText("+1")).toBeInTheDocument();
    expect(screen.getByText("−1")).toBeInTheDocument();
    expect(screen.getByText("cur = cur.__cause__")).toBeInTheDocument();
    expect(
      screen.getByText('cur = getattr(cur, "__cause__", None)'),
    ).toBeInTheDocument();
  });

  it("copies the diff and confirms on the button itself", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(window, "isSecureContext", { configurable: true, value: true });
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } });
    render(
      <DiffView filename="a.ts" oldText="a" newText="b" copyText="the patch" />,
      { wrapper: TooltipProvider },
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy diff" }));
    await vi.waitFor(() => {
      expect(writeText).toHaveBeenCalledWith("the patch");
      expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    });
  });
});

describe("DiffView with a git patch", () => {
  const patch = [
    "diff --git a/src/a.ts b/src/a.ts",
    "index 1111111..2222222 100644",
    "--- a/src/a.ts",
    "+++ b/src/a.ts",
    "@@ -10,3 +10,3 @@ function a() {",
    " keep ten",
    "-old eleven",
    "+new eleven",
    " keep twelve",
    "@@ -40,2 +40,3 @@",
    " keep forty",
    "+added forty-one",
    " keep forty-two",
    "",
  ].join("\n");

  it("renders hunks with their real line numbers and counts", () => {
    const { container } = render(
      <DiffView filename="src/a.ts" patch={patch} copyText={patch} />,
      { wrapper: TooltipProvider },
    );
    expect(screen.getByText("+2")).toBeInTheDocument();
    expect(screen.getByText("−1")).toBeInTheDocument();
    expect(screen.getByText("@@ -10,3 +10,3 @@ function a() {")).toBeInTheDocument();
    expect(screen.getByText("@@ -40,2 +40,3 @@")).toBeInTheDocument();
    // Context lines carry the file's own line numbers, not a running count.
    expect(screen.getByText("keep ten").previousSibling).toHaveTextContent("10");
    expect(screen.getByText("keep forty-two").previousSibling).toHaveTextContent("42");
    // File headers are not rows, and the trailing newline adds no empty row.
    expect(screen.queryByText(/index 1111111/)).not.toBeInTheDocument();
    const rows = container.querySelectorAll(".code-surface > div");
    expect(rows).toHaveLength(9);
  });
});
