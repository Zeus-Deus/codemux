import { exportDiagnostics, type PerfDiagnostics } from "./interaction-trace";
import { useAppStore } from "@/stores/app-store";
import { getPerformanceDiagnostics } from "@/tauri/commands";
import type {
  AppStateSnapshot,
  NativePerformanceDiagnostics,
  PaneNodeSnapshot,
} from "@/tauri/types";

export interface SnapshotDiagnostics {
  payloadBytes: number;
  workspaces: number;
  surfaces: number;
  panes: number;
  terminalSessions: number;
  browserSessions: number;
  agentBrowserSessions: number;
  chatThreads: number;
}

export interface PerformanceDiagnosticsReport {
  version: 1;
  capturedAt: string;
  snapshot: SnapshotDiagnostics;
  renderer: PerfDiagnostics;
  native: NativePerformanceDiagnostics | { unavailable: true };
}

function countPane(
  pane: PaneNodeSnapshot,
  chatThreads: Set<string>,
): number {
  if (pane.kind === "split") {
    return pane.children.reduce(
      (count, child) => count + countPane(child, chatThreads),
      0,
    );
  }
  if (pane.kind === "agent_chat" && pane.thread_id) {
    chatThreads.add(pane.thread_id);
  }
  return 1;
}

export function snapshotDiagnostics(snapshot: AppStateSnapshot | null): SnapshotDiagnostics {
  if (!snapshot) {
    return {
      payloadBytes: 0,
      workspaces: 0,
      surfaces: 0,
      panes: 0,
      terminalSessions: 0,
      browserSessions: 0,
      agentBrowserSessions: 0,
      chatThreads: 0,
    };
  }
  const chatThreads = new Set<string>();
  let surfaces = 0;
  let panes = 0;
  for (const workspace of snapshot.workspaces) {
    surfaces += workspace.surfaces.length;
    for (const surface of workspace.surfaces) {
      panes += countPane(surface.root, chatThreads);
    }
  }
  let payloadBytes = 0;
  try {
    payloadBytes = new TextEncoder().encode(JSON.stringify(snapshot)).byteLength;
  } catch {
    // A malformed debug fixture should not prevent the rest of the report.
  }
  return {
    payloadBytes,
    workspaces: snapshot.workspaces.length,
    surfaces,
    panes,
    terminalSessions: snapshot.terminal_sessions.length,
    browserSessions: snapshot.browser_sessions.length,
    agentBrowserSessions: snapshot.agent_browser_sessions.length,
    chatThreads: chatThreads.size,
  };
}

export function buildPerformanceDiagnosticsReport(
  snapshot: AppStateSnapshot | null,
  renderer: PerfDiagnostics,
  native: NativePerformanceDiagnostics | { unavailable: true },
  capturedAt = new Date().toISOString(),
): PerformanceDiagnosticsReport {
  return {
    version: 1,
    capturedAt,
    snapshot: snapshotDiagnostics(snapshot),
    renderer,
    native,
  };
}

export async function collectPerformanceDiagnostics(): Promise<PerformanceDiagnosticsReport> {
  const native = await getPerformanceDiagnostics().catch(
    () => ({ unavailable: true }) as const,
  );
  return buildPerformanceDiagnosticsReport(
    useAppStore.getState().appState,
    exportDiagnostics(),
    native,
  );
}
