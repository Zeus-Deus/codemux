/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, it, expect } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  ReasoningItem,
  SubagentRunItem,
  SubagentView,
  ToolCallItem,
} from "@/lib/agent-chat/types";
import { useUIStore } from "@/stores/ui-store";

import { ActivityBlock } from "./ActivityBlock";
import type { WorkEntry } from "./transcript-slots";

beforeEach(() => useUIStore.setState({ rightPanelTabs: {} }));
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

function subagent(overrides: Partial<SubagentView>): SubagentView {
  return {
    id: "s1",
    name: "Subagent",
    status: "completed",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function subagentRun(seq: number, subagents: SubagentView[]): SubagentRunItem {
  return { kind: "subagent_run", id: `run-${seq}`, seq, turn_id: "t1", subagents };
}

function renderBlock(items: WorkEntry[], working: boolean, workspaceId?: string) {
  return render(
    <ActivityBlock items={items} working={working} workspaceId={workspaceId} />,
  );
}

/** The collapsed line is the only collapsed disclosure in the block. */
function openLog(container: HTMLElement) {
  const line = container.querySelector('button[aria-expanded="false"]');
  if (!line) throw new Error("no collapsed work-log line");
  fireEvent.click(line);
}

describe("ActivityBlock — one-line work log", () => {
  it("collapses a stretch to the newest action plus totals", () => {
    renderBlock([read(0, "/a"), bash(1, "cargo test", { status: "running" })], true);
    expect(screen.getByText("run")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
    expect(screen.getByText("2 tools")).toBeInTheDocument();
    expect(screen.queryByText("/a")).toBeNull();
    expect(screen.queryByText(/previous/)).toBeNull();
  });

  it("opens the chronological history from the line and closes it again", () => {
    const { container } = renderBlock(
      [read(0, "/a"), bash(1, "cargo test", { status: "running" })],
      true,
    );
    openLog(container);
    expect(screen.getByText("/a")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
    expect(screen.getByText("done")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Work log/ }));
    expect(screen.queryByText("/a")).toBeNull();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
  });

  it("uses the same one-line treatment after settlement", () => {
    renderBlock([read(0, "/a"), read(1, "/b"), read(2, "/c")], false);
    expect(screen.getByText("/c")).toBeInTheDocument();
    expect(screen.getByText("3 tools")).toBeInTheDocument();
    expect(screen.queryByText("/a")).toBeNull();
    expect(screen.queryByText("Explored the codebase")).toBeNull();
  });

  it("renders a single entry as its own row without totals", () => {
    renderBlock([read(0, "/only")], false);
    expect(screen.getByText("/only")).toBeInTheDocument();
    expect(screen.queryByText("1 tool")).toBeNull();
  });

  it("caps the opened history and reveals earlier entries on request", () => {
    const items = Array.from({ length: 13 }, (_, i) => read(i, `/f${i}`));
    const { container } = renderBlock(items, false);
    openLog(container);
    expect(screen.queryByText("/f0")).toBeNull();
    expect(screen.getByText("/f3")).toBeInTheDocument();
    expect(screen.getByText("/f12")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Show 3 earlier" }));
    expect(screen.getByText("/f0")).toBeInTheDocument();
  });

  it("flags failures in the totals while the line shows a later success", () => {
    renderBlock([bash(0, "cargo test", { status: "error" }), read(1, "/fixed")], false);
    expect(screen.getByText("/fixed")).toBeInTheDocument();
    expect(screen.getByText(/1 failed/)).toBeInTheDocument();
  });

  it("folds the history back to one line when the work settles", () => {
    const items = [read(0, "/a"), read(1, "/b")];
    const { container, rerender } = renderBlock(items, true);
    openLog(container);
    expect(screen.getByText("/a")).toBeInTheDocument();
    rerender(<ActivityBlock items={items} working={false} />);
    expect(screen.queryByText("/a")).toBeNull();
  });
});

describe("ActivityBlock — subagent runs", () => {
  const review = subagentRun(1, [
    subagent({ id: "a", name: "review diff", toolUseCount: 6, durationMs: 22_000 }),
    subagent({ id: "b", name: "verify tests", toolUseCount: 4, durationMs: 18_000 }),
  ]);

  it("keeps a subagent run on the same line instead of splitting the log", () => {
    const { container } = renderBlock(
      [read(0, "/a"), review, bash(2, "npm test")],
      false,
      "ws-1",
    );
    expect(screen.getByText("npm test")).toBeInTheDocument();
    expect(screen.getByText("2 tools · 2 subagents")).toBeInTheDocument();
    expect(screen.queryByText(/work log · settled/)).toBeNull();
    // Jump anchors stay mounted while the run is folded out of view.
    expect(container.querySelector("[data-subagent-card='run-1']")).not.toBeNull();
    expect(container.querySelector("[data-subagent-run-id='run-1']")).not.toBeNull();
  });

  it("lists the run in the history and opens the Subagents panel from it", () => {
    const { container } = renderBlock(
      [read(0, "/a"), review, bash(2, "npm test")],
      false,
      "ws-1",
    );
    openLog(container);
    const row = screen.getByRole("button", { name: "View 2 subagents" });
    expect(row).toHaveTextContent("review diff · verify tests");
    expect(row).toHaveTextContent("10 tools · 0m 22s");
    fireEvent.click(row);
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("subagents");
    expect(row).toHaveAttribute("aria-pressed", "true");
  });

  it("shows a live subagent run as the newest line with one orb", () => {
    renderBlock(
      [
        read(0, "/a"),
        subagentRun(1, [subagent({ id: "c", name: "screenshot pass", status: "running" })]),
      ],
      true,
      "ws-1",
    );
    expect(screen.getByText("agents")).toBeInTheDocument();
    expect(screen.getByText("screenshot pass")).toBeInTheDocument();
    expect(document.querySelectorAll("canvas")).toHaveLength(1);
  });
});

describe("ActivityBlock — step-row inline detail", () => {
  it("expands the full tool detail beneath a clicked step row", () => {
    const withResult = read(0, "/a", {
      result_content: "hello world content",
    });
    const { container } = renderBlock([withResult, read(1, "/b")], false);
    openLog(container);
    // Detail hidden until the row is clicked.
    expect(screen.queryByText("hello world content")).toBeNull();
    fireEvent.click(screen.getByText("/a"));
    expect(screen.getByText("hello world content")).toBeInTheDocument();
  });

  it("expands a thought's full text beneath a reasoning step row", () => {
    const { container } = renderBlock(
      [think(0, "short first\nhidden detail line"), read(1, "/b")],
      false,
    );
    openLog(container);
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

  it("renders exactly one orb for the whole block, collapsed or open", () => {
    const { container } = renderBlock(
      [read(0, "/a"), think(1, "hmm"), read(2, "/b", { status: "running" })],
      true,
    );
    expect(document.querySelectorAll("canvas")).toHaveLength(1);
    openLog(container);
    expect(document.querySelectorAll("canvas")).toHaveLength(1);
  });
});
