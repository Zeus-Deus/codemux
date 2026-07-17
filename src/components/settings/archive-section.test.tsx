/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  cleanup,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { ArchivedWorkspaceSnapshot } from "@/tauri/types";

// ── Mocks ──
//
// `vi.mock()` factories are hoisted above `import`s, so any spies they
// reference must be created via `vi.hoisted` to survive that hoist.
const { mockUnarchive, mockDeleteArchived, mockToast, mockState } = vi.hoisted(
  () => ({
    mockUnarchive: vi.fn(),
    mockDeleteArchived: vi.fn(),
    mockToast: {
      success: vi.fn(),
      error: vi.fn(),
      info: vi.fn(),
      warning: vi.fn(),
      undoable: vi.fn(),
    },
    // Mutable app-store twin; tests assign `appState`/`homeDir` per case.
    mockState: { appState: null as unknown, homeDir: null as string | null },
  }),
);

vi.mock("@/tauri/commands", () => ({
  unarchiveWorkspace: (...args: unknown[]) => mockUnarchive(...args),
  deleteArchivedWorkspace: (...args: unknown[]) =>
    mockDeleteArchived(...args),
}));

vi.mock("@/lib/toast", () => ({
  toast: mockToast,
}));

// Keep the store module's PURE helpers (resolveProjectRoot,
// projectDisplayName) real — the section shares them with the sidebar
// grouping and the tests assert on that shared labeling — while the
// stateful hooks read from the mutable twin above.
vi.mock("@/stores/app-store", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/stores/app-store")>();
  return {
    ...actual,
    useAppStore: (selector: (s: typeof mockState) => unknown) =>
      selector(mockState),
    useHomeDir: () => mockState.homeDir,
  };
});

import { ArchiveSection } from "./archive-section";

const DAY = 86_400;
const now = Math.floor(Date.now() / 1000);

function makeEntry(
  overrides: Partial<ArchivedWorkspaceSnapshot> = {},
): ArchivedWorkspaceSnapshot {
  return {
    archive_id: "arch-1",
    workspace_id: "ws-1",
    title: "feature-work",
    cwd: "/home/user/.codemux/worktrees/myapp/feature-work",
    worktree_path: "/home/user/.codemux/worktrees/myapp/feature-work",
    project_root: "/home/user/projects/myapp",
    project_uid: "uid-myapp",
    workspace_kind: "worktree",
    git_branch: "feature/work",
    protected: false,
    is_git: true,
    archived_at: now - 2 * DAY,
    ...overrides,
  };
}

function setArchived(entries: ArchivedWorkspaceSnapshot[]) {
  mockState.appState = { archived_workspaces: entries };
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  mockUnarchive.mockResolvedValue("ws-1");
  mockDeleteArchived.mockResolvedValue(undefined);
  mockState.homeDir = null;
  setArchived([]);
});

describe("ArchiveSection — empty state", () => {
  it("renders the empty message when nothing is archived", () => {
    setArchived([]);
    render(<ArchiveSection />);
    expect(screen.getByText("No archived workspaces")).toBeInTheDocument();
  });

  it("treats a snapshot without the field as empty", () => {
    mockState.appState = {};
    render(<ArchiveSection />);
    expect(screen.getByText("No archived workspaces")).toBeInTheDocument();
  });
});

describe("ArchiveSection — grouping and entry rendering", () => {
  it("groups entries by project root basename", () => {
    setArchived([
      makeEntry({ archive_id: "a-1", project_root: "/home/user/projects/myapp" }),
      makeEntry({
        archive_id: "a-2",
        title: "blog-cleanup",
        project_root: "/home/user/projects/blog",
        git_branch: "chore/cleanup",
      }),
    ]);
    render(<ArchiveSection />);

    expect(screen.getByText("myapp")).toBeInTheDocument();
    expect(screen.getByText("blog")).toBeInTheDocument();
    expect(screen.getByText("feature-work")).toBeInTheDocument();
    expect(screen.getByText("blog-cleanup")).toBeInTheDocument();
  });

  it("falls back to the cwd when project_root is null", () => {
    setArchived([
      makeEntry({ project_root: null, cwd: "/home/user/projects/loose-dir" }),
    ]);
    render(<ArchiveSection />);
    expect(screen.getByText("loose-dir")).toBeInTheDocument();
  });

  it("labels home-rooted entries 'Home' — the sidebar grouping's label rule", () => {
    // Shared `projectDisplayName` rule: a project root that IS the home
    // directory groups under "Home", not the path basename ("user").
    mockState.homeDir = "/home/user";
    setArchived([
      makeEntry({ project_root: null, cwd: "/home/user" }),
    ]);
    render(<ArchiveSection />);
    expect(screen.getByText("Home")).toBeInTheDocument();
    expect(screen.queryByText("user")).not.toBeInTheDocument();
  });

  it("shows the branch chip and relative archived time", () => {
    setArchived([makeEntry({ git_branch: "feature/work" })]);
    render(<ArchiveSection />);
    expect(screen.getByText("feature/work")).toBeInTheDocument();
    expect(screen.getByText(/Archived 2 days ago/)).toBeInTheDocument();
  });

  it("marks repo-root entries and entries older than 30 days as stale", () => {
    setArchived([
      makeEntry({
        archive_id: "a-root",
        title: "myapp",
        worktree_path: null,
        workspace_kind: "main",
        protected: true,
        archived_at: now - 45 * DAY,
      }),
    ]);
    render(<ArchiveSection />);
    expect(screen.getByText("repo root")).toBeInTheDocument();
    expect(screen.getByText("stale")).toBeInTheDocument();
  });

  it("does not show the stale hint for recent entries", () => {
    setArchived([makeEntry({ archived_at: now - 2 * DAY })]);
    render(<ArchiveSection />);
    expect(screen.queryByText("stale")).not.toBeInTheDocument();
  });
});

describe("ArchiveSection — unarchive", () => {
  it("calls unarchiveWorkspace with the archive id and toasts success", async () => {
    setArchived([makeEntry({ archive_id: "arch-77", title: "comeback" })]);
    render(<ArchiveSection />);

    await userEvent.click(screen.getByRole("button", { name: /Unarchive/i }));

    expect(mockUnarchive).toHaveBeenCalledWith("arch-77");
    await waitFor(() =>
      expect(mockToast.success).toHaveBeenCalledWith('Restored "comeback"'),
    );
  });

  it("keeps the entry and toasts an error when the restore fails", async () => {
    mockUnarchive.mockRejectedValueOnce(
      "Worktree directory no longer exists — nothing left to restore.",
    );
    setArchived([makeEntry()]);
    render(<ArchiveSection />);

    await userEvent.click(screen.getByRole("button", { name: /Unarchive/i }));

    await waitFor(() => expect(mockToast.error).toHaveBeenCalledTimes(1));
    expect(mockToast.success).not.toHaveBeenCalled();
  });
});

describe("ArchiveSection — delete", () => {
  it("protected root entries offer 'Remove from archive' with no worktree checkbox", async () => {
    setArchived([
      makeEntry({
        archive_id: "a-root",
        title: "myapp",
        worktree_path: null,
        workspace_kind: "main",
        protected: true,
      }),
    ]);
    render(<ArchiveSection />);

    // No trash-delete affordance for the root…
    expect(
      screen.queryByRole("button", { name: /Delete archived workspace/i }),
    ).not.toBeInTheDocument();

    await userEvent.click(
      screen.getByRole("button", { name: /Remove from archive/i }),
    );

    const dialog = await screen.findByRole("dialog");
    // …and the confirm dialog never offers file deletion.
    expect(
      within(dialog).queryByText(/delete worktree from disk/i),
    ).not.toBeInTheDocument();
    expect(
      within(dialog).getByText(/never touched/i),
    ).toBeInTheDocument();

    await userEvent.click(
      within(dialog).getByRole("button", { name: /^Remove$/i }),
    );
    await waitFor(() =>
      expect(mockDeleteArchived).toHaveBeenCalledWith(
        "a-root",
        false,
        false,
        false,
      ),
    );
  });

  it("worktree entries delete with the chosen worktree/branch options", async () => {
    setArchived([makeEntry({ archive_id: "a-wt" })]);
    render(<ArchiveSection />);

    await userEvent.click(
      screen.getByRole("button", { name: /Delete archived workspace/i }),
    );
    const dialog = await screen.findByRole("dialog");

    // Opt into worktree deletion; branch sub-option defaults to true.
    await userEvent.click(
      within(dialog).getByLabelText(/Also delete worktree from disk/i),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: /^Delete$/i }),
    );

    await waitFor(() =>
      expect(mockDeleteArchived).toHaveBeenCalledWith("a-wt", true, true, false),
    );
  });

  it("escalates to 'Force delete' on a /use force/i rejection", async () => {
    const dirtyMessage =
      "Worktree has 3 uncommitted change(s). Use force to override.";
    mockDeleteArchived.mockRejectedValueOnce(dirtyMessage);
    setArchived([makeEntry({ archive_id: "a-dirty" })]);
    render(<ArchiveSection />);

    await userEvent.click(
      screen.getByRole("button", { name: /Delete archived workspace/i }),
    );
    const dialog = await screen.findByRole("dialog");
    await userEvent.click(
      within(dialog).getByLabelText(/Also delete worktree from disk/i),
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: /^Delete$/i }),
    );

    // Dialog stays open showing the backend message verbatim.
    expect(await screen.findByText(dirtyMessage)).toBeInTheDocument();
    expect(mockToast.error).not.toHaveBeenCalled();

    await userEvent.click(
      screen.getByRole("button", { name: /Force delete/i }),
    );
    await waitFor(() =>
      expect(mockDeleteArchived).toHaveBeenLastCalledWith(
        "a-dirty",
        true,
        true,
        true,
      ),
    );
  });
});
