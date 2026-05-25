/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type { WorkspaceSyncView, AdoptionPreview } from "@/tauri/commands";

// ── Mocks ───────────────────────────────────────────────────────

const mockPreview = vi.fn<
  (serverId: string) => Promise<AdoptionPreview>
>();
const mockAdopt = vi.fn<(serverId: string) => Promise<unknown>>();
const mockActivate = vi.fn();

vi.mock("@/tauri/commands", () => ({
  workspacesAdoptionPreview: (id: string) => mockPreview(id),
  workspacesAdoptSynced: (id: string) => mockAdopt(id),
  activateWorkspace: (id: string) => {
    mockActivate(id);
    return Promise.resolve();
  },
}));

vi.mock("@/stores/app-store", () => ({
  useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      setWorkspacePushPullInFlight: vi.fn(),
    }),
  ),
}));

vi.mock("@/stores/ui-store", () => ({
  useUIStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      setShowWorkspacesOverview: vi.fn(),
    }),
  ),
}));

vi.mock("@/stores/workspaces-sync-store", () => ({
  useWorkspacesSyncStore: vi.fn((selector: (s: unknown) => unknown) =>
    selector({
      refresh: () => Promise.resolve(),
    }),
  ),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
  },
}));

import { PullToDeviceDialog } from "./pull-to-device-dialog";

function makeSyncRow(
  overrides?: Partial<WorkspaceSyncView>,
): WorkspaceSyncView {
  return {
    id: 1,
    server_id: "42",
    workspace_id: null,
    title: "codemux/feature-x",
    host_server_id: "7",
    project_path: "/home/zeus/projects/codemux",
    project_remote: "git@github.com:Zeus-Deus/codemux.git",
    git_branch: "feature-x",
    created_at: "2026-05-20T10:00:00Z",
    updated_at: "2026-05-20T10:00:00Z",
    dirty: false,
    ...overrides,
  };
}

function makePreview(
  overrides?: Partial<AdoptionPreview>,
): AdoptionPreview {
  return {
    can_host_adopt: true,
    can_clone_adopt: true,
    host_configured: true,
    host_label: "homedesk",
    project_already_cloned_at: null,
    suggested_path: "/home/zeus/.codemux/worktrees/codemux/feature-x",
    is_path_in_use: false,
    already_adopted_workspace_id: null,
    ...overrides,
  };
}

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
  try {
    localStorage.clear();
  } catch {
    /* test env may not have localStorage */
  }
});

describe("PullToDeviceDialog", () => {
  beforeEach(() => {
    mockPreview.mockReset();
    mockAdopt.mockReset();
    mockActivate.mockReset();
  });

  it("does not render when syncRow is null", () => {
    const { container } = render(
      <PullToDeviceDialog syncRow={null} onOpenChange={() => {}} />,
    );
    expect(container.firstChild).toBeNull();
    expect(mockPreview).not.toHaveBeenCalled();
  });

  it("renders the host-backed summary when preview returns can_host_adopt", async () => {
    mockPreview.mockResolvedValue(makePreview());
    render(
      <PullToDeviceDialog
        syncRow={makeSyncRow()}
        onOpenChange={() => {}}
      />,
    );
    await waitFor(() => expect(mockPreview).toHaveBeenCalledWith("42"));
    // Radix Dialog portals content to document.body — use `screen`
    // rather than the container returned by `render`.
    await waitFor(() =>
      expect(screen.getAllByText(/homedesk/).length).toBeGreaterThan(0),
    );
    expect(screen.getByText("feature-x")).toBeInTheDocument();
    expect(
      screen.getByText("/home/zeus/.codemux/worktrees/codemux/feature-x"),
    ).toBeInTheDocument();
    // "What this does" should be pre-expanded on first pull
    // (localStorage flag absent).
    expect(
      screen.getByText(/Copies the workspace files from/i),
    ).toBeInTheDocument();
    // Pull button is enabled.
    expect(screen.queryByText("Pull workspace")).toBeInTheDocument();
  });

  it("renders 'configure the device first' when host isn't on this device", async () => {
    mockPreview.mockResolvedValue(
      makePreview({
        can_host_adopt: false,
        host_configured: false,
        host_label: null,
      }),
    );
    render(
      <PullToDeviceDialog
        syncRow={makeSyncRow()}
        onOpenChange={() => {}}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByText(/haven't configured here yet/i),
      ).toBeInTheDocument(),
    );
    // Pull button is NOT shown when host is missing.
    expect(screen.queryByText("Pull workspace")).toBeNull();
  });

  it("renders 'path in use' when the suggested path collides with a local workspace", async () => {
    mockPreview.mockResolvedValue(
      makePreview({ is_path_in_use: true }),
    );
    render(
      <PullToDeviceDialog
        syncRow={makeSyncRow()}
        onOpenChange={() => {}}
      />,
    );
    await waitFor(() =>
      expect(screen.getByText(/already using/i)).toBeInTheDocument(),
    );
    expect(screen.queryByText("Pull workspace")).toBeNull();
  });

  it("offers 'Open it' when the row is already adopted on this device", async () => {
    mockPreview.mockResolvedValue(
      makePreview({ already_adopted_workspace_id: "workspace-42" }),
    );
    render(
      <PullToDeviceDialog
        syncRow={makeSyncRow()}
        onOpenChange={() => {}}
      />,
    );
    await waitFor(() =>
      expect(
        screen.getByText(/already pulled this workspace/i),
      ).toBeInTheDocument(),
    );
    const openBtn = screen.getByText("Open it");
    fireEvent.click(openBtn);
    expect(mockActivate).toHaveBeenCalledWith("workspace-42");
  });

  it("calls workspacesAdoptSynced and closes the dialog on submit", async () => {
    mockPreview.mockResolvedValue(makePreview());
    mockAdopt.mockResolvedValue({
      workspace_id: "workspace-99",
      worktree_path: "/home/zeus/.codemux/worktrees/codemux/feature-x",
      message: "Workspace pulled back from homedesk",
    });
    const onOpenChange = vi.fn();
    render(
      <PullToDeviceDialog
        syncRow={makeSyncRow()}
        onOpenChange={onOpenChange}
      />,
    );
    await waitFor(() => expect(screen.getByText("Pull workspace")).toBeInTheDocument());

    fireEvent.click(screen.getByText("Pull workspace"));

    // The dialog closes immediately (optimistic).
    expect(onOpenChange).toHaveBeenCalledWith(false);
    await waitFor(() => expect(mockAdopt).toHaveBeenCalledWith("42"));
  });

  it("collapses 'What this does' on subsequent pulls once the flag is set", async () => {
    try {
      localStorage.setItem("codemux.workspaces.firstPullDone.default", "1");
    } catch {
      // skip if localStorage unavailable
    }
    mockPreview.mockResolvedValue(makePreview());
    render(
      <PullToDeviceDialog
        syncRow={makeSyncRow()}
        onOpenChange={() => {}}
      />,
    );
    // Wait for the preview-driven render. "Will land" path is a
    // good marker — only the host-backed form variant renders it.
    await waitFor(() =>
      expect(
        screen.getByText(
          "/home/zeus/.codemux/worktrees/codemux/feature-x",
        ),
      ).toBeInTheDocument(),
    );
    // The detail bullets should NOT be visible by default this time.
    expect(screen.queryByText(/Copies the workspace files from/i)).toBeNull();
    // The toggle is still there.
    expect(screen.getByText("What this does")).toBeInTheDocument();
  });
});
