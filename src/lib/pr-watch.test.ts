import { describe, expect, it } from "vitest";

import type { PaneNodeSnapshot, PrWatchStatus, WorkspaceSnapshot } from "@/tauri/types";

import { canWatchPr, describePrWatch, watchTargetThreadId } from "./pr-watch";

function watch(patch: Partial<PrWatchStatus> = {}): PrWatchStatus {
  return {
    thread_id: "t1",
    workspace_id: "ws",
    pr_number: 172,
    pr_url: "https://github.com/o/r/pull/172",
    status: "active",
    max_attempts: 3,
    attempts: 0,
    comment_streak: 0,
    head_sha: null,
    last_checked_at_ms: null,
    last_wake_at_ms: null,
    last_wake_summary: null,
    last_error: null,
    stop_reason: null,
    created_at_ms: 0,
    updated_at_ms: 0,
    ...patch,
  };
}

const chat = (pane: string, thread: string | null): PaneNodeSnapshot =>
  ({ kind: "agent_chat", pane_id: pane, title: "", thread_id: thread, provider: null, cwd: null }) as PaneNodeSnapshot;

function workspace(patch: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    pr_number: 172,
    pr_state: "OPEN",
    host_id: null,
    provider_kind: null,
    active_surface_id: "s2",
    surfaces: [
      { surface_id: "s1", root: chat("p1", "first"), active_pane_id: "p1" },
      {
        surface_id: "s2",
        root: { kind: "split", pane_id: "sp", children: [chat("p2", null), chat("p3", "active")] },
        active_pane_id: "p2",
      },
    ],
    ...patch,
  } as unknown as WorkspaceSnapshot;
}

describe("describePrWatch", () => {
  it("reads each state at a glance", () => {
    expect(describePrWatch(null)).toMatchObject({ label: "Off", tone: "off", active: false });
    expect(describePrWatch(watch())).toMatchObject({ label: "On", tone: "on", active: true });
    expect(describePrWatch(watch({ attempts: 2 })).label).toBe("On · 2/3");
    expect(describePrWatch(watch({ last_error: "gh is signed out" }))).toMatchObject({
      label: "Error",
      tone: "error",
      active: true,
    });
    const stopped = describePrWatch(
      watch({ status: "exhausted", stop_reason: "Still failing after 3 automatic fix attempts" }),
    );
    expect(stopped).toMatchObject({ label: "Stopped", tone: "warning", active: false });
    expect(stopped.title).toContain("Still failing after 3 automatic fix attempts on #172");
    expect(describePrWatch(watch({ status: "ended" }))).toMatchObject({ label: "Ended", active: false });
  });
});

describe("canWatchPr", () => {
  it("needs an open pull request on a local GitHub checkout", () => {
    expect(canWatchPr(workspace())).toBe(true);
    expect(canWatchPr(workspace({ pr_state: "DRAFT" }))).toBe(true);
    expect(canWatchPr(workspace({ provider_kind: "github" }))).toBe(true);
    expect(canWatchPr(workspace({ pr_state: "MERGED" }))).toBe(false);
    expect(canWatchPr(workspace({ pr_number: null }))).toBe(false);
    expect(canWatchPr(workspace({ host_id: 3 }))).toBe(false);
    expect(canWatchPr(workspace({ provider_kind: "gitlab" }))).toBe(false);
  });
});

describe("watchTargetThreadId", () => {
  it("prefers the focused chat, then the active surface, then any chat", () => {
    expect(watchTargetThreadId(workspace(), "focused")).toBe("focused");
    expect(watchTargetThreadId(workspace(), null)).toBe("active");
    expect(watchTargetThreadId(workspace({ active_surface_id: "gone" }), null)).toBe("first");
    expect(watchTargetThreadId(workspace({ surfaces: [] }), null)).toBeNull();
  });
});
