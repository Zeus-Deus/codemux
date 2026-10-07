import { create } from "zustand";

import { launchAgentChatPane } from "@/lib/agent-chat/launch-pane";
import { getHighestPriorityStatus } from "@/lib/pane-status";
import { toast } from "@/lib/toast";
import { useAppStore } from "@/stores/app-store";
import {
  closePane,
  closeTab,
  reorderTabs,
  terminalForegroundJobs,
} from "@/tauri/commands";
import type {
  AgentChatProviderKind,
  AppStateSnapshot,
  PaneNodeSnapshot,
  WorkspaceSnapshot,
} from "@/tauri/types";

/**
 * Every user-facing tab and pane close goes through here. Closing kills the
 * terminals and stops the agents inside, so a close that would interrupt
 * something live asks first, and a closed agent chat can be brought back
 * (its transcript is persisted; only the in-flight turn is lost).
 */

export interface CloseGuardPrompt {
  title: string;
  reason: string;
  confirm: () => void;
  cancel: () => void;
}

export const useCloseGuardStore = create<{
  prompt: CloseGuardPrompt | null;
  setPrompt: (prompt: CloseGuardPrompt | null) => void;
}>((set) => ({
  prompt: null,
  setPrompt: (prompt) => set({ prompt }),
}));

interface ClosedChat {
  threadId: string;
  provider: AgentChatProviderKind | null;
  cwd: string | null;
}

interface ClosedEntry {
  workspaceId: string;
  /** Tab position to restore, when the close removed a whole tab. */
  tabIndex: number | null;
  chats: ClosedChat[];
  /** The Undo toast, dismissed once the entry is reopened another way. */
  toastId?: string | number;
}

const MAX_CLOSED = 10;
let closedStack: ClosedEntry[] = [];

type Leaf = Exclude<PaneNodeSnapshot, { kind: "split" }>;

function leaves(node: PaneNodeSnapshot): Leaf[] {
  return node.kind === "split" ? node.children.flatMap(leaves) : [node];
}

function findLeaf(node: PaneNodeSnapshot, paneId: string): Leaf | null {
  return leaves(node).find((leaf) => leaf.pane_id === paneId) ?? null;
}

/** Why closing these panes would interrupt something, or null when nothing
 *  live would be lost. A failed process probe never blocks a close. */
async function closeRisk(
  appState: AppStateSnapshot,
  panes: Leaf[],
): Promise<string | null> {
  const status = getHighestPriorityStatus(
    panes.map((pane) => appState.pane_statuses[pane.pane_id]),
  );
  if (status === "working") return "The agent is still working. Closing stops it.";
  if (status === "permission")
    return "The agent is waiting for your answer. Closing stops it.";
  if (status === "monitoring")
    return "The agent is still monitoring in the background. Closing stops it.";

  const sessionIds = panes.flatMap((pane) =>
    pane.kind === "terminal" ? [pane.session_id] : [],
  );
  if (sessionIds.length === 0) return null;
  const jobs = await terminalForegroundJobs(sessionIds).catch(() => ({}));
  return describeJobs([...new Set(Object.values(jobs))]);
}

/** "node is still running", "node and cargo are still running", or
 *  "node and 2 others are still running" for a tab of busy terminals. */
function describeJobs(jobs: string[]): string | null {
  const [first, second] = jobs;
  if (!first) return null;
  const end = "Closing ends them.";
  if (!second) return `${first} is still running. Closing ends it.`;
  if (jobs.length === 2) return `${first} and ${second} are still running. ${end}`;
  return `${first} and ${jobs.length - 1} others are still running. ${end}`;
}

function rememberChats(
  workspaceId: string,
  tabIndex: number | null,
  panes: Leaf[],
  title: string,
): void {
  const chats = panes.flatMap((pane): ClosedChat[] =>
    pane.kind === "agent_chat" && pane.thread_id
      ? [{ threadId: pane.thread_id, provider: pane.provider, cwd: pane.cwd }]
      : [],
  );
  if (chats.length === 0) return;
  const entry: ClosedEntry = { workspaceId, tabIndex, chats };
  closedStack = [...closedStack, entry].slice(-MAX_CLOSED);
  entry.toastId = toast.undoable({
    message: `Closed ${title}`,
    durationMs: 6000,
    onUndo: () => reopen(entry),
  });
}

/** Resolves once `paneId` shows up in a tab of the workspace. */
function waitForPaneTab(workspaceId: string, paneId: string): Promise<string | null> {
  const find = (appState: AppStateSnapshot | null) => {
    const ws = appState?.workspaces.find((w) => w.workspace_id === workspaceId);
    const surface = ws?.surfaces.find((s) => findLeaf(s.root, paneId));
    return ws?.tabs.find((t) => t.surface_id === surface?.surface_id)?.tab_id ?? null;
  };
  const now = find(useAppStore.getState().appState);
  if (now) return Promise.resolve(now);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      unsubscribe();
      resolve(null);
    }, 3000);
    const unsubscribe = useAppStore.subscribe((state) => {
      const tabId = find(state.appState);
      if (!tabId) return;
      clearTimeout(timer);
      unsubscribe();
      resolve(tabId);
    });
  });
}

async function reopen(entry: ClosedEntry): Promise<void> {
  // The Undo toast and Ctrl+Shift+T can both reach the same entry; only the
  // first one reopens it.
  if (!closedStack.includes(entry)) return;
  closedStack = closedStack.filter((e) => e !== entry);
  if (entry.toastId !== undefined) toast.dismiss(entry.toastId);
  let firstPaneId: string | null = null;
  for (const chat of entry.chats) {
    const paneId = await launchAgentChatPane(
      entry.workspaceId,
      chat.provider,
      chat.cwd,
      "new_tab",
      chat.threadId,
    );
    firstPaneId ??= paneId;
  }
  if (entry.tabIndex === null || !firstPaneId) return;
  const tabId = await waitForPaneTab(entry.workspaceId, firstPaneId);
  const ws = useAppStore
    .getState()
    .appState?.workspaces.find((w) => w.workspace_id === entry.workspaceId);
  if (!tabId || !ws) return;
  const order = ws.tabs.map((t) => t.tab_id).filter((id) => id !== tabId);
  order.splice(Math.min(entry.tabIndex, order.length), 0, tabId);
  await reorderTabs(entry.workspaceId, order);
}

/** Reopen the most recently closed agent chat. False when there is none. */
export function reopenClosedTab(): boolean {
  const entry = closedStack[closedStack.length - 1];
  if (!entry) return false;
  reopen(entry).catch((err) => {
    toast.error("Couldn't reopen the chat", {
      description: err instanceof Error ? err.message : String(err),
    });
  });
  return true;
}

/** Closes at once when nothing live would be lost; otherwise asks. Resolves
 *  once the close has landed (or was cancelled), so a bulk close asks and
 *  closes tab by tab. */
async function guardedClose(
  title: string,
  panes: Leaf[],
  close: () => Promise<unknown>,
  remember: () => void,
): Promise<void> {
  const run = () =>
    close()
      .then(remember)
      .catch(console.error);
  const appState = useAppStore.getState().appState;
  const reason = appState ? await closeRisk(appState, panes) : null;
  if (!reason) return run();
  const store = useCloseGuardStore.getState();
  // A newer request replaces an unanswered one; the older close is dropped.
  store.prompt?.cancel();
  return new Promise((resolve) => {
    let settled = false;
    const answer = (confirmed: boolean) => {
      if (settled) return;
      settled = true;
      if (useCloseGuardStore.getState().prompt === prompt) store.setPrompt(null);
      if (confirmed) void run().then(resolve);
      else resolve();
    };
    const prompt: CloseGuardPrompt = {
      title,
      reason,
      confirm: () => answer(true),
      cancel: () => answer(false),
    };
    store.setPrompt(prompt);
  });
}

function findWorkspace(workspaceId: string): WorkspaceSnapshot | undefined {
  return useAppStore
    .getState()
    .appState?.workspaces.find((w) => w.workspace_id === workspaceId);
}

export function requestCloseTab(workspaceId: string, tabId: string): Promise<void> {
  const ws = findWorkspace(workspaceId);
  const tabIndex = ws?.tabs.findIndex((t) => t.tab_id === tabId) ?? -1;
  const tab = ws?.tabs[tabIndex];
  const surface = ws?.surfaces.find((s) => s.surface_id === tab?.surface_id);
  const panes = surface ? leaves(surface.root) : [];
  const title = tab?.title || "tab";
  return guardedClose(
    title,
    panes,
    () => closeTab(workspaceId, tabId),
    () => rememberChats(workspaceId, tabIndex < 0 ? null : tabIndex, panes, title),
  );
}

export function requestClosePane(paneId: string): Promise<void> {
  const workspaces = useAppStore.getState().appState?.workspaces ?? [];
  for (const ws of workspaces) {
    for (const surface of ws.surfaces) {
      const pane = findLeaf(surface.root, paneId);
      if (!pane) continue;
      const title = pane.title || "pane";
      return guardedClose(
        title,
        [pane],
        () => closePane(paneId),
        () => rememberChats(ws.workspace_id, null, [pane], title),
      );
    }
  }
  return guardedClose("pane", [], () => closePane(paneId), () => {});
}

/** Test seam: forget every remembered close. */
export function resetClosedTabsForTest(): void {
  closedStack = [];
}
