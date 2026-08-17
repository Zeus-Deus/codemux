import { describe, it, expect, vi, afterEach } from "vitest";
import type { AppStateSnapshot, PaneNodeSnapshot } from "@/tauri/types";

/**
 * The stress fixture resolves once per module lifetime (the seed is built at
 * module evaluation and must not shift under the running app), so each case
 * re-imports the fixtures module with a different selection.
 */
async function loadSeed(spec: string | null): Promise<AppStateSnapshot> {
  vi.resetModules();
  localStorage.clear();
  if (spec) localStorage.setItem("codemux:fixture", spec);
  const { createSeedAppState } = await import("./mock-fixtures");
  return createSeedAppState();
}

function collectPanes(node: PaneNodeSnapshot, out: PaneNodeSnapshot[] = []): PaneNodeSnapshot[] {
  out.push(node);
  if (node.kind === "split") for (const child of node.children) collectPanes(child, out);
  return out;
}

afterEach(() => {
  localStorage.clear();
});

describe("dev mock seed — stress fixture scaling", () => {
  it("leaves the curated seed untouched when no fixture is selected", async () => {
    const seed = await loadSeed(null);
    expect(seed.workspaces).toHaveLength(21);
    expect(seed.active_workspace_id).toBe("ws-codemux-chat");
  });

  it("grows the seed to the requested workspace count", async () => {
    const seed = await loadSeed("large");
    expect(seed.workspaces).toHaveLength(60);
    // The curated workspaces stay first and keep their identities.
    expect(seed.workspaces[0].workspace_id).toBe("ws-codemux-main");
    const ids = seed.workspaces.map((w) => w.workspace_id);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids).toContain(seed.active_workspace_id);
  });

  it("reaches the audited profile at the xl preset", async () => {
    const seed = await loadSeed("xl");
    expect(seed.workspaces).toHaveLength(80);
  });

  it("gives generated workspaces real surfaces, sessions and statuses", async () => {
    const seed = await loadSeed("large");
    const generated = seed.workspaces.filter((w) => w.workspace_id.startsWith("ws-stress-"));
    expect(generated).toHaveLength(39);

    for (const workspace of generated) {
      expect(workspace.surfaces).toHaveLength(1);
      expect(workspace.tabs).toHaveLength(1);
      expect(workspace.active_surface_id).toBe(workspace.surfaces[0].surface_id);
    }

    // Every third generated workspace opens on a chat pane, so a switch sweep
    // crosses the chat mount path as well as the terminal one.
    const chatPanes = generated
      .flatMap((w) => collectPanes(w.surfaces[0].root))
      .filter((p) => p.kind === "agent_chat");
    expect(chatPanes.length).toBeGreaterThan(0);
    for (const pane of chatPanes) {
      expect(pane.kind).toBe("agent_chat");
      if (pane.kind === "agent_chat") {
        expect(pane.thread_id).toMatch(/^thread-stress-\d+$/);
      }
    }

    // Terminal-backed generated workspaces contribute sessions to the snapshot.
    const sessionIds = new Set(seed.terminal_sessions.map((s) => s.session_id));
    const terminalPanes = generated
      .flatMap((w) => collectPanes(w.surfaces[0].root))
      .filter((p) => p.kind === "terminal");
    for (const pane of terminalPanes) {
      if (pane.kind === "terminal") expect(sessionIds.has(pane.session_id)).toBe(true);
    }
  });

  it("trims below the curated count while keeping the active workspace", async () => {
    const seed = await loadSeed("small");
    expect(seed.workspaces).toHaveLength(8);
    const ids = seed.workspaces.map((w) => w.workspace_id);
    expect(ids).toContain(seed.active_workspace_id);
  });

  it("accepts an inline object spec", async () => {
    const seed = await loadSeed('{"workspaces":25}');
    expect(seed.workspaces).toHaveLength(25);
  });
});
