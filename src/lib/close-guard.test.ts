import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  closeTab: vi.fn(),
  closePane: vi.fn(),
  reorderTabs: vi.fn(),
  terminalForegroundJobs: vi.fn(),
  launchAgentChatPane: vi.fn(),
  undoable: vi.fn(),
  error: vi.fn(),
}));

vi.mock("@/tauri/commands", () => ({
  closeTab: mocks.closeTab,
  closePane: mocks.closePane,
  reorderTabs: mocks.reorderTabs,
  terminalForegroundJobs: mocks.terminalForegroundJobs,
}));
vi.mock("@/lib/agent-chat/launch-pane", () => ({
  launchAgentChatPane: mocks.launchAgentChatPane,
}));
vi.mock("@/lib/toast", () => ({
  toast: { undoable: mocks.undoable, error: mocks.error },
}));

import {
  reopenClosedTab,
  requestClosePane,
  requestCloseTab,
  resetClosedTabsForTest,
  useCloseGuardStore,
} from "./close-guard";
import { useAppStore } from "@/stores/app-store";
import type {
  AppStateSnapshot,
  PaneNodeSnapshot,
  PaneStatus,
  WorkspaceSnapshot,
} from "@/tauri/types";

const terminal: PaneNodeSnapshot = {
  kind: "terminal",
  pane_id: "pane-term",
  session_id: "sess-term",
  title: "Dev server",
};
const chat: PaneNodeSnapshot = {
  kind: "agent_chat",
  pane_id: "pane-chat",
  title: "Agent Chat",
  thread_id: "thread-1",
  provider: "codex",
  cwd: "/p",
};

function seed(statuses: Record<string, PaneStatus> = {}) {
  const tab = (id: string, title: string) => ({
    tab_id: id,
    kind: "terminal" as const,
    title,
    surface_id: `surface-${id}`,
    browser_id: null,
    icon: null,
  });
  const workspace = {
    workspace_id: "ws-1",
    tabs: [tab("a", "Dev server"), tab("b", "Agent Chat"), tab("c", "Notes")],
    surfaces: [
      { surface_id: "surface-a", title: "", active_pane_id: "pane-term", root: terminal },
      { surface_id: "surface-b", title: "", active_pane_id: "pane-chat", root: chat },
    ],
  } as unknown as WorkspaceSnapshot;
  useAppStore.setState({
    appState: {
      workspaces: [workspace],
      pane_statuses: statuses,
    } as unknown as AppStateSnapshot,
  });
}

function prompt() {
  return useCloseGuardStore.getState().prompt;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocks.closeTab.mockResolvedValue(undefined);
  mocks.closePane.mockResolvedValue(null);
  mocks.reorderTabs.mockResolvedValue(undefined);
  mocks.terminalForegroundJobs.mockResolvedValue({});
  useCloseGuardStore.setState({ prompt: null });
  resetClosedTabsForTest();
  seed();
});

describe("requestCloseTab", () => {
  it("closes an idle shell at once", async () => {
    await requestCloseTab("ws-1", "a");
    expect(mocks.terminalForegroundJobs).toHaveBeenCalledWith(["sess-term"]);
    expect(mocks.closeTab).toHaveBeenCalledWith("ws-1", "a");
    expect(prompt()).toBeNull();
  });

  it("asks before killing a running process, and Cancel keeps it", async () => {
    mocks.terminalForegroundJobs.mockResolvedValue({ "sess-term": "node" });
    const done = requestCloseTab("ws-1", "a");
    await vi.waitFor(() => expect(prompt()).not.toBeNull());
    expect(prompt()).toMatchObject({
      title: "Dev server",
      reason: "node is still running. Closing ends it.",
    });
    expect(mocks.closeTab).not.toHaveBeenCalled();

    prompt()!.cancel();
    await done;
    expect(prompt()).toBeNull();
    expect(mocks.closeTab).not.toHaveBeenCalled();
  });

  it("asks before stopping a working agent, and Close goes ahead", async () => {
    seed({ "pane-chat": "working" });
    const done = requestCloseTab("ws-1", "b");
    await vi.waitFor(() => expect(prompt()).not.toBeNull());
    expect(prompt()!.reason).toBe("The agent is still working. Closing stops it.");

    prompt()!.confirm();
    await done;
    expect(mocks.closeTab).toHaveBeenCalledWith("ws-1", "b");
  });

  it("does not ask for a finished agent", async () => {
    seed({ "pane-chat": "review" });
    await requestCloseTab("ws-1", "b");
    expect(prompt()).toBeNull();
    expect(mocks.closeTab).toHaveBeenCalledWith("ws-1", "b");
  });

  it("closes when the process probe fails", async () => {
    mocks.terminalForegroundJobs.mockRejectedValue(new Error("no ipc"));
    await requestCloseTab("ws-1", "a");
    expect(mocks.closeTab).toHaveBeenCalledWith("ws-1", "a");
  });

  it("drops an unanswered prompt when a newer close replaces it", async () => {
    seed({ "pane-chat": "working", "pane-term": "permission" });
    const first = requestCloseTab("ws-1", "b");
    await vi.waitFor(() => expect(prompt()?.title).toBe("Agent Chat"));
    void requestCloseTab("ws-1", "a");
    await first;
    await vi.waitFor(() => expect(prompt()?.title).toBe("Dev server"));
    expect(mocks.closeTab).not.toHaveBeenCalled();
  });
});

describe("reopening a closed chat", () => {
  it("offers Undo and reopens the thread at its old position", async () => {
    await requestCloseTab("ws-1", "b");
    await vi.waitFor(() => expect(mocks.undoable).toHaveBeenCalled());
    expect(mocks.undoable.mock.calls[0][0].message).toBe("Closed Agent Chat");

    // The backend has dropped tab b; the reopened chat lands at the end.
    mocks.launchAgentChatPane.mockImplementation(async () => {
      const state = useAppStore.getState().appState!;
      const ws = state.workspaces[0];
      useAppStore.setState({
        appState: {
          ...state,
          workspaces: [
            {
              ...ws,
              tabs: [
                ws.tabs[0],
                ws.tabs[2],
                { ...ws.tabs[1], tab_id: "new", surface_id: "surface-new" },
              ],
              surfaces: [
                ...ws.surfaces,
                {
                  surface_id: "surface-new",
                  title: "",
                  active_pane_id: "pane-new",
                  root: { ...chat, pane_id: "pane-new" },
                },
              ],
            },
          ],
        },
      });
      return "pane-new";
    });

    expect(reopenClosedTab()).toBe(true);
    await vi.waitFor(() => expect(mocks.reorderTabs).toHaveBeenCalled());
    expect(mocks.launchAgentChatPane).toHaveBeenCalledWith(
      "ws-1",
      "codex",
      "/p",
      "new_tab",
      "thread-1",
    );
    expect(mocks.reorderTabs).toHaveBeenCalledWith("ws-1", ["a", "new", "c"]);
    // Each close reopens once.
    expect(reopenClosedTab()).toBe(false);
  });

  it("has nothing to reopen after closing a plain terminal", async () => {
    await requestCloseTab("ws-1", "a");
    expect(mocks.undoable).not.toHaveBeenCalled();
    expect(reopenClosedTab()).toBe(false);
  });
});

describe("requestClosePane", () => {
  it("guards the pane's own status and remembers its chat", async () => {
    seed({ "pane-chat": "permission" });
    const done = requestClosePane("pane-chat");
    await vi.waitFor(() => expect(prompt()).not.toBeNull());
    expect(prompt()!.reason).toBe(
      "The agent is waiting for your answer. Closing stops it.",
    );
    prompt()!.confirm();
    await done;
    expect(mocks.closePane).toHaveBeenCalledWith("pane-chat");
    await vi.waitFor(() => expect(mocks.undoable).toHaveBeenCalled());
  });

  it("closes a pane it cannot find without asking", async () => {
    await requestClosePane("pane-unknown");
    expect(mocks.closePane).toHaveBeenCalledWith("pane-unknown");
  });
});
