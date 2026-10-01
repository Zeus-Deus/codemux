/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { LocalSessionImport } from "./LocalSessionImport";
import { useUIStore } from "@/stores/ui-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useAppStore } from "@/stores/app-store";
const api = vi.hoisted(() => ({ scan: vi.fn(), importSessions: vi.fn(), open: vi.fn(), readState: vi.fn() }));
vi.mock("@/tauri/commands", () => ({ agentChatScanLocalSessions: api.scan, agentChatImportLocalSessions: api.importSessions, agentChatOpenSearchResult: api.open, getAppState: api.readState }));
afterEach(cleanup);
beforeEach(() => {
  vi.clearAllMocks();
  useFeatureFlags.setState({ enableAgentChat: true });
  useUIStore.setState({ showLocalSessionImport: true });
  useAppStore.setState({ appState: null, pendingActiveWorkspaceId: null, lastSeenRevision: 0, deltaBuffer: new Map() });
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
});
afterEach(() => { delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__; });
const sessions = [
  { source_id: "claude:one", provider: "claude", title: "Fix the parser", cwd: "/demo/parser", last_active_at: "2026-09-29T10:00:00Z", message_count: 4, already_imported: false },
  { source_id: "codex:two", provider: "codex", title: "Review tests", cwd: "/demo/tests", last_active_at: "2026-09-28T10:00:00Z", message_count: 2, already_imported: false },
  { source_id: "codex:old", provider: "codex", title: "Previous copy", cwd: "/demo/tests", last_active_at: "2026-09-28T10:00:00Z", message_count: 2, already_imported: true },
];
it("reviews unselected sessions and imports only selected rows", async () => {
  api.scan.mockResolvedValue({ sessions, warnings: ["One unreadable file was skipped."] });
  api.importSessions.mockResolvedValue({ imported: [{ source_id: "claude:one", thread_id: "imported-1", workspace_id: "ws-1" }], skipped: 0, warnings: [] });
  render(<LocalSessionImport />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  const first = await screen.findByRole("checkbox", { name: /Fix the parser/ });
  expect(first).not.toBeChecked();
  expect(screen.getByRole("checkbox", { name: /Previous copy/ })).toBeDisabled();
  expect(screen.getByText("One unreadable file was skipped.")).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Import selected (0)" })).toBeDisabled();
  fireEvent.click(first);
  fireEvent.click(screen.getByRole("button", { name: "Import selected (1)" }));
  expect(await screen.findByText("Imported 1 conversation.")).toBeInTheDocument();
  expect(api.importSessions).toHaveBeenCalledExactlyOnceWith(["claude:one"]);
  expect(api.open).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Done" }));
  expect(useUIStore.getState().showLocalSessionImport).toBe(false);
});
it("shows discovery progress, prevents duplicate scans, and retries empty results", async () => {
  let resolve!: (value: unknown) => void;
  api.scan.mockImplementationOnce(() => new Promise((r) => { resolve = r; })).mockResolvedValueOnce({ sessions, warnings: [] });
  render(<LocalSessionImport />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  expect(screen.getByRole("status")).toHaveTextContent("Finding recent chats");
  expect(screen.getByRole("button", { name: "Finding…" })).toBeDisabled();
  await act(async () => resolve({ sessions: [], warnings: [] }));
  expect(screen.getByText("No recent chats found.")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Find again" }));
  expect(await screen.findByRole("checkbox", { name: /Fix the parser/ })).toBeInTheDocument();
  expect(api.scan).toHaveBeenCalledTimes(2);
});
it("shows recoverable scan and import errors without discarding selection", async () => {
  api.scan.mockRejectedValueOnce(new Error("Discovery denied")).mockResolvedValueOnce({ sessions, warnings: [] });
  api.importSessions.mockRejectedValueOnce(new Error("Database busy")).mockResolvedValueOnce({ imported: [], skipped: 1, warnings: ["Already imported elsewhere."] });
  render(<LocalSessionImport />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Discovery denied");
  fireEvent.click(screen.getByRole("button", { name: "Retry discovery" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Fix the parser/ }));
  fireEvent.click(screen.getByRole("button", { name: "Import selected (1)" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Database busy");
  expect(screen.getByRole("checkbox", { name: /Fix the parser/ })).toBeChecked();
  fireEvent.click(screen.getByRole("button", { name: "Import selected (1)" }));
  expect(await screen.findByText("Imported 0 conversations.")).toBeInTheDocument();
  expect(screen.getByText("Skipped 1 conversation.")).toBeInTheDocument();
});
it("locks dismissal during import and prevents duplicate imports", async () => {
  api.scan.mockResolvedValue({ sessions, warnings: [] });
  let resolve!: (value: unknown) => void;
  api.importSessions.mockImplementationOnce(() => new Promise((r) => { resolve = r; }));
  render(<LocalSessionImport />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Fix the parser/ }));
  const importButton = screen.getByRole("button", { name: "Import selected (1)" });
  fireEvent.click(importButton); fireEvent.click(importButton);
  expect(api.importSessions).toHaveBeenCalledTimes(1);
  expect(screen.getByRole("button", { name: "Not now" })).toBeDisabled();
  expect(screen.queryByRole("button", { name: "Close" })).not.toBeInTheDocument();
  fireEvent.keyDown(screen.getByRole("dialog"), { key: "Escape" });
  expect(useUIStore.getState().showLocalSessionImport).toBe(true);
  await act(async () => resolve({ imported: [], skipped: 1, warnings: [] }));
  expect(screen.getByRole("button", { name: "Done" })).toBeEnabled();
});
it("discards late discovery results after closing and reopening", async () => {
  let resolve!: (value: unknown) => void;
  api.scan.mockImplementationOnce(() => new Promise((r) => { resolve = r; }));
  render(<LocalSessionImport />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  fireEvent.click(screen.getByRole("button", { name: "Not now" }));
  act(() => useUIStore.getState().setShowLocalSessionImport(true));
  await act(async () => resolve({ sessions, warnings: [] }));
  expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Find recent chats" })).toBeEnabled();
});
it("keeps successful import visible when opening fails, and preserves unsent drafts", async () => {
  api.scan.mockResolvedValue({ sessions, warnings: [] });
  api.importSessions.mockResolvedValue({ imported: [{ source_id: "claude:one", thread_id: "imported-1", workspace_id: "ws-1" }], skipped: 0, warnings: [] });
  api.open.mockRejectedValueOnce(new Error("Navigation unavailable")).mockResolvedValueOnce({ workspace_id: "ws-1", pane_id: "pane-imported" });
  api.readState.mockResolvedValue({ active_workspace_id: "ws-1", workspaces: [{
    workspace_id: "ws-1", active_surface_id: "surface-imported", surfaces: [{
      surface_id: "surface-imported", active_pane_id: "pane-imported",
      root: { kind: "agent_chat", pane_id: "pane-imported", thread_id: "imported-1" },
    }],
  }] });
  const draft = useChatDraftStore.getState().getOrCreateHomeDraft().draftId;
  useChatDraftStore.getState().setActiveDraft(draft);
  const drafts = useChatDraftStore.getState().draftsById;
  render(<LocalSessionImport />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: /Fix the parser/ }));
  fireEvent.click(screen.getByRole("button", { name: "Import selected (1)" }));
  fireEvent.click(await screen.findByRole("button", { name: "Open Fix the parser" }));
  expect(await screen.findByRole("alert")).toHaveTextContent("Navigation unavailable");
  expect(screen.getByText("Imported 1 conversation.")).toBeInTheDocument();
  expect(useChatDraftStore.getState().activeDraftId).toBe(draft);
  fireEvent.click(screen.getByRole("button", { name: "Open Fix the parser" }));
  await act(async () => {});
  expect(api.open).toHaveBeenCalledWith("imported-1");
  expect(useChatDraftStore.getState().activeDraftId).toBeNull();
  expect(useChatDraftStore.getState().draftsById).toBe(drafts);
  expect(useUIStore.getState().showSettings).toBe(false);
});
it("selects all available rows without including already imported sessions", async () => {
  api.scan.mockResolvedValue({ sessions, warnings: [] });
  render(<LocalSessionImport />);
  fireEvent.click(screen.getByRole("button", { name: "Find recent chats" }));
  fireEvent.click(await screen.findByRole("checkbox", { name: "Select all available chats" }));
  expect(screen.getByRole("button", { name: "Import selected (2)" })).toBeEnabled();
  expect(screen.getByRole("checkbox", { name: /Previous copy/ })).not.toBeChecked();
  fireEvent.click(screen.getByRole("checkbox", { name: "Select all available chats" }));
  expect(screen.getByRole("button", { name: "Import selected (0)" })).toBeDisabled();
});
it("does not expose local discovery in unsupported clients or with chat disabled", () => {
  (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
  const view = render(<LocalSessionImport />);
  expect(screen.queryByRole("dialog")).toBeNull();
  delete (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__;
  act(() => useFeatureFlags.setState({ enableAgentChat: false }));
  view.rerender(<LocalSessionImport />);
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(api.scan).not.toHaveBeenCalled();
});
it("requires explicit discovery after explaining local copies", () => {
  render(<LocalSessionImport />);
  expect(screen.getByRole("dialog", { name: "Import recent chats" })).toBeInTheDocument();
  expect(screen.getByText(/local Codemux database/)).toBeInTheDocument();
  expect(screen.getByText(/original files remain unchanged/i)).toBeInTheDocument();
  expect(screen.getByText(/may contain secrets/i)).toBeInTheDocument();
  expect(api.scan).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole("button", { name: "Not now" }));
  expect(useUIStore.getState().showLocalSessionImport).toBe(false);
});
