import { expect, it } from "vitest";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { transcriptCacheBinding, transcriptCacheBindings } from "./transcript-cache-binding";

export function chatWorkspace(id = "a"): WorkspaceSnapshot {
  return {
    workspace_id: id, cwd: "/project", active_tab_id: `tab-${id}`, active_surface_id: `surface-${id}`,
    tabs: [{ tab_id: `tab-${id}`, kind: "terminal", surface_id: `surface-${id}`, browser_id: null }],
    surfaces: [{ surface_id: `surface-${id}`, active_pane_id: `pane-${id}`, root: {
      kind: "agent_chat", pane_id: `pane-${id}`, thread_id: `thread-${id}`, provider: "claude", cwd: "/project",
    } }],
  } as WorkspaceSnapshot;
}

export function twoTabChatWorkspace(): WorkspaceSnapshot {
  const ws = chatWorkspace();
  const other = chatWorkspace("b");
  ws.tabs.push(...other.tabs);
  ws.surfaces.push(...other.surfaces);
  return ws;
}

it("binds the active single-chat tab in a multi-tab workspace and changes keys on switching", () => {
  const ws = twoTabChatWorkspace();
  const first = transcriptCacheBinding(ws);
  expect(first).toMatchObject({ workspaceId: "a", threadKey: "thread-a" });
  ws.active_tab_id = "tab-b";
  ws.active_surface_id = "surface-b";
  const second = transcriptCacheBinding(ws);
  expect(second).toMatchObject({ workspaceId: "a", threadKey: "thread-b" });
  expect(second!.key).not.toBe(first!.key);
});

it("enumerates compatible background tabs even when the active route is noncacheable", () => {
  expect(transcriptCacheBindings).toBeTypeOf("function");
  const ws = twoTabChatWorkspace();
  const first = transcriptCacheBinding(ws)!;
  ws.active_tab_id = "tab-b";
  ws.active_surface_id = "surface-b";
  const second = transcriptCacheBinding(ws)!;
  ws.active_tab_id = "editor";
  ws.active_surface_id = "missing";
  expect(transcriptCacheBindings(ws).map((binding) => binding.key)).toEqual([first.key, second.key]);
  expect(transcriptCacheBindings(null)).toEqual([]);
});

it("excludes incompatible background tabs without dropping compatible neighbors", () => {
  for (const mutate of [
    (w: WorkspaceSnapshot) => { w.tabs[1].kind = "editor"; },
    (w: WorkspaceSnapshot) => { w.tabs[1].kind = "diff"; },
    (w: WorkspaceSnapshot) => { w.tabs[1].kind = "browser"; },
    (w: WorkspaceSnapshot) => { w.tabs[1].surface_id = "missing"; },
    (w: WorkspaceSnapshot) => { w.surfaces[1].active_pane_id = "other"; },
    (w: WorkspaceSnapshot) => { w.surfaces[1].root = { kind: "split", pane_id: "split", direction: "horizontal", child_sizes: [1], children: [w.surfaces[1].root] }; },
    (w: WorkspaceSnapshot) => { w.surfaces[1].root = { kind: "terminal", pane_id: "p", session_id: "s", title: "sh" }; },
    (w: WorkspaceSnapshot) => { if (w.surfaces[1].root.kind === "agent_chat") w.surfaces[1].root.thread_id = null; },
  ]) {
    const ws = twoTabChatWorkspace();
    const active = transcriptCacheBinding(ws)!;
    mutate(ws);
    expect(transcriptCacheBindings(ws)).toEqual([active]);
  }
});

it("identifies only selected single-chat roots with authoritative thread bindings", () => {
  const ws = chatWorkspace();
  expect(transcriptCacheBinding(ws)).toMatchObject({ workspaceId: "a", threadKey: "thread-a", provider: "claude", cwd: "/project" });
  for (const mutate of [
    (w: WorkspaceSnapshot) => { w.tabs[0].kind = "editor"; },
    (w: WorkspaceSnapshot) => { w.tabs[0].kind = "diff"; },
    (w: WorkspaceSnapshot) => { w.tabs[0].kind = "browser"; },
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
  const cwd = structuredClone(original);
  cwd.cwd = "/other-project";
  expect(transcriptCacheBinding(cwd)!.key).not.toBe(key);
});
