import { describe, it, expect } from "vitest";

import {
  getHighestPriorityStatus,
  getWorkspaceStatus,
  pickHigherStatus,
  statusRank,
  STATUS_DOT_CLASS,
  STATUS_LABEL,
  STATUS_TEXT_CLASS,
} from "./pane-status";
import type {
  ActivePaneStatus,
  PaneStatus,
  SurfaceSnapshot,
} from "@/tauri/types";

const ACTIVE: ActivePaneStatus[] = [
  "permission",
  "working",
  "monitoring",
  "review",
];

function chatSurface(paneId: string): SurfaceSnapshot {
  return {
    surface_id: `surface-${paneId}`,
    title: paneId,
    root: {
      kind: "agent_chat",
      pane_id: paneId,
      title: paneId,
      thread_id: null,
      provider: "claude",
      cwd: "/tmp",
    },
    active_pane_id: paneId,
  };
}

describe("pane-status priority ladder", () => {
  // The one ordering the sidebar, the rail, the hover card, the command
  // palette and the Rust `PaneStatus` enum all agree on.
  it("ranks permission > working > monitoring > review > idle", () => {
    expect(statusRank("permission")).toBeGreaterThan(statusRank("working"));
    expect(statusRank("working")).toBeGreaterThan(statusRank("monitoring"));
    expect(statusRank("monitoring")).toBeGreaterThan(statusRank("review"));
    expect(statusRank(null)).toBe(0);
  });

  it("lets monitoring win over review but never over real work", () => {
    expect(pickHigherStatus("monitoring", "review")).toBe("monitoring");
    expect(pickHigherStatus("review", "monitoring")).toBe("monitoring");
    expect(pickHigherStatus("monitoring", "working")).toBe("working");
    expect(pickHigherStatus("monitoring", "permission")).toBe("permission");
    expect(pickHigherStatus("monitoring", "idle")).toBe("monitoring");
  });

  it("aggregates a mixed pane set to the most demanding status", () => {
    expect(
      getHighestPriorityStatus(["idle", "monitoring", "review"] as PaneStatus[]),
    ).toBe("monitoring");
    expect(
      getHighestPriorityStatus(["monitoring", "working"] as PaneStatus[]),
    ).toBe("working");
    expect(getHighestPriorityStatus(["idle", undefined])).toBeNull();
  });

  it("derives a workspace status from its panes", () => {
    const surfaces = [chatSurface("p-a"), chatSurface("p-b")];
    expect(
      getWorkspaceStatus(surfaces, { "p-a": "review", "p-b": "monitoring" }),
    ).toBe("monitoring");
  });
});

describe("pane-status presentation", () => {
  it("gives every active status exactly one label and tone", () => {
    for (const status of ACTIVE) {
      expect(STATUS_LABEL[status]).toBeTruthy();
      expect(STATUS_TEXT_CLASS[status]).toBeTruthy();
      expect(STATUS_DOT_CLASS[status]).toBeTruthy();
    }
    expect(STATUS_LABEL.monitoring).toBe("Monitoring");
  });

  it("uses the dedicated monitoring token, not another status tone", () => {
    expect(STATUS_TEXT_CLASS.monitoring).toBe("text-status-monitoring");
    expect(STATUS_DOT_CLASS.monitoring).toBe("bg-status-monitoring");
  });

  // Monitoring is calm background presence, not a call for attention — the
  // pulse is reserved for the one status that is genuinely blocked on a human.
  it("never animates the monitoring dot", () => {
    expect(STATUS_DOT_CLASS.monitoring).not.toContain("animate");
    expect(STATUS_DOT_CLASS.permission).toContain("animate-pulse");
  });
});
