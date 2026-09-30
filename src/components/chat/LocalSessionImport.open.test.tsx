/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { LocalSessionImport } from "./LocalSessionImport";
import { AgentChatPane } from "./AgentChatPane";
import { useEnsureDraftWhenEmpty } from "@/hooks/use-ensure-draft-when-empty";
import { useActiveWorkspace, useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import type { AppStateSnapshot, PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

const api = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", async (importOriginal) => ({
  ...await importOriginal<typeof import("@tauri-apps/api/core")>(),
  invoke: api.invoke,
}));
// Native event subscriptions are outside this renderer test. Keep the real
// command wrappers, stores, empty-state hook, pane, hydration and transcript.
vi.mock("@/hooks/use-agent-chat-events", () => ({ useAgentChatEvents: () => {} }));
vi.mock("@/hooks/use-tauri-event", () => ({ useTauriEvent: () => {} }));
vi.mock("./Composer", () => ({ Composer: () => <textarea aria-label="Writable composer" /> }));
// jsdom has no measured viewport. Render the real transcript rows instead of
// making LegendList's layout-dependent window hide every row.
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return { LegendList: React.forwardRef(function TestList(
    props: { data: unknown[]; renderItem: (entry: { item: unknown; index: number }) => React.ReactNode },
    ref,
  ) {
    const node = React.useRef<HTMLDivElement>(null);
    React.useImperativeHandle(ref, () => ({
      getScrollableNode: () => node.current,
      getState: () => ({ isAtEnd: true, listen: () => () => {} }),
      scrollToEnd: () => Promise.resolve(), scrollToIndex: () => Promise.resolve(),
    }));
    return <div ref={node} data-transcript-viewport>{props.data.map((item, index) =>
      <React.Fragment key={index}>{props.renderItem({ item, index })}</React.Fragment>)}</div>;
  }) };
});

const pane: Extract<PaneNodeSnapshot, { kind: "agent_chat" }> = {
  kind: "agent_chat", pane_id: "imported-pane", thread_id: "imported-thread",
  provider: "claude", cwd: "/demo/parser", title: "Fix the parser",
};
const workspace: WorkspaceSnapshot = {
  workspace_id: "imported-workspace", title: "Imported chats", workspace_type: "standard",
  cwd: "/demo/parser", project_root: "/demo/parser", worktree_path: null,
  git_branch: "main", git_ahead: 0, git_behind: 0, git_additions: 0,
  git_deletions: 0, git_changed_files: 0, notification_count: 0,
  notifications_muted: false, latest_agent_state: null, pr_number: null,
  pr_state: null, pr_url: null, linked_issue: null,
  active_tab_id: "imported-tab", active_surface_id: "imported-surface",
  tabs: [{ tab_id: "imported-tab", kind: "terminal", surface_id: "imported-surface", title: "Fix the parser", browser_id: null, icon: null }],
  surfaces: [{ surface_id: "imported-surface", title: "Fix the parser", active_pane_id: pane.pane_id, root: pane }],
};
function snapshot(active: boolean): AppStateSnapshot {
  return {
    schema_version: 1, snapshot_revision: active ? 2 : 1,
    active_workspace_id: active ? workspace.workspace_id : "", workspaces: active ? [workspace] : [],
    terminal_sessions: [], browser_sessions: [], agent_browser_sessions: [],
    notifications: [], detected_ports: [], pane_statuses: {},
    persistence: { schema_version: 1, stores_layout_metadata: true,
      stores_terminal_metadata: true, stores_live_process_state: false },
    config: {} as AppStateSnapshot["config"],
  };
}
const opened = snapshot(true);
const rows = [
  { type: "user_message", thread_id: pane.thread_id, client_nonce: "imported-user", text: "Please fix the parser from the saved conversation." },
  { type: "item_completed", thread_id: pane.thread_id, turn_id: "imported-turn", item: { kind: "assistant_text", text: "The saved parser fix is in the transcript." } },
  { type: "turn_completed", thread_id: pane.thread_id, turn_id: "imported-turn", status: { kind: "success" }, usage: null },
].map((payload, i) => ({ id: i + 1, payload: JSON.stringify(payload) }));

// Mirrors WorkspaceMain's draft-before-workspace dispatch without mounting
// unrelated terminal, browser, sidebar and right-panel infrastructure.
function NavigationSurface() {
  useEnsureDraftWhenEmpty();
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);
  const activeWorkspace = useActiveWorkspace();
  const root = activeWorkspace?.surfaces.find((s) => s.surface_id === activeWorkspace.active_surface_id)?.root;
  return <>
    {activeDraftId ? <textarea aria-label="Unsent draft" value={useChatDraftStore.getState().draftsById[activeDraftId].inputDraft} readOnly />
      : root?.kind === "agent_chat" ? <AgentChatPane pane={root} /> : null}
    <LocalSessionImport />
  </>;
}

beforeEach(() => {
  api.invoke.mockReset();
  useChatDraftStore.setState({ draftsById: {}, activeHomeDraftId: null, projectDraftIdByPath: {}, activeDraftId: null });
  useAgentChatStore.setState({ threads: {} });
  useAppStore.setState({ appState: snapshot(false), homeDir: "/home/test", lastSeenRevision: 1,
    pendingActiveWorkspaceId: null, pendingActivationAt: null, deltaBuffer: new Map(), backendInstance: null });
  useFeatureFlags.setState({ loaded: true, enableAgentChat: true, enableLazyWorkspaceCreation: true });
  useUIStore.setState({ showLocalSessionImport: true, showSettings: true });
  api.invoke.mockImplementation(async (command: string) => {
    switch (command) {
      case "agent_chat_scan_local_sessions": return { sessions: [{ source_id: "claude:one", provider: "claude", title: "Fix the parser", cwd: "/demo/parser", last_active_at: "2026-09-29T10:00:00Z", message_count: 2, already_imported: false }], warnings: [] };
      case "agent_chat_import_local_sessions": return { imported: [{ source_id: "claude:one", thread_id: pane.thread_id, workspace_id: workspace.workspace_id }], skipped: 0, warnings: [] };
      case "agent_chat_open_search_result": return { workspace_id: workspace.workspace_id, pane_id: pane.pane_id };
      case "get_app_state": return opened;
      case "agent_chat_get_session": return { thread_id: pane.thread_id, imported_from: "claude:one" };
      case "agent_chat_list_messages_tail": return { rows, total_rows: rows.length, complete: true };
      case "agent_chat_thread_head_id": return 3;
      case "agent_chat_turn_active": return false;
      case "agent_chat_list_sessions": case "agent_chat_list_turn_checkpoints": return [];
      case "grep_count_pattern": return 0;
      default: throw new Error(`Unexpected renderer IPC: ${command}`);
    }
  });
});
afterEach(cleanup);

async function importOne() {
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Fix the parser/ }));
  fireEvent.click(screen.getByRole("button", { name: "Import selected (1)" }));
  return screen.findByRole("button", { name: "Open Fix the parser" });
}

it("locks dismissal while Open is waiting for the selected pane snapshot", async () => {
  let resolveSnapshot!: (value: AppStateSnapshot) => void;
  const invoke = api.invoke.getMockImplementation()!;
  api.invoke.mockImplementation((command: string) => command === "get_app_state"
    ? new Promise<AppStateSnapshot>((resolve) => { resolveSnapshot = resolve; }) : invoke(command));
  const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
  useChatDraftStore.getState().updateDraftInput(draft.draftId, "Keep my unsent message while opening");
  useChatDraftStore.getState().setActiveDraft(draft.draftId);
  const preserved = useChatDraftStore.getState().draftsById;
  render(<NavigationSurface />);
  const open = await importOne();
  fireEvent.click(open);
  await act(async () => {});
  expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
  expect(useChatDraftStore.getState().draftsById).toBe(preserved);
  expect(screen.getByRole("button", { name: "Done" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
  expect(open).toBeDisabled();
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  fireEvent.pointerDown(document.body);
  expect(useUIStore.getState().showLocalSessionImport).toBe(true);
  await act(async () => resolveSnapshot(opened));
  expect(await screen.findByText("Imported conversation · Read-only copy")).toBeInTheDocument();
  expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  expect(useChatDraftStore.getState().draftsById).toBe(preserved);
});

it.each([
  ["snapshot fetch fails", () => Promise.reject(new Error("Snapshot unavailable")), "Snapshot unavailable"],
  ["workspace has not arrived", () => Promise.resolve(snapshot(false)), "selection is not available"],
  ["returned pane is not selected", () => Promise.resolve({ ...opened, workspaces: [{ ...workspace, surfaces: [{ ...workspace.surfaces[0], active_pane_id: "another-pane" }] }] }), "selection is not available"],
] as const)("keeps unsent drafts and allows retry when %s", async (_name, readState, error) => {
  const invoke = api.invoke.getMockImplementation()!;
  api.invoke.mockImplementation((command: string) => command === "get_app_state" ? readState() : invoke(command));
  const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
  useChatDraftStore.getState().updateDraftInput(draft.draftId, "Keep this message after navigation fails");
  useChatDraftStore.getState().setActiveDraft(draft.draftId);
  const preserved = useChatDraftStore.getState().draftsById;
  render(<NavigationSurface />);
  fireEvent.click(await importOne());
  expect(await screen.findByRole("alert")).toHaveTextContent(error);
  expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
  expect(useChatDraftStore.getState().draftsById).toBe(preserved);
  expect(useUIStore.getState().showLocalSessionImport).toBe(true);
  expect(useUIStore.getState().showSettings).toBe(true);
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled();
  expect(screen.getByRole("button", { name: "Open Fix the parser" })).toBeEnabled();
});

it("does not dismiss a draft after a newer workspace selection supersedes the Open readback", async () => {
  let resolveSnapshot!: (value: AppStateSnapshot) => void;
  const invoke = api.invoke.getMockImplementation()!;
  api.invoke.mockImplementation((command: string) => command === "get_app_state"
    ? new Promise<AppStateSnapshot>((resolve) => { resolveSnapshot = resolve; }) : invoke(command));
  const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
  useChatDraftStore.getState().setActiveDraft(draft.draftId);
  render(<NavigationSurface />);
  fireEvent.click(await importOne());
  await act(async () => {});
  const newerWorkspace = { ...workspace, workspace_id: "newer-workspace" };
  act(() => useAppStore.getState().setAppState({ ...opened, snapshot_revision: 4,
    active_workspace_id: newerWorkspace.workspace_id, workspaces: [workspace, newerWorkspace] }));
  await act(async () => resolveSnapshot(opened));
  expect(await screen.findByRole("alert")).toHaveTextContent("selection is not available");
  expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
  expect(useAppStore.getState().appState?.active_workspace_id).toBe(newerWorkspace.workspace_id);
});

it("never creates an empty Home draft when opening from an unsent project draft", async () => {
  const draft = useChatDraftStore.getState().getOrCreateProjectDraft("/demo/other");
  useChatDraftStore.getState().updateDraftInput(draft.draftId, "Keep the only unsent project message");
  useChatDraftStore.getState().setActiveDraft(draft.draftId);
  const preserved = useChatDraftStore.getState().draftsById;
  render(<NavigationSurface />);
  fireEvent.click(await importOne());
  await act(async () => {});
  expect(await screen.findByText("Imported conversation · Read-only copy")).toBeInTheDocument();
  expect(await screen.findByText("Please fix the parser from the saved conversation.")).toBeInTheDocument();
  expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  expect(useChatDraftStore.getState().activeHomeDraftId).toBeNull();
  expect(useChatDraftStore.getState().draftsById).toBe(preserved);
});

it("reveals the saved readonly transcript when Open returns before its app-state event", async () => {
  const drafts = useChatDraftStore.getState();
  const home = drafts.getOrCreateHomeDraft();
  drafts.updateDraftInput(home.draftId, "Do not lose my Home message");
  const project = drafts.getOrCreateProjectDraft("/demo/other");
  drafts.updateDraftInput(project.draftId, "Do not lose my project message");
  drafts.setActiveDraft(home.draftId);
  const preserved = useChatDraftStore.getState().draftsById;
  render(<NavigationSurface />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Fix the parser/ }));
  fireEvent.click(screen.getByRole("button", { name: "Import selected (1)" }));
  fireEvent.click(await screen.findByRole("button", { name: "Open Fix the parser" }));
  // No emitted app-state has arrived yet. Open must obtain/apply a real
  // selection receipt before removing the draft overlay.
  await act(async () => {});
  expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  expect(await screen.findByText("Imported conversation · Read-only copy")).toBeInTheDocument();
  expect(await screen.findByText("Please fix the parser from the saved conversation.")).toBeInTheDocument();
  expect(await screen.findByText("The saved parser fix is in the transcript.")).toBeInTheDocument();
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  expect(useChatDraftStore.getState().draftsById).toBe(preserved);
  expect(useUIStore.getState().showLocalSessionImport).toBe(false);
  expect(useUIStore.getState().showSettings).toBe(false);
  // The later event must not put a draft back over the imported pane.
  act(() => useAppStore.getState().setAppState({ ...opened, snapshot_revision: 3 }));
  expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  expect(useChatDraftStore.getState().draftsById).toBe(preserved);
  expect(api.invoke).not.toHaveBeenCalledWith("agent_chat_create_pane", expect.anything());
});
