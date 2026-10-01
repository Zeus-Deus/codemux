import { useContext } from "react";
import { act, cleanup, render } from "@testing-library/react";
import { afterEach, expect, it } from "vitest";
import { useAppStore } from "@/stores/app-store";
import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";
import { TranscriptBindingContext } from "./transcript-cache-binding";
import { TranscriptCacheMount } from "./transcript-cache";
import { WorkspaceTranscriptCache } from "./workspace-transcript-cache";

const originalState = useAppStore.getState();
afterEach(() => { cleanup(); useAppStore.setState(originalState, true); });

function workspace(): WorkspaceSnapshot {
  return {
    workspace_id: "workspace", cwd: "/project", active_tab_id: "tab-a", active_surface_id: "surface-a",
    tabs: ["a", "b"].map((id) => ({ tab_id: `tab-${id}`, kind: "terminal", surface_id: `surface-${id}`, browser_id: null })),
    surfaces: ["a", "b"].map((id) => ({ surface_id: `surface-${id}`, active_pane_id: `pane-${id}`, root: {
      kind: "agent_chat", pane_id: `pane-${id}`, thread_id: `thread-${id}`, provider: "claude", cwd: "/project",
    } })),
  } as WorkspaceSnapshot;
}
function select(ws: WorkspaceSnapshot, id: string): WorkspaceSnapshot {
  return { ...ws, active_tab_id: `tab-${id}`, active_surface_id: `surface-${id}` };
}
function setWorkspaces(workspaces: WorkspaceSnapshot[]) {
  act(() => useAppStore.setState({ appState: { workspaces } as AppStateSnapshot }));
}
function Pane({ tab }: { tab: string }) {
  const binding = useContext(TranscriptBindingContext);
  return <section><input data-testid="composer" />{binding ?
    <TranscriptCacheMount cacheKey={binding.key}><button data-testid={binding.threadKey}>{binding.threadKey}</button></TranscriptCacheMount> :
    <span data-testid="uncached">{tab}</span>}
  </section>;
}
function tree(ws: WorkspaceSnapshot, enabled = true) {
  return <WorkspaceTranscriptCache workspace={ws} enabled={enabled}>
    <Pane key={ws.active_tab_id} tab={ws.active_tab_id} />
  </WorkspaceTranscriptCache>;
}

it.each([
  ["thread", (ws: WorkspaceSnapshot) => { if (ws.surfaces[0].root.kind === "agent_chat") ws.surfaces[0].root.thread_id = "replacement"; }],
  ["provider", (ws: WorkspaceSnapshot) => { if (ws.surfaces[0].root.kind === "agent_chat") ws.surfaces[0].root.provider = "codex"; }],
  ["cwd", (ws: WorkspaceSnapshot) => { if (ws.surfaces[0].root.kind === "agent_chat") ws.surfaces[0].root.cwd = "/replacement"; }],
  ["pane", (ws: WorkspaceSnapshot) => { ws.surfaces[0].root.pane_id = ws.surfaces[0].active_pane_id = "replacement"; }],
  ["tab", (ws: WorkspaceSnapshot) => { ws.tabs[0].tab_id = "replacement"; }],
  ["surface", (ws: WorkspaceSnapshot) => { ws.tabs[0].surface_id = ws.surfaces[0].surface_id = "replacement"; }],
  ["conversion", (ws: WorkspaceSnapshot) => { ws.tabs[0].kind = "editor"; }],
  ["deletion", (ws: WorkspaceSnapshot) => { ws.tabs.splice(0, 1); ws.surfaces.splice(0, 1); }],
] as const)("evicts a parked tab immediately after %s changes without visiting it", (_name, mutate) => {
  const a = workspace();
  const b = select(a, "b");
  setWorkspaces([a]);
  const view = render(tree(a));
  const host = view.getByTestId("thread-a").closest('[data-transcript-cache-host]')!;
  setWorkspaces([b]);
  view.rerender(tree(b));
  const visible = view.getByTestId("thread-b");
  const changed = structuredClone(b);
  mutate(changed);
  // The store subscription must invalidate parked keys without a pane rerender.
  setWorkspaces([changed]);
  expect(view.queryByTestId("thread-a")).toBeNull();
  expect(host.childNodes).toHaveLength(0);
  expect(host.isConnected).toBe(false);
  expect(view.getByTestId("thread-b")).toBe(visible);
});

it("preserves Composer ancestry when adding another compatible tab", () => {
  const multi = workspace();
  const single = { ...multi, tabs: multi.tabs.slice(0, 1), surfaces: multi.surfaces.slice(0, 1) };
  setWorkspaces([single]);
  const view = render(tree(single));
  const composer = view.getByTestId("composer");
  const row = view.getByTestId("thread-a");
  setWorkspaces([multi]);
  view.rerender(tree(multi));
  expect(view.getByTestId("composer")).toBe(composer);
  expect(view.getByTestId("thread-a")).toBe(row);
});

it("keeps parked transcripts while visiting a noncacheable tab", () => {
  const a = workspace();
  const editor = select(structuredClone(a), "b");
  editor.tabs[1].kind = "editor";
  setWorkspaces([a]);
  const view = render(tree(a));
  const row = view.getByTestId("thread-a");
  setWorkspaces([editor]);
  view.rerender(tree(editor));
  expect(view.getByTestId("uncached")).toBeTruthy();
  expect(view.getByTestId("thread-a")).toBe(row);
  expect(row.closest('[data-transcript-cache-parking]')).not.toBeNull();
  const returned = select(editor, "a");
  setWorkspaces([returned]);
  view.rerender(tree(returned));
  expect(view.getByTestId("thread-a")).toBe(row);
});

it("preserves both transcript hosts while alternating tabs in the same workspace", () => {
  const a = workspace();
  const b = select(a, "b");
  setWorkspaces([a]);
  const view = render(tree(a));
  const rowA = view.getByTestId("thread-a");
  const hostA = rowA.closest('[data-transcript-cache-host]');
  const composerA = view.getByTestId("composer");
  setWorkspaces([b]);
  view.rerender(tree(b));
  const rowB = view.getByTestId("thread-b");
  const hostB = rowB.closest('[data-transcript-cache-host]');
  expect(view.getByTestId("thread-a")).toBe(rowA);
  expect(hostA?.closest('[data-transcript-cache-parking]')).not.toBeNull();
  for (let i = 0; i < 3; i++) {
    setWorkspaces([a]);
    view.rerender(tree(a));
    expect(view.getByTestId("thread-a")).toBe(rowA);
    expect(rowA.closest('[data-transcript-cache-host]')).toBe(hostA);
    expect(rowA.closest('[data-transcript-cache-slot]')).not.toBeNull();
    expect(view.getByTestId("composer")).not.toBe(composerA);
    setWorkspaces([b]);
    view.rerender(tree(b));
    expect(view.getByTestId("thread-b")).toBe(rowB);
    expect(rowB.closest('[data-transcript-cache-host]')).toBe(hostB);
  }
});
