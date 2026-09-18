/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useMobileNavigationStore } from "@/stores/mobile-navigation-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import type { AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

vi.mock("@/tauri/commands", () => ({
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  dbGetUiState: vi.fn().mockResolvedValue(null),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
  undockBrowserFromRightPanel: vi.fn().mockResolvedValue(undefined),
}));
vi.mock("@/stores/chat-draft-store", () => ({
  useChatDraftStore: (selector: (state: { activeDraftId: null }) => unknown) =>
    selector({ activeDraftId: null }),
}));
vi.mock("@/components/layout/workspace-main", () => ({
  // An uncontrolled input makes an accidental remount observable: its value
  // belongs to this mounted conversation, not to a mocked persistent store.
  WorkspaceMain: () => <textarea aria-label="Conversation draft" />,
}));
vi.mock("./mobile-session-sheet", () => ({
  MobileSessionSheet: ({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) =>
    open ? <section role="dialog" aria-label="Sessions"><button onClick={() => onOpenChange(false)}>Close sessions</button></section> : null,
}));
vi.mock("./mobile-workspace-actions", () => ({ MobileWorkspaceActions: () => null }));
vi.mock("./mobile-install", () => ({ MobileInstall: () => null }));
vi.mock("@/components/overlays/clone-dialog", () => ({ CloneDialog: () => null }));
vi.mock("@/components/browser/BrowserPeekOverlay", () => ({ BrowserPeekOverlay: () => null }));
vi.mock("@/components/chat/WorkspaceStatusCluster", () => ({ WorkspaceStatusCluster: () => null }));

import { MobileShell } from "./mobile-shell";

const workspace = {
  workspace_id: "mobile-workspace",
  title: "Mobile polish",
  cwd: "/projects/demo",
  project_root: "/projects/demo",
  git_branch: "mobile-polish",
  git_changed_files: 0,
  tabs: [{ tab_id: "chat-1", title: "Agent chat", kind: "agent_chat", surface_id: "surface-1", browser_id: null, icon: null }],
  active_tab_id: "chat-1",
  surfaces: [],
} as unknown as WorkspaceSnapshot;

beforeEach(() => {
  useAppStore.setState({
    appState: { active_workspace_id: workspace.workspace_id, workspaces: [workspace], pane_statuses: {} } as AppStateSnapshot,
    pendingActiveWorkspaceId: null,
    remoteActiveWorkspaceId: null,
  });
  useUIStore.setState({ rightPanelTabs: {}, rightPanelPanes: {}, showNewWorkspaceDialog: false });
  useMobileNavigationStore.setState({ home: false });
  useSidebarInboxStore.setState({ settled: [], loaded: true, load: vi.fn().mockResolvedValue(undefined) });
});
afterEach(cleanup);

describe("MobileShell conversation navigation", () => {
  it("opens Files through Tools and returns to the same conversation draft", async () => {
    render(<MobileShell overlays={null} />);
    const composer = screen.getByRole("textbox", { name: "Conversation draft" });
    fireEvent.change(composer, { target: { value: "Keep this unsent prompt" } });

    fireEvent.click(screen.getByRole("button", { name: "Workspace tools" }));
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    expect(useUIStore.getState().rightPanelTabs[workspace.workspace_id]).toBe("files");
    await waitFor(() => expect(screen.queryByRole("dialog", { name: "Workspace tools" })).not.toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Back to conversation" }));
    expect(useUIStore.getState().rightPanelTabs[workspace.workspace_id]).toBeNull();
    expect(useMobileNavigationStore.getState().home).toBe(false);
    expect(screen.getByRole("textbox", { name: "Conversation draft" })).toBe(composer);
    expect(composer).toHaveValue("Keep this unsent prompt");
    expect(screen.getByRole("button", { name: "All workspaces" })).toBeInTheDocument();
  });

  it("reveals sessions from the title without a persistent tab strip or footer", () => {
    const { container } = render(<MobileShell overlays={null} />);
    const switcher = screen.getByRole("button", { name: "Switch session" });
    expect(switcher).toHaveTextContent("Mobile polish");
    expect(switcher).toHaveTextContent("Agent chat");
    expect(switcher).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.queryByRole("navigation")).not.toBeInTheDocument();
    expect(container.querySelector("footer")).toBeNull();
    expect(screen.queryByRole("button", { name: "Files" })).not.toBeInTheDocument();

    fireEvent.click(switcher);
    expect(screen.getByRole("dialog", { name: "Sessions" })).toBeInTheDocument();
    expect(switcher).toHaveAttribute("aria-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: "Close sessions" }));
    expect(screen.queryByRole("dialog", { name: "Sessions" })).not.toBeInTheDocument();
    expect(switcher).toHaveAttribute("aria-expanded", "false");
  });

  it("keeps the mounted draft when visiting the workspace list and returning", async () => {
    render(<MobileShell overlays={null} />);
    const composer = screen.getByRole("textbox", { name: "Conversation draft" });
    fireEvent.change(composer, { target: { value: "Resume this later" } });
    fireEvent.click(screen.getByRole("button", { name: "All workspaces" }));
    expect(composer).not.toBeVisible();
    expect(composer.closest("[inert]")).not.toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /Mobile polish.*demo/ }));
    await waitFor(() => expect(composer).toBeVisible());
    expect(screen.getByRole("textbox", { name: "Conversation draft" })).toBe(composer);
    expect(composer).toHaveValue("Resume this later");
  });
});
