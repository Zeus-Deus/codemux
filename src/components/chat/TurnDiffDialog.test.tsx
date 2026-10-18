/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import { useUIStore } from "@/stores/ui-store";
import type { AgentChatTurnDiff } from "@/tauri/types";

const getTurnDiff = vi.fn();
vi.mock("@/tauri/commands", () => ({
  agentChatGetTurnDiff: (...args: unknown[]) => getTurnDiff(...args),
}));

import { TurnDiffDialog } from "./TurnDiffDialog";

const ready: AgentChatTurnDiff = {
  thread_id: "t1",
  turn_id: "turn-1",
  status: "ready",
  repo_root: "/repo",
  before_commit: "a",
  after_commit: "b",
  files: [
    {
      path: "src/a.ts",
      old_path: null,
      change: "modified",
      additions: 1,
      deletions: 1,
      binary: false,
      patch: "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1 @@\n-before\n+after\n",
      patch_omitted: null,
    },
    {
      path: "logo.png",
      old_path: null,
      change: "added",
      additions: 0,
      deletions: 0,
      binary: true,
      patch: null,
      patch_omitted: "binary",
    },
    {
      path: "new/name.txt",
      old_path: "old/name.txt",
      change: "renamed",
      additions: 30000,
      deletions: 0,
      binary: false,
      patch: null,
      patch_omitted: "too_large",
    },
  ],
  files_changed: 3,
  additions: 30001,
  deletions: 1,
  truncated: false,
};

function open(workspaceId: string | null = "ws-1") {
  render(
    <TurnDiffDialog
      open
      onOpenChange={() => {}}
      threadId="t1"
      turnId="turn-1"
      workspaceId={workspaceId}
    />,
  );
}

describe("TurnDiffDialog", () => {
  const initialState = useUIStore.getState();
  beforeEach(() => {
    getTurnDiff.mockReset();
    useUIStore.setState(initialState, true);
  });
  afterEach(cleanup);

  it("shows every changed file, with a reason where there is no patch", async () => {
    getTurnDiff.mockResolvedValue(ready);
    open();
    expect(getTurnDiff).toHaveBeenCalledWith("t1", "turn-1");
    expect(await screen.findByText("before")).toBeInTheDocument();
    expect(screen.getByText("after")).toBeInTheDocument();
    expect(screen.getByText("3 files changed ·", { exact: false })).toBeInTheDocument();
    const omitted = screen.getAllByTestId("turn-diff-omitted");
    expect(omitted[0]).toHaveTextContent("logo.png");
    expect(omitted[0]).toHaveTextContent("Binary file");
    expect(omitted[1]).toHaveTextContent("old/name.txt → new/name.txt");
    expect(omitted[1]).toHaveTextContent("Too large to show");
    expect(omitted[1]).toHaveTextContent("+30000");
  });

  it("explains a turn that was not captured and offers the Changes pane", async () => {
    getTurnDiff.mockResolvedValue(null);
    open();
    expect(await screen.findByText(/weren't captured/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open Changes pane" }));
    expect(useUIStore.getState().rightPanelTabs["ws-1"]).toBe("changes");
  });

  it("says when the turn is still running or changed nothing", async () => {
    getTurnDiff.mockResolvedValue({ ...ready, status: "in_progress", files: [] });
    open(null);
    expect(await screen.findByText(/still running/)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Open Changes pane" })).toBeNull();
    cleanup();

    getTurnDiff.mockResolvedValue({ ...ready, files: [], files_changed: 0 });
    open();
    expect(await screen.findByText(/didn't change any files/)).toBeInTheDocument();
  });

  it("reports a failed load", async () => {
    getTurnDiff.mockRejectedValue("git diff failed");
    open();
    expect(await screen.findByText(/Couldn't load .*git diff failed/)).toBeInTheDocument();
  });
});
