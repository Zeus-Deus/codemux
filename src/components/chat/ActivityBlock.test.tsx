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

describe("ActivityBlock — compact work log", () => {
  it("shows only the newest action and one quiet history disclosure", () => {
    renderBlock([read(0, "/a"), bash(1, "cargo test", { status: "running" })], true);
    expect(screen.getByText("run")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /\+1 previous tool call/ })).toBeInTheDocument();
    expect(screen.queryByText("/a")).toBeNull();
    expect(screen.queryByText("Working")).toBeNull();
  });

  it("restores previous rows without replacing the newest row", () => {
    renderBlock([read(0, "/a"), bash(1, "cargo test", { status: "running" })], true);
    fireEvent.click(screen.getByRole("button", { name: /\+1 previous tool call/ }));
    expect(screen.getByText("/a")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
    expect(screen.getByText("done")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /Show fewer work entries/ })).toBeInTheDocument();
  });

  it("uses the same latest-first treatment after settlement", () => {
    renderBlock([read(0, "/a"), read(1, "/b"), read(2, "/c")], false);
    expect(screen.getByText("/c")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /\+2 previous tool calls/ })).toBeInTheDocument();
    expect(screen.queryByText("/a")).toBeNull();
    expect(screen.queryByText("Explored the codebase")).toBeNull();
    expect(screen.queryByText("Details")).toBeNull();
  });

  it("reveals all rows in chronology and can compact them again", () => {
    renderBlock([read(0, "/a"), read(1, "/b")], false);
    fireEvent.click(screen.getByRole("button", { name: /\+1 previous tool call/ }));
    expect(screen.getByText("/a")).toBeInTheDocument();
    expect(screen.getByText("/b")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Show fewer work entries/ }));
    expect(screen.queryByText("/a")).toBeNull();
  });

  it("keeps a latest failure visible and flags a hidden earlier failure", () => {
    const { rerender } = renderBlock(
      [read(0, "/a"), bash(1, "cargo test", { status: "error" })],
      false,
    );
    expect(screen.getByText("failed")).toBeInTheDocument();
    rerender(
      <ActivityBlock
        items={[
          bash(0, "cargo test", { status: "error" }),
          read(1, "/fixed"),
        ]}
        working={false}
      />,
    );
    expect(screen.getByRole("button", { name: /\+1 previous tool call · 1 failed/ })).toBeInTheDocument();
  });
});

describe("ActivityBlock — step-row inline detail", () => {
  it("expands the full tool detail beneath a clicked step row", () => {
    const withResult = read(0, "/a", {
      result_content: "hello world content",
    });
    renderBlock([withResult, read(1, "/b")], false);
    fireEvent.click(screen.getByRole("button", { name: /\+1 previous tool call/ }));
    // Detail hidden until the row is clicked.
    expect(screen.queryByText("hello world content")).toBeNull();
    fireEvent.click(screen.getByText("/a"));
    expect(screen.getByText("hello world content")).toBeInTheDocument();
  });

  it("expands a thought's full text beneath a reasoning step row", () => {
    renderBlock([think(0, "short first\nhidden detail line"), read(1, "/b")], false);
    fireEvent.click(screen.getByRole("button", { name: /\+1 previous log entry/ }));
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
