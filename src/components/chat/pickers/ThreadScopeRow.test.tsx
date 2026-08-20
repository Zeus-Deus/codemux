/// <reference types="@testing-library/jest-dom/vitest" />
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { BranchDetail, WorkspaceSnapshot } from "@/tauri/types";

// ── App-store mock — keep
// the real grouping helper, stub the store hooks against a
// test-controlled workspace list. ──
let currentWorkspaces: WorkspaceSnapshot[] = [];

function makeWs(overrides: Partial<WorkspaceSnapshot>): WorkspaceSnapshot {
  return {
    workspace_id: "ws-default",
    title: "ws",
    workspace_type: "standard",
    cwd: "/projects/foo",
    git_branch: null,
    git_ahead: 0,
    git_behind: 0,
    git_additions: 0,
    git_deletions: 0,
    git_changed_files: 0,
    notification_count: 0,
    notifications_muted: false,
    latest_agent_state: null,
    worktree_path: null,
    project_root: "/projects/foo",
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  } as WorkspaceSnapshot;
}

vi.mock("@/stores/app-store", async () => {
  const actual =
    await vi.importActual<typeof import("@/stores/app-store")>(
      "@/stores/app-store",
    );
  return {
    ...actual,
    useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
      selector({ appState: { workspaces: currentWorkspaces } }),
    ),
    useHomeDir: () => "/home/user",
    useProjectGroupedWorkspaces: actual.useProjectGroupedWorkspaces,
    resolveProjectRoot: actual.resolveProjectRoot,
  };
});

vi.mock("@/stores/hosts-store", () => ({
  useHosts: () => [],
}));

const mockOpenProject = vi.fn();
vi.mock("@/hooks/use-project-actions", () => ({
  useProjectActions: () => ({ openProject: mockOpenProject }),
}));

vi.mock("@/tauri/commands", () => ({
  dbGetUiState: vi.fn().mockResolvedValue(null),
  dbSetUiState: vi.fn().mockResolvedValue(undefined),
  listBranchesDetailed: vi.fn(),
  // Probe fallback used by ThreadScopeRow when no workspace row carries
  // the project's `is_git` flag. Defaults to true (git repo) so the
  // checkout/branch controls render as they did pre-probe; individual
  // tests override it to exercise the non-git path.
  checkIsGitRepo: vi.fn().mockResolvedValue(true),
}));

import { ThreadScopeRow, type ThreadScopeRowProps } from "./ThreadScopeRow";
import {
  dbGetUiState,
  dbSetUiState,
  listBranchesDetailed,
} from "@/tauri/commands";
import {
  SETTLED_UI_STATE_KEY,
  __resetSidebarInboxStoreForTests,
  type SettledEntry,
  type SnoozeEntry,
} from "@/stores/sidebar-inbox-store";
import { SETTLED_COLLAPSED_COUNT } from "./project-scope-list";

/** Seed the persisted sidebar-inbox blob the picker loads on mount. Keyed so
 *  the per-project avatar reads (same command, different keys) still get null. */
function seedInbox(opts: {
  settled?: SettledEntry[];
  snoozed?: SnoozeEntry[];
  activity?: Record<string, number>;
}) {
  const blob = JSON.stringify({
    settled: opts.settled ?? [],
    snoozed: opts.snoozed ?? [],
    keepActive: [],
    activity: opts.activity ?? {},
  });
  vi.mocked(dbGetUiState).mockImplementation((key: string) =>
    Promise.resolve(key === SETTLED_UI_STATE_KEY ? blob : null),
  );
}

/** Project roots listed in the open "Run in" popover, in DOM order. Reads the
 *  row's `data-project-path` rather than its text, which is prefixed by the
 *  avatar's initial glyph. Excludes the pinned Home row and the "Show N more"
 *  affordance, neither of which carries the attribute. */
function listedProjects(): string[] {
  return Array.from(
    document.querySelectorAll<HTMLElement>("[data-project-path]"),
  ).map((el) => el.dataset.projectPath ?? "");
}

afterEach(() => cleanup());

const NOW = Math.floor(Date.now() / 1000);

function branch(name: string, overrides: Partial<BranchDetail> = {}): BranchDetail {
  return {
    name,
    last_commit_unix: NOW - 3600,
    is_local: true,
    is_remote: false,
    is_head: false,
    ...overrides,
  };
}

function renderRow(
  overrides: Partial<ThreadScopeRowProps> & {
    draftTarget?: import("@/stores/chat-draft-store").DraftTarget;
  } = {},
) {
  const onChangeTarget = vi.fn();
  const onChangeCheckoutMode = vi.fn();
  const onChangeWorktreeName = vi.fn();
  const onChangeBaseBranch = vi.fn();
  const { draftTarget, checkoutMode, worktreeName, baseBranch, ...rest } =
    overrides;
  const props: ThreadScopeRowProps = {
    target: draftTarget ?? { kind: "project", projectPath: "/projects/foo" },
    onChangeTarget,
    projectPath: "/projects/foo",
    checkoutMode: checkoutMode ?? "current",
    worktreeName: worktreeName ?? "",
    baseBranch: baseBranch ?? "main",
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
    ...rest,
  };
  const utils = render(<ThreadScopeRow {...props} />);
  return {
    ...utils,
    onChangeTarget,
    onChangeCheckoutMode,
    onChangeWorktreeName,
    onChangeBaseBranch,
  };
}

/** Controlled harness: the real composer stores `checkoutMode` /
 *  `baseBranch` in the draft and feeds them back down, so the pill shows
 *  whatever the row seeds. `renderRow` freezes those props, which is fine
 *  for click assertions but hides the seeding effect's result. */
function renderControlled(
  initial: {
    checkoutMode?: "current" | "worktree";
    baseBranch?: string;
  } = {},
) {
  const onChangeCheckoutMode = vi.fn();
  const onChangeBaseBranch = vi.fn();
  function Harness() {
    const [checkoutMode, setCheckoutMode] = useState<"current" | "worktree">(
      initial.checkoutMode ?? "current",
    );
    const [baseBranch, setBaseBranch] = useState(initial.baseBranch ?? "");
    return (
      <ThreadScopeRow
        target={{ kind: "project", projectPath: "/projects/foo" }}
        onChangeTarget={vi.fn()}
        projectPath="/projects/foo"
        checkoutMode={checkoutMode}
        worktreeName=""
        baseBranch={baseBranch}
        onChangeCheckoutMode={(mode) => {
          onChangeCheckoutMode(mode);
          setCheckoutMode(mode);
        }}
        onChangeWorktreeName={vi.fn()}
        onChangeBaseBranch={(name) => {
          onChangeBaseBranch(name);
          setBaseBranch(name);
        }}
      />
    );
  }
  const utils = render(<Harness />);
  return { ...utils, onChangeCheckoutMode, onChangeBaseBranch };
}

describe("ThreadScopeRow", () => {
  beforeEach(() => {
    currentWorkspaces = [];
    __resetSidebarInboxStoreForTests();
    vi.mocked(dbGetUiState).mockReset().mockResolvedValue(null);
    vi.mocked(dbSetUiState).mockReset().mockResolvedValue(undefined);
    mockOpenProject.mockReset().mockResolvedValue({ success: false });
    vi.mocked(listBranchesDetailed).mockReset().mockResolvedValue([
      branch("main", { last_commit_unix: NOW - 3600 }),
      branch("develop", { last_commit_unix: NOW - 86400 }),
    ]);
  });

  describe("home target", () => {
    it("renders only the location control — no checkout/branch controls", () => {
      renderRow({ draftTarget: { kind: "home" }, projectPath: null });
      expect(screen.getByText("Home")).toBeInTheDocument();
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(screen.queryByText("New worktree")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
    });
  });

  describe("project target — current checkout", () => {
    it("renders location, checkout, and branch controls", () => {
      renderRow();
      expect(screen.getByText("foo")).toBeInTheDocument();
      expect(screen.getByText("Current checkout")).toBeInTheDocument();
      expect(screen.getByText("main")).toBeInTheDocument();
    });

    it("renders the worktree checkout control when checkoutMode is 'worktree'", () => {
      renderRow({ checkoutMode: "worktree", baseBranch: "develop" });
      expect(screen.getByText("New worktree")).toBeInTheDocument();
    });

    it("hides checkout/branch controls when projectPath hasn't resolved yet", () => {
      renderRow({
        draftTarget: {
          kind: "existing_workspace",
          workspaceId: "ws-not-here",
        },
        projectPath: null,
      });
      expect(screen.queryByText("Current checkout")).toBeNull();
      expect(screen.queryByText(/^from$/)).toBeNull();
    });
  });

  describe("location control", () => {
    it("selecting Home from the location popover calls onChangeTarget({kind: 'home'})", async () => {
      const user = userEvent.setup();
      const { onChangeTarget } = renderRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      await user.click(screen.getByText("Home directory (~)"));
      expect(onChangeTarget).toHaveBeenCalledWith({ kind: "home" });
    });

    it("lists known projects and selecting one calls onChangeTarget({kind:'project', projectPath})", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-bar",
          cwd: "/projects/bar",
          project_root: "/projects/bar",
        }),
      ];
      const user = userEvent.setup();
      const { onChangeTarget } = renderRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      await user.click(screen.getByText("bar"));
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/bar",
      });
    });

    it("'Open another project…' calls openProject and forwards the picked path", async () => {
      mockOpenProject.mockResolvedValue({
        success: true,
        path: "/projects/opened",
        name: "opened",
      });
      const user = userEvent.setup();
      const { onChangeTarget } = renderRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Open another project…");
      await user.click(screen.getByText("Open another project…"));
      await waitFor(() => expect(mockOpenProject).toHaveBeenCalled());
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/opened",
      });
    });
  });

  describe("location control — type-to-filter", () => {
    /** Three sibling projects, so a query has something to narrow. */
    function seedProjects() {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-bar",
          cwd: "/projects/bar",
          project_root: "/projects/bar",
        }),
        makeWs({
          workspace_id: "ws-codemux",
          cwd: "/projects/codemux",
          project_root: "/projects/codemux",
        }),
        makeWs({
          workspace_id: "ws-site",
          cwd: "/projects/codemux-sitev2",
          project_root: "/projects/codemux-sitev2",
        }),
      ];
    }

    async function openPicker() {
      const user = userEvent.setup();
      const rendered = renderRow();
      await user.click(screen.getByText("foo"));
      const input = await screen.findByPlaceholderText("Search projects…");
      return { user, input, ...rendered };
    }

    it("focuses the search input when the popover opens", async () => {
      seedProjects();
      const { input } = await openPicker();
      await waitFor(() => expect(input).toHaveFocus());
    });

    it("types straight into the focused input — no click needed first", async () => {
      seedProjects();
      const { user, input } = await openPicker();
      // Deliberately NO click: keystrokes go wherever focus already is.
      // The popover's open-autofocus must land on the input itself — cmdk's
      // root only handles navigation keys, so if focus sat anywhere else
      // these printable keystrokes would go nowhere.
      await user.keyboard("codemux");
      expect(input).toHaveValue("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      expect(screen.getByText("codemux")).toBeInTheDocument();
    });

    it("narrows the list to fuzzy matches and hides the rest", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      expect(screen.getByText("codemux")).toBeInTheDocument();
      expect(screen.getByText("codemux-sitev2")).toBeInTheDocument();
      expect(screen.queryByText("Home directory (~)")).toBeNull();
    });

    it("switches to path matching once the query contains a slash", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-a",
          cwd: "/work/alpha/app",
          project_root: "/work/alpha/app",
        }),
        makeWs({
          workspace_id: "ws-b",
          cwd: "/work/beta/app",
          project_root: "/work/beta/app",
        }),
      ];
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("beta/");
      await user.keyboard("{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/work/beta/app",
      });
    });

    it("does not let long paths defeat name filtering", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-deep",
          cwd: "/home/user/dev/scratch/bar",
          project_root: "/home/user/dev/scratch/bar",
        }),
        makeWs({
          workspace_id: "ws-vexis",
          cwd: "/projects/vexis",
          project_root: "/projects/vexis",
        }),
      ];
      const { user } = await openPicker();
      // "ve" is a subsequence of `/home/user/dev/…` — name-only
      // matching is what keeps that row out.
      await user.keyboard("ve");
      await waitFor(() => expect(screen.getByText("vexis")).toBeInTheDocument());
      expect(screen.queryByText("bar")).toBeNull();
    });

    it("does not let a query grazing the shared path prefix match every row", async () => {
      // Every row's path starts with "/projects/" — because a slash-less
      // query matches display names only, typing the shared prefix must
      // match NOTHING rather than everything at the same score.
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("projects");
      await waitFor(() => expect(listedProjects()).toEqual([]));
      expect(screen.getByText(/No projects match/)).toBeInTheDocument();
    });

    it("matches on scattered characters, not just prefixes", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("cdx");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      expect(screen.getByText("codemux")).toBeInTheDocument();
    });

    it("Enter picks the top match without touching the mouse", async () => {
      seedProjects();
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      await user.keyboard("{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/codemux",
      });
    });

    it("arrow keys move the highlight before Enter commits", async () => {
      seedProjects();
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      await user.keyboard("{ArrowDown}{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({
        kind: "project",
        projectPath: "/projects/codemux-sitev2",
      });
    });

    it("keeps Home reachable by name", async () => {
      seedProjects();
      const { user, onChangeTarget } = await openPicker();
      await user.keyboard("home");
      await waitFor(() => expect(screen.queryByText("codemux")).toBeNull());
      await user.keyboard("{Enter}");
      expect(onChangeTarget).toHaveBeenCalledWith({ kind: "home" });
    });

    it("shows an empty state but keeps the open-project escape hatch", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("zzzz");
      await waitFor(() =>
        expect(screen.getByText(/No projects match/)).toBeInTheDocument(),
      );
      expect(screen.getByText("Open another project…")).toBeInTheDocument();
    });

    it("resets the query so the next open starts from the full list", async () => {
      seedProjects();
      const { user } = await openPicker();
      await user.keyboard("codemux");
      await waitFor(() => expect(screen.queryByText("bar")).toBeNull());
      await user.keyboard("{Escape}");
      await user.click(screen.getByText("foo"));
      expect(await screen.findByText("bar")).toBeInTheDocument();
    });
  });

  // The picker lists every project that has a live workspace, which on a
  // long-lived install is a lot of projects — settling a workspace parks its
  // sidebar card but closes nothing, so it stays a valid "Run in" target
  // forever. These cover the sectioning that keeps that list legible.
  describe("location picker — Active / Settled sections", () => {
    function projectWs(name: string, id?: string) {
      return makeWs({
        workspace_id: id ?? `ws-${name}`,
        cwd: `/projects/${name}`,
        project_root: `/projects/${name}`,
      });
    }

    async function openPicker() {
      const user = userEvent.setup();
      renderRow();
      await user.click(screen.getByText("foo"));
      await screen.findByText("Run in");
      return user;
    }

    it("stays a flat, heading-free list when nothing is settled", async () => {
      currentWorkspaces = [projectWs("foo"), projectWs("bar")];
      await openPicker();
      expect(screen.queryByText("Active")).toBeNull();
      expect(screen.queryByText(/Settled/)).toBeNull();
    });

    it("splits settled projects into their own labelled section", async () => {
      currentWorkspaces = [projectWs("foo"), projectWs("bar")];
      seedInbox({ settled: [{ id: "ws-bar", at: 1_000 }] });
      await openPicker();
      await waitFor(() => expect(screen.getByText("Active")).toBeInTheDocument());
      // The heading says the part a bare "Settled" label would leave the user
      // guessing about — these projects are parked, not closed.
      expect(screen.getByText(/still open/)).toBeInTheDocument();
      expect(screen.getByText("bar")).toBeInTheDocument();
    });

    it("keeps a project Active while any of its worktrees is unsettled", async () => {
      currentWorkspaces = [
        projectWs("foo"),
        projectWs("bar", "ws-bar-main"),
        projectWs("bar", "ws-bar-wt"),
      ];
      seedInbox({ settled: [{ id: "ws-bar-wt", at: 1_000 }] });
      await openPicker();
      await waitFor(() => expect(screen.getByText("bar")).toBeInTheDocument());
      expect(screen.queryByText(/still open/)).toBeNull();
    });

    it("parks a project whose every workspace is snoozed — snooze folds into the partition", async () => {
      currentWorkspaces = [projectWs("foo"), projectWs("napping")];
      seedInbox({
        snoozed: [{ id: "ws-napping", at: 1_000, until: 999_999_999 }],
      });
      await openPicker();
      // The fully-snoozed project must not read as Active: it lands in the
      // parked section alongside settled projects.
      await waitFor(() => expect(screen.getByText("Active")).toBeInTheDocument());
      expect(screen.getByText(/still open/)).toBeInTheDocument();
      expect(screen.getByText("napping")).toBeInTheDocument();
      expect(listedProjects()).toEqual([
        "/projects/foo",
        "/projects/napping",
      ]);
    });

    it("orders Active projects most-recently-active first", async () => {
      currentWorkspaces = [
        projectWs("stale"),
        projectWs("foo"),
        projectWs("fresh"),
      ];
      seedInbox({
        activity: { "ws-stale": 1_000, "ws-foo": 5_000, "ws-fresh": 9_000 },
      });
      await openPicker();
      await waitFor(() =>
        expect(listedProjects()).toEqual([
          "/projects/fresh",
          "/projects/foo",
          "/projects/stale",
        ]),
      );
    });

    it("collapses a long settled tail behind 'Show N more'", async () => {
      const settledCount = SETTLED_COLLAPSED_COUNT + 3;
      currentWorkspaces = [
        projectWs("foo"),
        ...Array.from({ length: settledCount }, (_, i) => projectWs(`old${i}`)),
      ];
      seedInbox({
        settled: Array.from({ length: settledCount }, (_, i) => ({
          id: `ws-old${i}`,
          at: 1_000 + i,
        })),
      });
      const user = await openPicker();
      const showMore = await screen.findByText(
        `Show ${settledCount - SETTLED_COLLAPSED_COUNT} more`,
      );
      // foo (active) + the collapsed settled head.
      expect(listedProjects()).toHaveLength(1 + SETTLED_COLLAPSED_COUNT);
      await user.click(showMore);
      await waitFor(() =>
        expect(listedProjects()).toHaveLength(1 + settledCount),
      );
    });

    it("never hides the targeted project in the collapsed settled tail", async () => {
      const settledCount = SETTLED_COLLAPSED_COUNT + 3;
      // Newest-settled first, so "foo" (settled longest ago) lands last.
      currentWorkspaces = [
        projectWs("foo"),
        ...Array.from({ length: settledCount }, (_, i) => projectWs(`old${i}`)),
      ];
      seedInbox({
        settled: [
          ...Array.from({ length: settledCount }, (_, i) => ({
            id: `ws-old${i}`,
            at: 5_000 + i,
          })),
          { id: "ws-foo", at: 1_000 },
        ],
      });
      await openPicker();
      await waitFor(() => expect(listedProjects()).toContain("/projects/foo"));
      expect(listedProjects()).toHaveLength(SETTLED_COLLAPSED_COUNT + 1);
    });

    it("search reaches a project buried in the collapsed settled tail", async () => {
      const settledCount = SETTLED_COLLAPSED_COUNT + 4;
      currentWorkspaces = [
        projectWs("foo"),
        ...Array.from({ length: settledCount }, (_, i) => projectWs(`old${i}`)),
      ];
      seedInbox({
        settled: Array.from({ length: settledCount }, (_, i) => ({
          id: `ws-old${i}`,
          at: 1_000 + i,
        })),
      });
      const user = await openPicker();
      // Settled sorts newest-settled first, so the EARLIEST-settled project
      // (old0) is the one that falls into the hidden tail.
      const buried = "old0";
      await waitFor(() =>
        expect(listedProjects()).not.toContain(`/projects/${buried}`),
      );
      await user.type(screen.getByPlaceholderText("Search projects…"), buried);
      await waitFor(() =>
        expect(listedProjects()).toEqual([`/projects/${buried}`]),
      );
      // The "Show N more" affordance must not leak into search results —
      // a non-empty query reveals the whole section instead.
      expect(screen.queryByText(/Show \d+ more/)).toBeNull();
    });

    it("selecting a settled project does not un-settle it (first send resurfaces it)", async () => {
      currentWorkspaces = [projectWs("foo"), projectWs("bar")];
      seedInbox({ settled: [{ id: "ws-bar", at: 1_000 }] });
      const user = await openPicker();
      await waitFor(() => expect(screen.getByText("bar")).toBeInTheDocument());
      await user.click(screen.getByText("bar"));
      expect(vi.mocked(dbSetUiState)).not.toHaveBeenCalled();
    });

    it("only loads avatars for rows on screen, not every known project", async () => {
      const settledCount = SETTLED_COLLAPSED_COUNT + 5;
      currentWorkspaces = [
        projectWs("foo"),
        ...Array.from({ length: settledCount }, (_, i) => projectWs(`old${i}`)),
      ];
      seedInbox({
        settled: Array.from({ length: settledCount }, (_, i) => ({
          id: `ws-old${i}`,
          at: 1_000 + i,
        })),
      });
      const user = await openPicker();
      await waitFor(() =>
        expect(listedProjects()).toHaveLength(1 + SETTLED_COLLAPSED_COUNT),
      );

      const avatarKeys = () =>
        new Set(
          vi
            .mocked(dbGetUiState)
            .mock.calls.map(([key]) => key)
            .filter((key) => key.startsWith("project.color:")),
        );
      // Only the active row + the collapsed settled head — the buried tail
      // costs nothing until it is revealed.
      expect(avatarKeys().size).toBe(1 + SETTLED_COLLAPSED_COUNT);
      expect(avatarKeys()).not.toContain(
        `project.color:/projects/old${settledCount - 1 - SETTLED_COLLAPSED_COUNT}`,
      );

      await user.click(
        screen.getByText(`Show ${settledCount - SETTLED_COLLAPSED_COUNT} more`),
      );
      await waitFor(() => expect(avatarKeys().size).toBe(1 + settledCount));
    });

    it("keeps 'Open another project…' reachable outside the scrolling list", async () => {
      currentWorkspaces = Array.from({ length: 20 }, (_, i) => projectWs(`p${i}`))
        .concat(projectWs("foo"));
      await openPicker();
      const action = screen.getByText("Open another project…");
      expect(action).toBeInTheDocument();
      expect(action.closest("[data-slot='command-list']")).toBeNull();
    });
  });

  describe("checkout control", () => {
    it("selecting 'New worktree' calls onChangeCheckoutMode('worktree')", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode } = renderRow();
      await user.click(screen.getByText("Current checkout"));
      await screen.findByText("Where should the agent work?");
      await user.click(screen.getByText("New worktree"));
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
    });

    it("shows the name input + hint only when checkoutMode is 'worktree', and typing calls onChangeWorktreeName", async () => {
      const user = userEvent.setup();
      const { onChangeWorktreeName } = renderRow({ checkoutMode: "worktree" });
      await user.click(screen.getByText("New worktree"));
      const input = await screen.findByPlaceholderText(
        "name — leave empty to auto-name",
      );
      await user.type(input, "x");
      expect(onChangeWorktreeName).toHaveBeenCalledWith("x");
      expect(
        screen.getByText(/CodeMux names it from your first message/i),
      ).toBeInTheDocument();
    });
  });

  describe("branch control", () => {
    it("picking a DIFFERENT branch while on 'current' checkout flips to 'worktree' with that branch as base", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "current",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const developRow = await screen.findByText("develop");
      await user.click(developRow);
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
      expect(onChangeBaseBranch).toHaveBeenCalledWith("develop");
    });

    it("picking the SAME branch while on 'current' checkout does not flip checkoutMode", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "current",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const rows = await screen.findAllByText("main");
      // Click the row inside the popover list (not the trigger).
      await user.click(rows[rows.length - 1]);
      expect(onChangeCheckoutMode).not.toHaveBeenCalled();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("main");
    });

    it("picking a branch while already on 'worktree' checkout just updates the base branch", async () => {
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderRow({
        checkoutMode: "worktree",
        baseBranch: "main",
      });
      await user.click(screen.getByText("main"));
      const developRow = await screen.findByText("develop");
      await user.click(developRow);
      expect(onChangeCheckoutMode).not.toHaveBeenCalled();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("develop");
    });

    it("seeds the pill from the checked-out branch, not main", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("main"),
        branch("feature-x", { is_head: true }),
        branch("develop"),
      ]);
      const { onChangeBaseBranch } = renderControlled();
      expect(await screen.findByText("feature-x")).toBeInTheDocument();
      expect(onChangeBaseBranch).toHaveBeenCalledWith("feature-x");
      expect(onChangeBaseBranch).not.toHaveBeenCalledWith("main");
    });

    it("falls back to main when no branch is flagged as checked out", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("feature-x"),
        branch("main"),
        branch("develop"),
      ]);
      const { onChangeBaseBranch } = renderControlled();
      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenCalledWith("main");
      });
      expect(screen.getByText("main")).toBeInTheDocument();
    });

    it("picking main while the checkout is on feature-x flips to a worktree based on main", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("feature-x", { is_head: true }),
        branch("main"),
      ]);
      const user = userEvent.setup();
      const { onChangeCheckoutMode, onChangeBaseBranch } = renderControlled();
      await user.click(await screen.findByText("feature-x"));
      await user.click(await screen.findByText("main"));
      expect(onChangeCheckoutMode).toHaveBeenCalledWith("worktree");
      expect(onChangeBaseBranch).toHaveBeenLastCalledWith("main");
      expect(await screen.findByText("New worktree")).toBeInTheDocument();
    });

    it("switching back to the current checkout snaps the pill back to the real HEAD", async () => {
      vi.mocked(listBranchesDetailed).mockResolvedValue([
        branch("feature-x", { is_head: true }),
        branch("main"),
      ]);
      const user = userEvent.setup();
      const { onChangeBaseBranch } = renderControlled();
      await user.click(await screen.findByText("feature-x"));
      await user.click(await screen.findByText("main"));
      await screen.findByText("New worktree");

      await user.click(screen.getByText("New worktree"));
      await screen.findByText("Where should the agent work?");
      await user.click(screen.getByText("Current checkout"));

      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenLastCalledWith("feature-x");
      });
      expect(screen.getByText("feature-x")).toBeInTheDocument();
    });

    it("refetches on a project switch, so the pill can't keep the old project's HEAD", async () => {
      vi.mocked(listBranchesDetailed).mockImplementation(async (path: string) =>
        path === "/projects/bar"
          ? [branch("bar-head", { is_head: true })]
          : [branch("foo-head", { is_head: true })],
      );
      const onChangeBaseBranch = vi.fn();
      const shared = {
        onChangeTarget: vi.fn(),
        checkoutMode: "current",
        worktreeName: "",
        baseBranch: "",
        onChangeCheckoutMode: vi.fn(),
        onChangeWorktreeName: vi.fn(),
        onChangeBaseBranch,
      } satisfies Partial<ThreadScopeRowProps>;

      const { rerender } = render(
        <ThreadScopeRow
          {...shared}
          target={{ kind: "project", projectPath: "/projects/foo" }}
          projectPath="/projects/foo"
        />,
      );
      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenLastCalledWith("foo-head");
      });

      rerender(
        <ThreadScopeRow
          {...shared}
          target={{ kind: "project", projectPath: "/projects/bar" }}
          projectPath="/projects/bar"
        />,
      );
      await waitFor(() => {
        expect(onChangeBaseBranch).toHaveBeenLastCalledWith("bar-head");
      });
    });

    it("shows a WORKTREE badge on branches that have a worktree on this device", async () => {
      currentWorkspaces = [
        makeWs({
          workspace_id: "ws-foo-main",
          cwd: "/projects/foo",
          project_root: "/projects/foo",
          git_branch: "main",
        }),
      ];
      const user = userEvent.setup();
      renderRow();
      await user.click(screen.getByText("main"));
      await waitFor(() => {
        expect(screen.getByText("WORKTREE")).toBeInTheDocument();
      });
    });
  });
});
