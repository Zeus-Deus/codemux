/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type { ReasoningItem, ToolCallItem } from "@/lib/agent-chat/types";

import { ActivityBlock } from "./ActivityBlock";
import type { ActivityStep } from "./transcript-slots";

afterEach(() => cleanup());

function read(seq: number, path: string, overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Read",
    input: { file_path: path },
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function bash(seq: number, command: string, overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Bash",
    input: { command },
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function think(seq: number, text: string, overrides: Partial<ReasoningItem> = {}): ReasoningItem {
  return {
    kind: "reasoning",
    id: `re-${seq}`,
    seq,
    turn_id: "t1",
    text,
    streaming: false,
    ...overrides,
  };
}

function renderBlock(items: ActivityStep[], working: boolean) {
  return render(<ActivityBlock items={items} working={working} />);
}

describe("ActivityBlock — working header", () => {
  it("shows the Working label, live action and done/running counter", () => {
    renderBlock([read(0, "/a"), bash(1, "cargo test", { status: "running" })], true);
    expect(screen.getByText("Working")).toBeInTheDocument();
    // Live action reflects the running step; counter reads N done · M running.
    expect(screen.getByText("run cargo test")).toBeInTheDocument();
    expect(screen.getByText("1 done · 1 running")).toBeInTheDocument();
  });

  it("dims completed steps and keeps the running step full opacity when expanded", () => {
    renderBlock([read(0, "/a"), bash(1, "cargo test", { status: "running" })], true);
    fireEvent.click(screen.getByText("Working"));
    // The running step row exposes a "running" meta; the done step shows "done".
    expect(screen.getByText("running")).toBeInTheDocument();
    expect(screen.getByText("done")).toBeInTheDocument();
  });
});

describe("ActivityBlock — settled header", () => {
  it("rolls up to a summary sentence, a step count and a Details toggle", () => {
    renderBlock([read(0, "/a"), read(1, "/b"), read(2, "/c")], false);
    expect(screen.getByText("Explored the codebase")).toBeInTheDocument();
    expect(screen.getByText("3 steps")).toBeInTheDocument();
    expect(screen.getByText("Details")).toBeInTheDocument();
    // Collapsed by default — step rows are hidden.
    expect(screen.queryByText("/a")).toBeNull();
  });

  it("reveals the step rows and flips to Hide on expand", () => {
    renderBlock([read(0, "/a"), read(1, "/b")], false);
    fireEvent.click(screen.getByText("Explored the codebase"));
    expect(screen.getByText("/a")).toBeInTheDocument();
    expect(screen.getByText("/b")).toBeInTheDocument();
    expect(screen.getByText("Hide")).toBeInTheDocument();
  });

  it("derives a duration from step timestamps when available", () => {
    const first = read(0, "/a", { started_at: 1_000, completed_at: 5_000 });
    const last = read(1, "/b", { started_at: 6_000, completed_at: 73_000 });
    renderBlock([first, last], false);
    // 73_000 − 1_000 = 72s = 1m 12s.
    expect(screen.getByText(/1m 12s/)).toBeInTheDocument();
  });

  it("appends a red failed count when a step errored", () => {
    renderBlock([read(0, "/a"), bash(1, "cargo test", { status: "error" })], false);
    expect(screen.getByText(/1 failed/)).toBeInTheDocument();
    // The errored step row surfaces as a "failed" meta on expand.
    fireEvent.click(screen.getByText("Ran commands and inspected the code"));
    expect(screen.getByText("failed")).toBeInTheDocument();
  });
});

describe("ActivityBlock — step-row inline detail", () => {
  it("expands the full tool detail beneath a clicked step row", () => {
    const withResult = read(0, "/a", {
      result_content: "hello world content",
    });
    renderBlock([withResult, read(1, "/b")], false);
    fireEvent.click(screen.getByText("Explored the codebase"));
    // Detail hidden until the row is clicked.
    expect(screen.queryByText("hello world content")).toBeNull();
    fireEvent.click(screen.getByText("/a"));
    expect(screen.getByText("hello world content")).toBeInTheDocument();
  });

  it("expands a thought's full text beneath a reasoning step row", () => {
    renderBlock([think(0, "short first\nhidden detail line"), read(1, "/b")], false);
    fireEvent.click(screen.getByText("Explored the codebase"));
    expect(screen.queryByText(/hidden detail line/)).toBeNull();
    // Row shows only the first line; clicking reveals the full thought.
    fireEvent.click(screen.getByText("short first"));
    expect(screen.getByText(/hidden detail line/)).toBeInTheDocument();
  });
});

// ── Agent orb (the turn's one live indicator) ──
//
// While a tool is running this block owns the thread's "in progress"
// signal — `shouldShowThinkingIndicator` stands the transcript-tail marker
// down — so the orb here must reflect what the agent is actually doing.

describe("ActivityBlock — agent orb", () => {
  const orbState = () =>
    document.querySelector("canvas")?.getAttribute("data-orb-state") ?? null;

  it("shows no orb once the run has settled", () => {
    renderBlock([read(0, "/a"), read(1, "/b")], false);
    expect(document.querySelector("canvas")).toBeNull();
  });

  it("matches the orb to the running tool", () => {
    renderBlock([read(0, "/a", { status: "running" })], true);
    expect(orbState()).toBe("searching");
  });

  it("reads a running shell's command, not just the tool name", () => {
    renderBlock([bash(0, "git push origin HEAD", { status: "running" })], true);
    expect(orbState()).toBe("connecting");
  });

  it("goes neutral between tools rather than holding the last one's state", () => {
    renderBlock([read(0, "/a")], true);
    expect(orbState()).toBe("working");
  });

  it("reads a running tool after a failed one as a retry", () => {
    renderBlock(
      [bash(0, "cargo test", { status: "error" }), bash(1, "cargo test", { status: "running" })],
      true,
    );
    expect(orbState()).toBe("solving");
  });

  it("renders exactly one orb for the whole block", () => {
    renderBlock(
      [read(0, "/a"), think(1, "hmm"), read(2, "/b", { status: "running" })],
      true,
    );
    expect(document.querySelectorAll("canvas")).toHaveLength(1);
  });
});
