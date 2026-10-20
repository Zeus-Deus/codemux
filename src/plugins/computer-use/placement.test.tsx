/// <reference types="@testing-library/jest-dom/vitest" />
import { act, cleanup, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, it, vi } from "vitest";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { TooltipProvider } from "@/components/ui/tooltip";
import { WorkspaceDetailsHost } from "@/components/layout/workspace-details/workspace-details-panel";
import { PluginPeekOverlay } from "@/components/peek/PluginPeekOverlay";
import { registry } from "@/contrib/registry";
import { useBrowserPeekStore, DEFAULT_PEEK_SIZE } from "@/stores/browser-peek-store";
import { useUIStore } from "@/stores/ui-store";
import { COMPOSER_STATUS_AREA, PEEK_AREA, host, type PeekRenderProps } from "@codemux/sdk";
import { ComputerUseController } from "./controller";
import { ComputerChatChip, ComputerPreview } from "./views";

const state = vi.hoisted(() => ({ workspace: null as WorkspaceSnapshot | null }));
vi.mock("@/stores/app-store", async (original) => ({
  ...await original<typeof import("@/stores/app-store")>(),
  useActiveWorkspace: () => state.workspace,
  useActiveWorkspaceId: () => "demo",
  useHomeDir: () => "/home/demo",
}));
vi.mock("@/components/browser/background-browser-indicator", () => ({ useBackgroundBrowserSession: () => null }));
vi.mock("@/hooks/use-editor-launcher", () => ({ useEditorLauncher: () => ({ editors: [], unavailable: true }) }));
vi.mock("@/components/layout/run-button", () => ({ RunButton: () => null }));
vi.mock("@/stores/hosts-store", () => ({ useHosts: () => [] }));
vi.mock("@/stores/local-device-store", () => ({ useLocalDeviceName: () => "Demo device" }));
vi.mock("@/lib/toast", () => ({ toast: { error: vi.fn(), info: vi.fn(), success: vi.fn() } }));
vi.mock("@/sdk/host", async (original) => {
  const actual = await original<typeof import("@/sdk/host")>();
  return { ...actual, host: { ...actual.host, peek: { ...actual.host.peek, toggle: (id: string) => {
    useBrowserPeekStore.getState().openPlugin(id, "demo");
    return true;
  } } } };
});

afterEach(() => { cleanup(); vi.restoreAllMocks(); });

it("opens the real Computer preview from Workspace details Live above the composer", async () => {
  state.workspace = {
    workspace_id: "demo", title: "Demo", workspace_type: "standard", cwd: "/demo",
    git_branch: null, git_ahead: 0, git_behind: 0, git_additions: 0, git_deletions: 0, git_changed_files: 0,
    notification_count: 0, latest_agent_state: null, worktree_path: null, project_root: "/demo",
    pr_number: null, pr_state: null, pr_url: null, linked_issue: null, notifications_muted: false,
    tabs: [{ tab_id: "tab-demo", kind: "terminal", title: "Chat", surface_id: "surface-demo", browser_id: null, icon: null }],
    active_tab_id: "tab-demo", active_surface_id: "surface-demo",
    surfaces: [{ surface_id: "surface-demo", title: "Chat", active_pane_id: "pane-demo", root: {
      kind: "agent_chat", pane_id: "pane-demo", title: "Chat", thread_id: "thread-demo", provider: "claude", cwd: "/demo",
    } }],
  };
  useUIStore.setState({ workspaceDetailsOpen: true, workspaceDetailsMode: "pinned", workspaceDetailsPopoverOpen: false });
  useBrowserPeekStore.setState({ pluginPeek: null, openWorkspaceId: null, size: DEFAULT_PEEK_SIZE });
  vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (this: HTMLElement) {
    if (this.hasAttribute("data-composer-dock")) return new DOMRect(1000, 980, 484, 86);
    if (this.querySelector(":scope > [role=dialog]")) return new DOMRect(0, 0, 1920, 1080);
    return new DOMRect();
  });
  const controller = new ComputerUseController({ ...host, registerMcpServer: async () => async () => {} });
  vi.spyOn(controller, "getState").mockReturnValue({ ...controller.getState(),
    workspace: { id: "demo", title: "Demo", local: true }, optedIn: true, phase: "active",
    target: { pid: 42, windowId: 123, title: "Demo Counter", appName: "Counter" },
    frame: { kind: "unavailable", message: "This application did not provide a window image." },
  });
  const offStatus = registry.register({ id: "computer-use:status", source: "plugin:computer-use", area: COMPOSER_STATUS_AREA,
    render: ({ workspaceId }: { workspaceId: string | null }) => <ComputerChatChip controller={controller} workspaceId={workspaceId} /> });
  const offPeek = registry.register({ id: "computer-use:live", source: "plugin:computer-use", area: PEEK_AREA, title: "Computer use",
    render: (props: PeekRenderProps) => <ComputerPreview {...props} controller={controller} /> });
  try {
    render(<TooltipProvider>
      <WorkspaceDetailsHost workspaceId="demo" columnWidth={1920} rightInset={0} edgeGap={4} remPx={16} />
      <div data-composer-dock />
      <PluginPeekOverlay />
    </TooltipProvider>);
    const live = screen.getByTestId("workspace-details-live");
    const chip = within(live).getByRole("button", { name: "View Computer preview" });
    expect(screen.getAllByRole("button", { name: "View Computer preview" })).toHaveLength(1);
    await userEvent.click(chip);
    const dialog = screen.getByRole("dialog", { name: "Computer use preview" });
    await act(async () => { await new Promise((resolve) => requestAnimationFrame(resolve)); });
    expect(dialog).toHaveClass("right-3.5");
    expect(dialog).not.toHaveClass("top-3.5");
    expect(dialog.style.bottom).toBe("112px");
    expect(within(dialog).getByText("No window image")).toBeVisible();
    expect(within(dialog).getByRole("button", { name: "Stop computer use in live view" })).toBeEnabled();
  } finally {
    cleanup(); offPeek(); offStatus();
    useBrowserPeekStore.getState().closeAll();
    await controller.dispose();
  }
});
