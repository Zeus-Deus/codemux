/**
 * How platform plugin vocabulary maps onto Codemux state. A plugin "thread" is
 * an agent chat thread; its "project" is the workspace's project identity
 * (`project_uid`, else the project root, else the workspace folder), the same
 * id the host's plugin services use. The client's "location" is what the user
 * is looking at, so a slow plugin answer can tell whether they moved on.
 */
import { selectUtilityPage } from "@/lib/utility-pages";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

export const pluginProjectIdOf = (workspace: Pick<WorkspaceSnapshot, "project_uid" | "project_root" | "cwd">) =>
  workspace.project_uid || workspace.project_root || workspace.cwd;

function collectChats(node: PaneNodeSnapshot, out: { paneId: string; threadId: string }[]) {
  if (node.kind === "split") for (const child of node.children) collectChats(child, out);
  else if (node.kind === "agent_chat" && node.thread_id) out.push({ paneId: node.pane_id, threadId: node.thread_id });
}

/** Every chat in a workspace, active surface first. */
export function workspaceChats(workspace: WorkspaceSnapshot): { paneId: string; threadId: string }[] {
  const out: { paneId: string; threadId: string }[] = [];
  const surfaces = [...workspace.surfaces].sort(
    (a, b) => Number(b.surface_id === workspace.active_surface_id) - Number(a.surface_id === workspace.active_surface_id),
  );
  for (const surface of surfaces) collectChats(surface.root, out);
  return out;
}

function findPane(node: PaneNodeSnapshot, paneId: string): PaneNodeSnapshot | null {
  if (node.pane_id === paneId) return node;
  if (node.kind !== "split") return null;
  for (const child of node.children) {
    const match = findPane(child, paneId);
    if (match) return match;
  }
  return null;
}

/** The chat the workspace shows: its focused chat pane, else its first chat. */
export function workspaceActiveThreadId(workspace: WorkspaceSnapshot | null | undefined): string | null {
  if (!workspace) return null;
  const surface = workspace.surfaces.find((candidate) => candidate.surface_id === workspace.active_surface_id);
  const pane = surface ? findPane(surface.root, surface.active_pane_id) : null;
  if (pane?.kind === "agent_chat" && pane.thread_id) return pane.thread_id;
  return workspaceChats(workspace)[0]?.threadId ?? null;
}

/** The workspace and pane that hold a chat thread, if one is open. */
export function findThreadLocation(
  threadId: string,
  workspaces: ReadonlyArray<WorkspaceSnapshot> = useAppStore.getState().appState?.workspaces ?? [],
): { workspace: WorkspaceSnapshot; paneId: string } | null {
  for (const workspace of workspaces) {
    const chat = workspaceChats(workspace).find((entry) => entry.threadId === threadId);
    if (chat) return { workspace, paneId: chat.paneId };
  }
  return null;
}

export function workspaceById(workspaceId: string | null | undefined): WorkspaceSnapshot | null {
  if (!workspaceId) return null;
  return useAppStore.getState().appState?.workspaces.find((ws) => ws.workspace_id === workspaceId) ?? null;
}

/**
 * What the user is looking at now: the open page (settings, a utility page)
 * or the active workspace and its focused pane.
 */
export function currentPluginLocationKey(): string {
  const ui = useUIStore.getState();
  const page = ui.showSettings ? "settings" : (selectUtilityPage(ui) ?? "");
  const appState = useAppStore.getState();
  const workspaceId = selectActiveWorkspaceId(appState) ?? "";
  const workspace = workspaceById(workspaceId);
  const surface = workspace?.surfaces.find((candidate) => candidate.surface_id === workspace.active_surface_id);
  return [page, workspaceId, surface?.active_pane_id ?? ""].join("|");
}

/** Composer key: one composer per workspace and chat thread. */
export const pluginComposerKey = (workspaceId: string, threadId: string) => `${workspaceId}\0${threadId}`;

export function parsePluginComposerKey(key: string): { workspaceId: string; threadId: string } | null {
  const at = key.indexOf("\0");
  return at < 0 ? null : { workspaceId: key.slice(0, at), threadId: key.slice(at + 1) };
}
