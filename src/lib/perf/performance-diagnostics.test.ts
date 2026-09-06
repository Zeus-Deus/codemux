import { describe, expect, it } from "vitest";
import { buildPerformanceDiagnosticsReport, snapshotDiagnostics } from "./performance-diagnostics";
import type { AppStateSnapshot } from "@/tauri/types";
import type { PerfDiagnostics } from "./interaction-trace";

function secretSnapshot(): AppStateSnapshot {
  return {
    schema_version: 1,
    active_workspace_id: "secret-workspace-id",
    workspaces: [{
      workspace_id: "secret-workspace-id",
      title: "Secret Client",
      workspace_type: "standard",
      cwd: "/home/private/secret-project",
      git_branch: null,
      git_ahead: 0,
      git_behind: 0,
      git_additions: 0,
      git_deletions: 0,
      git_changed_files: 0,
      notification_count: 0,
      latest_agent_state: null,
      worktree_path: null,
      project_root: null,
      surfaces: [{
        surface_id: "secret-surface",
        title: "Secret",
        active_pane_id: "secret-pane",
        root: {
          kind: "agent_chat",
          pane_id: "secret-pane",
          title: "Secret thread",
          thread_id: "secret-thread",
          provider: "claude",
          cwd: "/home/private/secret-project",
        },
      }],
      tabs: [],
      active_surface_id: "secret-surface",
      active_tab_id: null,
    }],
    terminal_sessions: [],
    browser_sessions: [],
    agent_browser_sessions: [],
    notifications: [],
    detected_ports: [],
    pane_statuses: {},
    persistence: { mode: "writable", reason: null },
    config: { notification_sound: true },
  } as unknown as AppStateSnapshot;
}

it("reduces the snapshot to numeric counts and bytes", () => {
  expect(snapshotDiagnostics(secretSnapshot())).toMatchObject({
    workspaces: 1,
    surfaces: 1,
    panes: 1,
    chatThreads: 1,
    terminalSessions: 0,
  });
});

describe("performance diagnostics privacy", () => {
  it("never retains workspace, path, pane, session, or thread identifiers", () => {
    const renderer = {
      version: 3,
      enabled: true,
      observedEntryTypes: [],
      renderer: {
        userAgent: "test-agent",
        webkitVersion: null,
        webkitReleaseVersion: null,
        linuxWebKitGtk: false,
        devicePixelRatio: 1,
        terminalWebgl: null,
      },
      startup: [],
      traceCount: 0,
      summaries: [],
      traces: [],
    } satisfies PerfDiagnostics;
    const report = buildPerformanceDiagnosticsReport(
      secretSnapshot(),
      renderer,
      { version: 1, capacity: 256, sample_count: 0, timings: [] },
      "2026-01-01T00:00:00.000Z",
    );
    const serialized = JSON.stringify(report);
    expect(serialized).not.toContain("Secret");
    expect(serialized).not.toContain("secret-");
    expect(serialized).not.toContain("/home/private");
  });
});
