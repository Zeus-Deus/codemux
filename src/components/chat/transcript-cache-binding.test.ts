import { expect, it } from "vitest";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { transcriptCacheBinding } from "./transcript-cache-binding";

export function chatWorkspace(id = "a"): WorkspaceSnapshot {
  return {
    workspace_id: id, cwd: "/project", active_tab_id: `tab-${id}`, active_surface_id: `surface-${id}`,
    tabs: [{ tab_id: `tab-${id}`, kind: "terminal", surface_id: `surface-${id}`, browser_id: null }],
    surfaces: [{ surface_id: `surface-${id}`, active_pane_id: `pane-${id}`, root: {
      kind: "agent_chat", pane_id: `pane-${id}`, thread_id: `thread-${id}`, provider: "claude", cwd: "/project",
    } }],
  } as WorkspaceSnapshot;
}

it("identifies only sole selected chat roots with authoritative thread bindings", () => {
  const ws = chatWorkspace();
  expect(transcriptCacheBinding(ws)).toMatchObject({ workspaceId: "a", threadKey: "thread-a", provider: "claude", cwd: "/project" });
  for (const mutate of [
    (w: WorkspaceSnapshot) => { w.tabs[0].kind = "editor"; },
    (w: WorkspaceSnapshot) => { w.tabs[0].kind = "diff"; },
    (w: WorkspaceSnapshot) => { w.tabs[0].kind = "browser"; },
    (w: WorkspaceSnapshot) => { w.tabs.push({ ...w.tabs[0], tab_id: "other" }); },
    (w: WorkspaceSnapshot) => { w.surfaces.push({ ...w.surfaces[0], surface_id: "other" }); },
    (w: WorkspaceSnapshot) => { w.active_tab_id = "other"; },
    (w: WorkspaceSnapshot) => { w.active_surface_id = "other"; },
    (w: WorkspaceSnapshot) => { w.surfaces[0].active_pane_id = "other"; },
    (w: WorkspaceSnapshot) => { w.tabs[0].surface_id = "other"; },
    (w: WorkspaceSnapshot) => { w.surfaces[0].root = { kind: "terminal", pane_id: "p", session_id: "s", title: "sh" }; },
    (w: WorkspaceSnapshot) => { w.surfaces[0].root = { kind: "browser", pane_id: "p", browser_id: "b", title: "web" }; },
    (w: WorkspaceSnapshot) => { w.surfaces[0].root = { kind: "split", pane_id: "p", direction: "horizontal", child_sizes: [1], children: [w.surfaces[0].root] }; },
    (w: WorkspaceSnapshot) => { if (w.surfaces[0].root.kind === "agent_chat") w.surfaces[0].root.thread_id = null; },
  ]) {
    const candidate = structuredClone(ws);
    mutate(candidate);
    expect(transcriptCacheBinding(candidate)).toBeNull();
  }
});

it("invalidates binding identity for thread, provider, cwd, pane, tab and surface changes", () => {
  const original = chatWorkspace();
  const key = transcriptCacheBinding(original)!.key;
  for (const field of ["thread_id", "provider", "cwd", "pane_id"] as const) {
    const ws = structuredClone(original);
    if (ws.surfaces[0].root.kind !== "agent_chat") throw Error("fixture");
    Object.assign(ws.surfaces[0].root, { [field]: field === "provider" ? "codex" : "other" });
    if (field === "pane_id") ws.surfaces[0].active_pane_id = "other";
    expect(transcriptCacheBinding(ws)!.key).not.toBe(key);
  }
  const tab = structuredClone(original);
  tab.tabs[0].tab_id = tab.active_tab_id = "other";
  expect(transcriptCacheBinding(tab)!.key).not.toBe(key);
  const surface = structuredClone(original);
  surface.surfaces[0].surface_id = surface.tabs[0].surface_id = surface.active_surface_id = "other";
  expect(transcriptCacheBinding(surface)!.key).not.toBe(key);
});
