/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TooltipProvider } from "@/components/ui/tooltip";

import type { WorkspaceSnapshot } from "@/tauri/types";

// ── App-store mock ────────────────────────────────────────────────────
//
// `WorktreePicker` reads the workspace list and homeDir via app-store
// hooks, then groups them with `useProjectGroupedWorkspaces`. We mock
// the hooks to return whatever the test seeded under `currentState`.

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
  const actual = await vi.importActual<
    typeof import("@/stores/app-store")
  >("@/stores/app-store");
  return {
    ...actual,
    useAppStore: vi.fn((selector: (s: unknown) => unknown) =>
      selector({ appState: { workspaces: currentWorkspaces } }),
    ),
    useHomeDir: () => "/home/user",
    useProjectGroupedWorkspaces: actual.useProjectGroupedWorkspaces,
  };
});

// Tauri commands — `createWorktreeWorkspace` is the heart of the
// inline-input flow; `generateRandomBranchName` feeds the "empty name"
// path.
vi.mock("@/tauri/commands", () => ({
  createWorktreeWorkspace: vi.fn(),
  generateRandomBranchName: vi.fn(),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
    warning: vi.fn(),
  },
}));

import { WorktreePicker } from "./WorktreePicker";
import type { ChatDraft } from "@/stores/chat-draft-store";
import {
  createWorktreeWorkspace,
  generateRandomBranchName,
} from "@/tauri/commands";

afterEach(() => cleanup());

const FOO_WORKTREES: WorkspaceSnapshot[] = [
  makeWs({
    workspace_id: "ws-foo-main",
    cwd: "/projects/foo",
    project_root: "/projects/foo",
    git_branch: "main",
  }),
  makeWs({
    workspace_id: "ws-foo-feat",
    cwd: "/projects/foo-feat-x",
    project_root: "/projects/foo",
    git_branch: "feat/x",
  }),
  makeWs({
    workspace_id: "ws-foo-bug",
    cwd: "/projects/foo-bug-fix",
    project_root: "/projects/foo",
    git_branch: "bugfix/y",
  }),
];

function renderActive(
  overrides: Partial<Parameters<typeof WorktreePicker>[0]> = {},
) {
  const onSwitchWorkspace = vi.fn();
  const onWorktreeCreated = vi.fn();
  const utils = render(
    <TooltipProvider>
      <WorktreePicker
        mode="active"
        projectPath="/projects/foo"
        currentWorkspaceId="ws-foo-main"
        derivativeBranch="main"
        onSwitchWorkspace={onSwitchWorkspace}
        onWorktreeCreated={onWorktreeCreated}
        {...overrides}
      />
    </TooltipProvider>,
  );
  const trigger = utils.container.querySelector(
    "button",
  ) as HTMLButtonElement | null;
  return { ...utils, trigger, onSwitchWorkspace, onWorktreeCreated };
}

function renderDraft(
  overrides: Partial<Parameters<typeof WorktreePicker>[0]> = {},
) {
  const onChangeDraftTarget = vi.fn();
  const onWorktreeCreated = vi.fn();
  const draftTarget: ChatDraft["target"] = {
    kind: "project",
    projectPath: "/projects/foo",
  };
  const utils = render(
    <TooltipProvider>
      <WorktreePicker
        mode="draft"
        projectPath="/projects/foo"
        draftTarget={draftTarget}
        derivativeBranch="main"
        onChangeDraftTarget={onChangeDraftTarget}
        onWorktreeCreated={onWorktreeCreated}
        {...overrides}
      />
    </TooltipProvider>,
  );
  const trigger = utils.container.querySelector(
    "button",
  ) as HTMLButtonElement | null;
  return { ...utils, trigger, onChangeDraftTarget, onWorktreeCreated };
}

describe("WorktreePicker — active mode", () => {
  beforeEach(() => {
    currentWorkspaces = FOO_WORKTREES;
    vi.mocked(createWorktreeWorkspace).mockReset();
    vi.mocked(generateRandomBranchName).mockReset();
  });

  it("trigger pill shows the worktree folder basename, not the git branch", () => {
    // The primary pill identifies the worktree (a folder). For the
    // workspace at /projects/foo on branch "main", the pill should
    // read "foo" — the branch string belongs to the adjacent
    // DerivativeBranchPicker ("from main"). Regression guard for the
    // "shows `master` instead of project folder name" bug.
    const { trigger } = renderActive();
    expect(trigger).not.toBeNull();
    expect(trigger!.textContent).toContain("foo");
    expect(trigger!.textContent).not.toContain("main");
  });

  it("trigger uses cwd basename even for non-default worktrees with a populated git_branch", () => {
    // Non-default worktree: cwd differs from project_root and the
    // branch is populated. The pill must still read the folder name.
    const { trigger } = renderActive({
      currentWorkspaceId: "ws-foo-feat",
    });
    expect(trigger!.textContent).toContain("foo-feat-x");
    expect(trigger!.textContent).not.toContain("feat/x");
  });

  it("uses basename of cwd when git_branch is null", () => {
    currentWorkspaces = [
      makeWs({
        workspace_id: "ws-x",
        cwd: "/projects/foo-no-branch",
        project_root: "/projects/foo",
        git_branch: null,
      }),
    ];
    const { trigger } = renderActive({
      currentWorkspaceId: "ws-x",
    });
    expect(trigger!.textContent).toContain("foo-no-branch");
  });

  it("opens the menu and lists all worktrees for the project", async () => {
    const user = userEvent.setup();
    const { trigger } = renderActive();
    await user.click(trigger!);
    const options = await screen.findAllByRole("option");
    // 3 worktrees + "New worktree…" = 4 rows
    expect(options.length).toBe(4);
    const labels = options.map((el) => el.textContent ?? "");
    expect(labels.some((t) => t.includes("main"))).toBe(true);
    expect(labels.some((t) => t.includes("feat/x"))).toBe(true);
    expect(labels.some((t) => t.includes("bugfix/y"))).toBe(true);
    expect(labels.some((t) => t.includes("New worktree"))).toBe(true);
  });

  it("renders the 'active' badge on the current workspace row", async () => {
    const user = userEvent.setup();
    const { trigger } = renderActive({ currentWorkspaceId: "ws-foo-feat" });
    await user.click(trigger!);
    const featRow = (await screen.findAllByRole("option")).find((row) =>
      row.textContent?.includes("feat/x"),
    );
    expect(featRow).toBeDefined();
    expect(featRow!.textContent).toContain("active");
    const mainRow = (await screen.findAllByRole("option")).find((row) =>
      row.textContent?.includes("main"),
    );
    expect(mainRow!.textContent).not.toContain("active");
  });

  it("clicking a worktree row fires onSwitchWorkspace with that id", async () => {
    const user = userEvent.setup();
    const { trigger, onSwitchWorkspace } = renderActive();
    await user.click(trigger!);
    await user.click(await screen.findByText("feat/x"));
    expect(onSwitchWorkspace).toHaveBeenCalledWith("ws-foo-feat");
  });

  it("disabled prop disables the trigger AND prevents the menu from opening", async () => {
    const user = userEvent.setup();
    const { trigger, container } = renderActive({ disabled: true });
    expect(trigger!.disabled).toBe(true);
    // Clicking a disabled <button> in jsdom is a no-op. Verify no
    // popover content appears anywhere in the document.
    await user.click(trigger!);
    // Even if a click did slip through, no role="option" should be
    // findable since the popover never mounted.
    expect(container.querySelector('[role="option"]')).toBeNull();
    expect(document.querySelector('[role="option"]')).toBeNull();
  });

  it("Enter on the first row fires the switch handler with that row's workspace id", async () => {
    const user = userEvent.setup();
    const { trigger, onSwitchWorkspace, onWorktreeCreated } = renderActive();
    await user.click(trigger!);
    // cmdk auto-highlights the first option after open; Enter selects
    // it. Combined with ArrowDown the call still lands on a worktree
    // (the "New worktree…" row sits below the separator and is the
    // last item, never the first).
    await screen.findAllByRole("option");
    await user.keyboard("{Enter}");
    expect(onWorktreeCreated).not.toHaveBeenCalled();
    expect(onSwitchWorkspace).toHaveBeenCalledTimes(1);
    expect(
      ["ws-foo-main", "ws-foo-feat", "ws-foo-bug"].includes(
        onSwitchWorkspace.mock.calls[0][0] as string,
      ),
    ).toBe(true);
  });

  it("does not render the 'active' badge when currentWorkspaceId matches no worktree", async () => {
    const user = userEvent.setup();
    const { trigger } = renderActive({
      currentWorkspaceId: "ws-does-not-exist",
    });
    await user.click(trigger!);
    const options = await screen.findAllByRole("option");
    for (const row of options) {
      expect(row.textContent).not.toContain("active");
    }
  });

  it("filters worktrees by projectPath — siblings from other projects are excluded", async () => {
    currentWorkspaces = [
      ...FOO_WORKTREES,
      makeWs({
        workspace_id: "ws-bar-main",
        cwd: "/projects/bar",
        project_root: "/projects/bar",
        git_branch: "bar-main",
      }),
      makeWs({
        workspace_id: "ws-bar-feat",
        cwd: "/projects/bar-feat",
        project_root: "/projects/bar",
        git_branch: "bar-feat",
      }),
    ];
    const user = userEvent.setup();
    const { trigger } = renderActive({ projectPath: "/projects/foo" });
    await user.click(trigger!);
    const options = await screen.findAllByRole("option");
    const labels = options.map((el) => el.textContent ?? "");
    // 3 foo worktrees + "New worktree…" = 4. Bar worktrees absent.
    expect(options.length).toBe(4);
    expect(labels.some((t) => t.includes("bar-main"))).toBe(false);
    expect(labels.some((t) => t.includes("bar-feat"))).toBe(false);
  });

  it("re-clicking the current workspace row still fires onSwitchWorkspace (caller decides to no-op)", async () => {
    const user = userEvent.setup();
    const { trigger, onSwitchWorkspace } = renderActive({
      currentWorkspaceId: "ws-foo-main",
    });
    await user.click(trigger!);
    const options = await screen.findAllByRole("option");
    const mainRow = options.find((row) => row.textContent?.includes("main"));
    expect(mainRow).toBeDefined();
    await user.click(mainRow!);
    // The picker doesn't suppress same-id selections — the caller
    // (AgentChatPane) is responsible for short-circuiting if it cares.
    expect(onSwitchWorkspace).toHaveBeenCalledWith("ws-foo-main");
  });
});

describe("WorktreePicker — draft mode", () => {
  beforeEach(() => {
    currentWorkspaces = FOO_WORKTREES;
    vi.mocked(createWorktreeWorkspace).mockReset();
    vi.mocked(generateRandomBranchName).mockReset();
  });

  it("trigger label uses project basename for project-target drafts", () => {
    const { trigger } = renderDraft();
    expect(trigger!.textContent).toContain("foo");
  });

  it("trigger label uses the targeted worktree's folder basename for existing_workspace", () => {
    // Regression guard: the trigger shows the worktree FOLDER name,
    // not the git branch. The branch sits in the sibling
    // DerivativeBranchPicker.
    const { trigger } = renderDraft({
      draftTarget: {
        kind: "existing_workspace",
        workspaceId: "ws-foo-feat",
      },
    });
    expect(trigger!.textContent).toContain("foo-feat-x");
    expect(trigger!.textContent).not.toContain("feat/x");
  });

  it("clicking a worktree row fires onChangeDraftTarget with existing_workspace target", async () => {
    const user = userEvent.setup();
    const { trigger, onChangeDraftTarget } = renderDraft();
    await user.click(trigger!);
    await user.click(await screen.findByText("feat/x"));
    expect(onChangeDraftTarget).toHaveBeenCalledWith({
      kind: "existing_workspace",
      workspaceId: "ws-foo-feat",
    });
  });

  it("renders the 'active' badge on the existing_workspace target row", async () => {
    const user = userEvent.setup();
    const { trigger } = renderDraft({
      draftTarget: {
        kind: "existing_workspace",
        workspaceId: "ws-foo-bug",
      },
    });
    await user.click(trigger!);
    const bugRow = (await screen.findAllByRole("option")).find((row) =>
      row.textContent?.includes("bugfix/y"),
    );
    expect(bugRow!.textContent).toContain("active");
  });

  it("falls back to project basename when existing_workspace target's workspace is no longer in app-state", () => {
    const { trigger } = renderDraft({
      draftTarget: {
        kind: "existing_workspace",
        workspaceId: "ws-no-longer-here",
      },
    });
    // Workspace lookup misses → projectPath basename takes over.
    expect(trigger!.textContent).toContain("foo");
  });

  it("clicking a worktree row in draft mode does NOT call onSwitchWorkspace", async () => {
    const user = userEvent.setup();
    const onSwitchWorkspace = vi.fn();
    const { trigger } = renderDraft({ onSwitchWorkspace });
    await user.click(trigger!);
    await user.click(await screen.findByText("feat/x"));
    expect(onSwitchWorkspace).not.toHaveBeenCalled();
  });
});

describe("WorktreePicker — screenshot regression (folder vs branch)", () => {
  // Direct reproduction of the reported bug: project folder
  // "the-machine" sits on branch "master" (the default). The trigger
  // pill must show "the-machine", not "master".
  beforeEach(() => {
    currentWorkspaces = [
      makeWs({
        workspace_id: "ws-the-machine-default",
        cwd: "/home/user/code/the-machine",
        project_root: "/home/user/code/the-machine",
        git_branch: "master",
      }),
    ];
    vi.mocked(createWorktreeWorkspace).mockReset();
    vi.mocked(generateRandomBranchName).mockReset();
  });

  it("active mode: pill reads 'the-machine', not 'master'", () => {
    const { trigger } = renderActive({
      projectPath: "/home/user/code/the-machine",
      currentWorkspaceId: "ws-the-machine-default",
    });
    expect(trigger!.textContent).toContain("the-machine");
    expect(trigger!.textContent).not.toContain("master");
  });

  it("draft existing_workspace: pill reads 'the-machine', not 'master'", () => {
    const { trigger } = renderDraft({
      projectPath: "/home/user/code/the-machine",
      draftTarget: {
        kind: "existing_workspace",
        workspaceId: "ws-the-machine-default",
      },
    });
    expect(trigger!.textContent).toContain("the-machine");
    expect(trigger!.textContent).not.toContain("master");
  });
});

describe("WorktreePicker — empty worktree list", () => {
  beforeEach(() => {
    currentWorkspaces = [];
    vi.mocked(createWorktreeWorkspace).mockReset();
    vi.mocked(generateRandomBranchName).mockReset();
  });

  it("renders only the 'New worktree…' row", async () => {
    const user = userEvent.setup();
    const { trigger } = renderActive({ currentWorkspaceId: undefined });
    await user.click(trigger!);
    const options = await screen.findAllByRole("option");
    expect(options.length).toBe(1);
    expect(options[0].textContent).toContain("New worktree");
  });

  it("trigger falls back to the projectPath basename when no workspaces exist", () => {
    const { trigger } = renderActive({
      projectPath: "/projects/empty-proj",
      currentWorkspaceId: undefined,
    });
    expect(trigger!.textContent).toContain("empty-proj");
  });
});

// ── Stage D follow-up: inline "+ New worktree…" input ─────────────────
//
// The row starts in its resting "+ New worktree…" label state. Clicking
// it transforms in place into a text input (still inside the same row).
// Enter submits (empty → auto-generated name), Escape cancels.
describe("WorktreePicker — inline New Worktree input", () => {
  beforeEach(() => {
    currentWorkspaces = FOO_WORKTREES;
    vi.mocked(createWorktreeWorkspace).mockReset();
    vi.mocked(generateRandomBranchName).mockReset();
  });

  it("clicking the row transforms the label into an editable input", async () => {
    const user = userEvent.setup();
    const { trigger } = renderActive();
    await user.click(trigger!);
    // Resting state: label is visible.
    expect(
      document.querySelector(
        'input[aria-label="New worktree branch name"]',
      ),
    ).toBeNull();
    await user.click(await screen.findByText(/New worktree/));
    const input = document.querySelector<HTMLInputElement>(
      'input[aria-label="New worktree branch name"]',
    );
    expect(input).not.toBeNull();
    expect(input!.placeholder).toContain("leave empty for auto");
    expect(document.activeElement).toBe(input);
  });

  it("Enter with a typed branch name calls createWorktreeWorkspace and onWorktreeCreated", async () => {
    vi.mocked(createWorktreeWorkspace).mockResolvedValueOnce("ws-new");
    const user = userEvent.setup();
    const { trigger, onWorktreeCreated } = renderActive();
    await user.click(trigger!);
    await user.click(await screen.findByText(/New worktree/));
    const input = document.querySelector<HTMLInputElement>(
      'input[aria-label="New worktree branch name"]',
    );
    expect(input).not.toBeNull();
    await user.type(input!, "feature/cool-thing");
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(createWorktreeWorkspace).toHaveBeenCalled();
    });
    const call = vi.mocked(createWorktreeWorkspace).mock.calls[0];
    expect(call[0]).toBe("/projects/foo"); // repoPath
    expect(call[1]).toBe("feature/cool-thing"); // branch
    expect(call[2]).toBe(true); // newBranch
    // "empty" layout → backend creates git worktree + empty
    // workspace (no terminal/PTY); chat pane is attached afterward
    // by handleWorktreeCreated → agentChatCreatePane. Any other
    // layout reintroduces the split-with-leftover-terminal bug.
    expect(call[3]).toBe("empty"); // layout
    expect(call[4]).toBe("main"); // base → derivativeBranch
    // generateRandomBranchName MUST NOT be called — user provided one.
    expect(generateRandomBranchName).not.toHaveBeenCalled();
    await waitFor(() => {
      expect(onWorktreeCreated).toHaveBeenCalledWith("ws-new");
    });
  });

  it("Enter with empty input calls generateRandomBranchName and uses the returned name", async () => {
    vi.mocked(generateRandomBranchName).mockResolvedValueOnce("auto-abc");
    vi.mocked(createWorktreeWorkspace).mockResolvedValueOnce("ws-auto");
    const user = userEvent.setup();
    const { trigger, onWorktreeCreated } = renderActive();
    await user.click(trigger!);
    await user.click(await screen.findByText(/New worktree/));
    const input = document.querySelector<HTMLInputElement>(
      'input[aria-label="New worktree branch name"]',
    );
    // Focus the input explicitly to avoid the cmdk root stealing the
    // Enter keystroke when the input blurs (empty-input case).
    input!.focus();
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(generateRandomBranchName).toHaveBeenCalledWith("/projects/foo");
    });
    await waitFor(() => {
      expect(createWorktreeWorkspace).toHaveBeenCalled();
    });
    const call = vi.mocked(createWorktreeWorkspace).mock.calls[0];
    expect(call[1]).toBe("auto-abc");
    await waitFor(() => {
      expect(onWorktreeCreated).toHaveBeenCalledWith("ws-auto");
    });
  });

  it("Escape cancels edit mode without submitting", async () => {
    const user = userEvent.setup();
    const { trigger, onWorktreeCreated } = renderActive();
    await user.click(trigger!);
    await user.click(await screen.findByText(/New worktree/));
    const input = document.querySelector<HTMLInputElement>(
      'input[aria-label="New worktree branch name"]',
    );
    await user.type(input!, "junk");
    await user.keyboard("{Escape}");
    // The input unmounts (the row either transforms back to its
    // label or the popover closes — either way, edit mode is over).
    await waitFor(() => {
      expect(
        document.querySelector(
          'input[aria-label="New worktree branch name"]',
        ),
      ).toBeNull();
    });
    expect(createWorktreeWorkspace).not.toHaveBeenCalled();
    expect(generateRandomBranchName).not.toHaveBeenCalled();
    expect(onWorktreeCreated).not.toHaveBeenCalled();
    // Re-opening the picker surfaces the resting label again — edit
    // state doesn't leak across opens.
    await user.click(trigger!);
    expect(await screen.findByText(/New worktree/)).toBeInTheDocument();
    expect(
      document.querySelector('input[aria-label="New worktree branch name"]'),
    ).toBeNull();
  });

  it("passes the picker's derivativeBranch as the `base` to createWorktreeWorkspace", async () => {
    vi.mocked(createWorktreeWorkspace).mockResolvedValueOnce("ws-new");
    const user = userEvent.setup();
    const { trigger } = renderActive({ derivativeBranch: "develop" });
    await user.click(trigger!);
    await user.click(await screen.findByText(/New worktree/));
    const input = document.querySelector<HTMLInputElement>(
      'input[aria-label="New worktree branch name"]',
    );
    await user.type(input!, "feature/x");
    await user.keyboard("{Enter}");
    await waitFor(() => {
      expect(createWorktreeWorkspace).toHaveBeenCalled();
    });
    const call = vi.mocked(createWorktreeWorkspace).mock.calls[0];
    expect(call[4]).toBe("develop");
  });
});
