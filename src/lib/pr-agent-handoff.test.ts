import { describe, it, expect, vi, beforeEach } from "vitest";
import type { WorkspaceSnapshot } from "@/tauri/types";

// ── Command layer ──

const mockApplyPreset = vi.fn().mockResolvedValue(undefined);
const mockCreateTab = vi.fn().mockResolvedValue("tab-1");
const mockCreateWorktree = vi
  .fn()
  .mockResolvedValue({ workspaceId: "ws-new", adopted: false, initialTabId: "tab-default" });
const mockGetCheckLogExcerpt = vi.fn().mockResolvedValue("");
const mockGetPresets = vi.fn().mockResolvedValue({
  presets: [
    {
      id: "builtin-claude",
      name: "Claude Code",
      description: null,
      commands: ["claude"],
      working_directory: null,
      launch_mode: "new_tab",
      icon: null,
      pinned: true,
      is_builtin: true,
      auto_run_on_workspace: false,
      auto_run_on_new_tab: false,
      kind: "cli",
      launch_config: null,
    },
  ],
  bar_visible: true,
  default_preset_id: null,
});

vi.mock("@/tauri/commands", () => ({
  applyPreset: (...a: unknown[]) => mockApplyPreset(...a),
  createTab: (...a: unknown[]) => mockCreateTab(...a),
  createWorktreeWorkspaceResult: (...a: unknown[]) => mockCreateWorktree(...a),
  getCheckLogExcerpt: (...a: unknown[]) => mockGetCheckLogExcerpt(...a),
  getPresets: (...a: unknown[]) => mockGetPresets(...a),
}));

const mockActivate = vi.fn().mockResolvedValue(undefined);
vi.mock("@/lib/perf/instrumented-activate", () => ({
  activateWorkspaceInteraction: (...a: unknown[]) => mockActivate(...a),
}));

let workspaces: WorkspaceSnapshot[] = [];
vi.mock("@/stores/app-store", () => ({
  useAppStore: { getState: () => ({ appState: { workspaces } }) },
}));

let lastSelectedAgentId: string | null = null;
vi.mock("@/stores/ui-store", () => ({
  useUIStore: { getState: () => ({ lastSelectedAgentId }) },
}));

const mockToastSuccess = vi.fn();
vi.mock("@/lib/toast", () => ({
  toast: {
    success: (...a: unknown[]) => mockToastSuccess(...a),
    error: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
  },
}));

import {
  buildHandoffPrompt,
  handOffToAgent,
  pickAgentPreset,
  type HandoffRequest,
  type HandoffTask,
} from "./pr-agent-handoff";
import {
  selectRunsForPr,
  usePrAgentRunsStore,
} from "@/stores/pr-agent-runs-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";

const PR = {
  number: 285,
  title: "fix: keep the shutdown handler bounded",
  url: "https://github.com/acme/app/pull/285",
  head_branch: "fix-windows-shutdown",
  base_branch: "main",
};

function req(task: HandoffTask, over: Partial<HandoffRequest> = {}): HandoffRequest {
  return {
    pr: PR,
    task,
    prRef: "acme/app#285",
    projectRoot: "/repo",
    cwd: "/repo",
    cli: "gh",
    providerKind: "github",
    ...over,
  };
}

function ws(over: Partial<WorkspaceSnapshot>): WorkspaceSnapshot {
  return {
    workspace_id: "ws-x",
    title: "some workspace",
    workspace_type: "standard",
    cwd: "/repo",
    git_branch: null,
    project_root: "/repo",
    ...over,
  } as WorkspaceSnapshot;
}

beforeEach(() => {
  vi.clearAllMocks();
  mockCreateWorktree.mockResolvedValue({ workspaceId: "ws-new", adopted: false, initialTabId: "tab-default" });
  mockGetCheckLogExcerpt.mockResolvedValue("");
  workspaces = [];
  lastSelectedAgentId = null;
  useChatDraftStore.setState({ draftsById: {}, activeDraftId: null });
  usePrAgentRunsStore.getState().clear();
});

// ── Prompt composition ────────────────────────────────────────────────

describe("buildHandoffPrompt — failing check", () => {
  const task: HandoffTask = {
    kind: "failing-check",
    checkName: "web-checks (windows-latest)",
    logExcerpt: "AssertionError: expected 2 calls, received 1",
    detailUrl: "https://github.com/acme/app/actions/runs/9",
  };

  it("carries the PR ref, title, branches, check name and log", () => {
    const prompt = buildHandoffPrompt(req(task));
    expect(prompt).toContain("acme/app#285");
    expect(prompt).toContain("fix: keep the shutdown handler bounded");
    expect(prompt).toContain("https://github.com/acme/app/pull/285");
    expect(prompt).toContain("fix-windows-shutdown");
    expect(prompt).toContain("main");
    expect(prompt).toContain("web-checks (windows-latest)");
    expect(prompt).toContain("AssertionError: expected 2 calls, received 1");
    expect(prompt).toContain("https://github.com/acme/app/actions/runs/9");
  });

  it("asks for the smallest relevant verification, not the whole suite", () => {
    const prompt = buildHandoffPrompt(req(task));
    expect(prompt).toContain("smallest relevant check");
    expect(prompt).toContain("AGENTS.md");
    expect(prompt).toMatch(/push to `fix-windows-shutdown`/);
  });

  it("prefers a freshly fetched excerpt over the one on the task", () => {
    const prompt = buildHandoffPrompt(
      req({ ...task, logExcerpt: null }),
      "TypeError: cannot read properties of undefined",
    );
    expect(prompt).toContain("TypeError: cannot read properties of undefined");
  });

  it("survives an excerpt that contains its own code fence", () => {
    const prompt = buildHandoffPrompt(
      req({ ...task, logExcerpt: "```\nnested\n```" }),
    );
    expect(prompt).toContain("````");
    expect(prompt).toContain("nested");
  });
});

describe("buildHandoffPrompt — review thread", () => {
  const task: HandoffTask = {
    kind: "review-thread",
    reviewer: "juliusm",
    body: "The discoverability leg is a separate follow-up — worth a line here saying so.",
    path: "AGENTS.md",
    line: 12,
    verdict: "CHANGES_REQUESTED",
  };

  it("carries reviewer, verdict, file:line and the quoted comment", () => {
    const prompt = buildHandoffPrompt(req(task));
    expect(prompt).toContain("juliusm");
    expect(prompt).toContain("requested changes");
    expect(prompt).toContain("AGENTS.md:12");
    expect(prompt).toContain(
      "> The discoverability leg is a separate follow-up — worth a line here saying so.",
    );
    expect(prompt).toContain("acme/app#285");
  });

  it("includes the parent comment when the note is a reply", () => {
    const prompt = buildHandoffPrompt(
      req({ ...task, parent: { author: "zeus", body: "Why not both?" } }),
    );
    expect(prompt).toContain("Earlier in the thread, zeus wrote:");
    expect(prompt).toContain("> Why not both?");
    expect(prompt).toContain("juliusm replied:");
  });

  it("offers the host comment command only when a CLI is real", () => {
    expect(buildHandoffPrompt(req(task))).toContain("gh pr comment 285");
    // No resolve-thread command exists at any layer, so the prompt must
    // not promise one.
    expect(buildHandoffPrompt(req(task))).toContain("leave that to the reviewer");

    const noCli = buildHandoffPrompt(req(task, { cli: null, providerKind: "unknown" }));
    expect(noCli).not.toContain("gh pr comment");
  });
});

describe("buildHandoffPrompt — conflicts", () => {
  it("names the base branch and lists known files", () => {
    const prompt = buildHandoffPrompt(
      req({ kind: "conflicts", files: ["src/a.ts", "src/b.ts"] }),
    );
    expect(prompt).toContain("conflicts with `main`");
    expect(prompt).toContain("- src/a.ts");
    expect(prompt).toContain("- src/b.ts");
    expect(prompt).toContain("--force-with-lease");
  });

  it("works with no file list at all", () => {
    const prompt = buildHandoffPrompt(req({ kind: "conflicts" }));
    expect(prompt).toContain("conflicts with `main`");
    expect(prompt).toContain("git fetch origin");
  });
});

// ── Preset resolution ─────────────────────────────────────────────────

describe("pickAgentPreset", () => {
  const p = (over: Record<string, unknown>) =>
    ({ id: "x", name: "x", pinned: true, kind: "cli", commands: ["agent"], ...over }) as never;

  it("prefers the agent the user last launched", () => {
    const presets = [p({ id: "builtin-claude" }), p({ id: "builtin-codex" })];
    expect(pickAgentPreset(presets, "builtin-codex")?.id).toBe("builtin-codex");
  });

  it("falls back to the built-in, then to the first pinned CLI preset", () => {
    const presets = [p({ id: "builtin-codex" }), p({ id: "builtin-claude" })];
    expect(pickAgentPreset(presets, null)?.id).toBe("builtin-claude");
    expect(pickAgentPreset([p({ id: "builtin-codex" })], null)?.id).toBe("builtin-codex");
  });

  it("never picks a chat-agent or unpinned preset", () => {
    // `apply_preset` refuses chat_agent presets outright, and an
    // unpinned one is one the user took off their own launcher.
    expect(pickAgentPreset([p({ id: "a", kind: "chat_agent" })], "a")).toBeNull();
    expect(pickAgentPreset([p({ id: "a", pinned: false })], "a")).toBeNull();
    expect(pickAgentPreset([p({ id: "builtin-shell", commands: [] })], "builtin-shell")).toBeNull();
  });
});

// ── Route selection ───────────────────────────────────────────────────

const CHECK_TASK: HandoffTask = {
  kind: "failing-check",
  checkName: "rust (ubuntu-latest)",
  logExcerpt: "error[E0308]: mismatched types",
};

describe("handOffToAgent — routes", () => {
  it("(a) standing in the branch: opens a thread in this workspace", async () => {
    const out = await handOffToAgent(req(CHECK_TASK, { currentWorkspaceId: "ws-here" }));

    expect(out.route).toBe("current");
    expect(mockCreateTab).toHaveBeenCalledWith("ws-here", "terminal");
    expect(mockApplyPreset).toHaveBeenCalledTimes(1);
    const [wsId, presetId, mode, prompt, , freshTerminal, targetTabId] = mockApplyPreset.mock.calls[0];
    expect(wsId).toBe("ws-here");
    expect(presetId).toBe("builtin-claude");
    expect(mode).toBe("current_terminal");
    expect(prompt).toBeNull();
    expect(freshTerminal).toBe(true);
    expect(targetTabId).toBe("tab-1");
    expect(mockCreateWorktree).not.toHaveBeenCalled();
    expect(mockToastSuccess).toHaveBeenCalledWith(
      expect.stringContaining("without the PR task"),
      expect.objectContaining({ action: expect.objectContaining({ label: "Copy prompt" }) }),
    );
    const saved = Object.values(useChatDraftStore.getState().draftsById)[0];
    expect(saved.target).toEqual({ kind: "existing_workspace", workspaceId: "ws-here" });
    expect(saved.inputDraft).toBe(out.prompt);
    expect(saved.manualPromptRequired).toBe(true);
  });

  it("(b) another workspace has the branch: activates it, then opens a thread", async () => {
    workspaces = [
      ws({ workspace_id: "ws-other", git_branch: "fix-windows-shutdown", title: "shutdown" }),
    ];

    const out = await handOffToAgent(req(CHECK_TASK));

    expect(out.route).toBe("existing");
    expect(mockActivate).toHaveBeenCalledWith("ws-other");
    expect(mockCreateTab).toHaveBeenCalledWith("ws-other", "terminal");
    expect(mockApplyPreset.mock.calls[0][0]).toBe("ws-other");
    expect(mockCreateWorktree).not.toHaveBeenCalled();
  });

  it("preserves a review comment verbatim on existing and new-worktree routes", async () => {
    const task: HandoffTask = {
      kind: "review-thread",
      reviewer: "mario",
      body: "Please keep `$(date)` literal in the prompt.",
      path: "src/app.ts",
      line: 12,
    };
    workspaces = [ws({ workspace_id: "ws-other", git_branch: PR.head_branch })];
    const existing = await handOffToAgent(req(task));
    const first = Object.values(useChatDraftStore.getState().draftsById)[0];
    expect(first.target).toEqual({ kind: "existing_workspace", workspaceId: "ws-other" });
    expect(first.inputDraft).toBe(existing.prompt);
    expect(first.inputDraft).toContain(task.body);

    workspaces = [];
    const worktree = await handOffToAgent(req(task));
    const second = Object.values(useChatDraftStore.getState().draftsById).find(
      (draft) => draft.target.kind === "existing_workspace" && draft.target.workspaceId === "ws-new",
    );
    expect(second?.inputDraft).toBe(worktree.prompt);
    expect(second?.inputDraft).toContain(task.body);
    expect(mockCreateWorktree.mock.calls[mockCreateWorktree.mock.calls.length - 1]?.[5]).toBeNull();
  });

  it("ignores a same-branch workspace belonging to another project", async () => {
    workspaces = [
      ws({
        workspace_id: "ws-elsewhere",
        git_branch: "fix-windows-shutdown",
        project_root: "/other-repo",
      }),
    ];

    const out = await handOffToAgent(req(CHECK_TASK));

    expect(out.route).toBe("worktree");
    expect(mockActivate).not.toHaveBeenCalled();
  });

  it("(c) nowhere to land: cuts a worktree and retains the prompt", async () => {
    const out = await handOffToAgent(req(CHECK_TASK));

    expect(out.route).toBe("worktree");
    expect(out.workspaceId).toBe("ws-new");
    const args = mockCreateWorktree.mock.calls[0];
    expect(args[0]).toBe("/repo");
    expect(args[1]).toBe("fix-windows-shutdown");
    expect(args[2]).toBe(false);
    expect(args[3]).toBe("single");
    expect(args[5]).toBeNull(); // no prompt in the terminal launch
    expect(args[6]).toBeNull(); // the command is sent only after workspace creation
    expect(args[7]).toBe(285); // prNumber
    expect(Object.values(useChatDraftStore.getState().draftsById)[0].inputDraft).toBe(out.prompt);
    expect(mockApplyPreset).toHaveBeenCalledWith(
      "ws-new", "builtin-claude", "current_terminal", null, null, true, "tab-default",
    );
    expect(mockCreateTab).not.toHaveBeenCalled();
  });

  it("uses the tab returned at worktree creation even if tab order later changes", async () => {
    mockCreateWorktree.mockResolvedValueOnce({
      workspaceId: "ws-new",
      adopted: false,
      initialTabId: "tab-original",
    });

    await handOffToAgent(req(CHECK_TASK));

    expect(mockApplyPreset.mock.calls[0][6]).toBe("tab-original");
  });

  it("does not substitute another terminal if the original tab ID is missing", async () => {
    mockCreateWorktree.mockResolvedValueOnce({
      workspaceId: "ws-new",
      adopted: false,
      initialTabId: null,
    });

    await expect(handOffToAgent(req(CHECK_TASK))).rejects.toThrow(/no initial terminal tab/);
    expect(mockApplyPreset).not.toHaveBeenCalled();
    expect(mockToastSuccess).not.toHaveBeenCalled();
    expect(Object.values(useChatDraftStore.getState().draftsById)[0].manualPromptRequired).toBe(false);
  });

  it("keeps the PR task without claiming a new-worktree agent opened when its write fails", async () => {
    mockApplyPreset.mockRejectedValueOnce(new Error("Terminal shell is not writable"));

    await expect(handOffToAgent(req(CHECK_TASK))).rejects.toThrow(/saved in Drafts/);

    const saved = Object.values(useChatDraftStore.getState().draftsById)[0];
    expect(saved.target).toEqual({ kind: "existing_workspace", workspaceId: "ws-new" });
    expect(saved.inputDraft).toContain("rust (ubuntu-latest)");
    expect(saved.manualPromptRequired).toBe(false);
    expect(mockToastSuccess).not.toHaveBeenCalled();
    expect(usePrAgentRunsStore.getState().runs).toEqual([]);
  });

  it("(c') an adopted workspace opens the preset and saves the prompt", async () => {
    mockCreateWorktree.mockResolvedValue({ workspaceId: "ws-adopted", adopted: true });

    const out = await handOffToAgent(req(CHECK_TASK));

    // Adoption drops the prompt and preset on the backend side; the
    // handoff must not let the instruction evaporate.
    expect(out.route).toBe("adopted");
    expect(mockApplyPreset).toHaveBeenCalledTimes(1);
    const [wsId, , mode, prompt] = mockApplyPreset.mock.calls[0];
    expect(wsId).toBe("ws-adopted");
    expect(mode).toBe("current_terminal");
    expect(prompt).toBeNull();
    expect(mockApplyPreset.mock.calls[0][5]).toBe(true);
    expect(mockApplyPreset.mock.calls[0][6]).toBe("tab-1");
    expect(Object.values(useChatDraftStore.getState().draftsById)[0].inputDraft).toBe(out.prompt);
  });

  it("fetches a missing log excerpt rather than sending a thin prompt", async () => {
    mockGetCheckLogExcerpt.mockResolvedValue("panicked at 'index out of bounds'");

    const out = await handOffToAgent(
      req(
        { kind: "failing-check", checkName: "rust (ubuntu-latest)" },
        { currentWorkspaceId: "ws-here" },
      ),
    );

    expect(mockGetCheckLogExcerpt).toHaveBeenCalledWith("/repo", 285, "rust (ubuntu-latest)");
    expect(out.prompt).toContain("panicked at 'index out of bounds'");
  });

  it("does not claim an agent opened when the fresh PTY write fails", async () => {
    mockApplyPreset.mockRejectedValueOnce(new Error("Terminal shell is not writable"));

    await expect(handOffToAgent(req(CHECK_TASK, { currentWorkspaceId: "ws-here" })))
      .rejects.toThrow("Terminal shell is not writable");

    expect(mockToastSuccess).not.toHaveBeenCalled();
    expect(usePrAgentRunsStore.getState().runs).toEqual([]);
    expect(Object.values(useChatDraftStore.getState().draftsById)).toEqual([]);
  });

  it("uses the remembered agent preset when there is one", async () => {
    lastSelectedAgentId = "builtin-claude";
    await handOffToAgent(req(CHECK_TASK, { currentWorkspaceId: "ws-here" }));
    expect(mockApplyPreset.mock.calls[0][1]).toBe("builtin-claude");
  });

  it("refuses to route a PR with no head branch", async () => {
    await expect(
      handOffToAgent(req(CHECK_TASK, { pr: { ...PR, head_branch: null } })),
    ).rejects.toThrow(/head branch/);
    expect(mockCreateWorktree).not.toHaveBeenCalled();
  });
});

// ── The local record the Timeline merges ──

describe("handOffToAgent — the run it records", () => {
  beforeEach(() => usePrAgentRunsStore.getState().clear());

  it("records the run against the PR, with the thread it opened", async () => {
    await handOffToAgent(req(CHECK_TASK, { currentWorkspaceId: "ws-here" }));

    const runs = selectRunsForPr(usePrAgentRunsStore.getState().runs, {
      prRef: "acme/app#285",
      projectRoot: "/repo",
      prNumber: 285,
    });
    expect(runs).toHaveLength(1);
    expect(runs[0]).toMatchObject({
      kind: "failing-check",
      // The check name is what the card shows.
      summary: "rust (ubuntu-latest)",
      promptNotSent: true,
      workspaceId: "ws-here",
      threadTabId: "tab-1",
      prNumber: 285,
    });
    // Never invented — the agent has written nothing yet.
    expect(runs[0].files).toBeUndefined();
  });

  it("summarizes a review thread by its file:line anchor", async () => {
    await handOffToAgent(
      req(
        {
          kind: "review-thread",
          reviewer: "juliusm",
          body: "Worth a line here.",
          path: "AGENTS.md",
          line: 12,
        },
        { currentWorkspaceId: "ws-here" },
      ),
    );
    expect(usePrAgentRunsStore.getState().runs[0].summary).toBe("AGENTS.md:12");
  });

  it("records nothing when the handoff never started a thread", async () => {
    await expect(
      handOffToAgent(req(CHECK_TASK, { pr: { ...PR, head_branch: null } })),
    ).rejects.toThrow();
    // The timeline is a record of what happened, not of what was tried.
    expect(usePrAgentRunsStore.getState().runs).toEqual([]);
  });
});
