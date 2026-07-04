/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";

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

describe("DiffView", () => {
  it("renders the filename, +N/−N counts and diff rows", () => {
    render(
      <DiffView
        filename="gateway/run.py"
        oldText="cur = cur.__cause__"
        newText='cur = getattr(cur, "__cause__", None)'
        copyText='cur = getattr(cur, "__cause__", None)'
      />,
    );
    expect(screen.getByText("gateway/run.py")).toBeInTheDocument();
    expect(screen.getByText("+1")).toBeInTheDocument();
    expect(screen.getByText("−1")).toBeInTheDocument();
    expect(screen.getByText("cur = cur.__cause__")).toBeInTheDocument();
    expect(
      screen.getByText('cur = getattr(cur, "__cause__", None)'),
    ).toBeInTheDocument();
  });
});
