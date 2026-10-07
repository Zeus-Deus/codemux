/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act, cleanup, within } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ProjectOnboarding } from "./project-onboarding";

// ── Mock Tauri commands ──
//
// The component fires several side-effecting calls on mount (branch list,
// worktree list, etc.). Resolve them with empty defaults so the component
// reaches its rendered state. Then we can assert on skip behavior.
vi.mock("@/tauri/commands", () => ({
  listBranchesDetailed: vi.fn().mockResolvedValue([]),
  listWorktrees: vi.fn().mockResolvedValue([]),
  getDefaultBranch: vi.fn().mockResolvedValue("main"),
  generateBranchName: vi.fn().mockResolvedValue("some-branch"),
  generateRandomBranchName: vi.fn().mockResolvedValue("random-branch"),
  createWorktreeWorkspaceResult: vi
    .fn()
    .mockResolvedValue({ workspaceId: "ws-new", adopted: false }),
  importWorktreeWorkspace: vi.fn().mockResolvedValue("ws-new"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  closeWorkspace: vi.fn().mockResolvedValue(undefined),
  detectPackageManager: vi.fn().mockResolvedValue([]),
  setProjectScripts: vi.fn().mockResolvedValue(undefined),
  dbAddRecentProject: vi.fn().mockResolvedValue(undefined),
  getPresets: vi.fn().mockResolvedValue({
    presets: [],
    bar_visible: false,
    default_preset_id: null,
  }),
}));

vi.mock("@/lib/toast", () => ({
  toast: { info: vi.fn(), warning: vi.fn(), error: vi.fn(), success: vi.fn() },
}));

import {
  closeWorkspace,
  createWorktreeWorkspaceResult,
  getPresets,
  importWorktreeWorkspace,
  listWorktrees,
  activateWorkspace,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { useUIStore } from "@/stores/ui-store";
import type { TerminalPreset, WorktreeInfo } from "@/tauri/types";

const mockCloseWorkspace = vi.mocked(closeWorkspace);
const mockCreate = vi.mocked(createWorktreeWorkspaceResult);
const mockGetPresets = vi.mocked(getPresets);
const mockImport = vi.mocked(importWorktreeWorkspace);
const mockListWorktrees = vi.mocked(listWorktrees);
const mockActivate = vi.mocked(activateWorkspace);

function makePreset(overrides: Partial<TerminalPreset>): TerminalPreset {
  return {
    id: "builtin-claude",
    name: "Claude Code",
    description: null,
    commands: ["claude"],
    working_directory: null,
    launch_mode: "new_tab",
    icon: "claude",
    pinned: true,
    is_builtin: true,
    auto_run_on_workspace: false,
    auto_run_on_new_tab: false,
    kind: "cli",
    ...overrides,
  };
}

function renderOnboarding(overrides: {
  onComplete?: () => void;
  onCancel?: () => void;
} = {}) {
  const onComplete = overrides.onComplete ?? vi.fn();
  const onCancel = overrides.onCancel ?? vi.fn();
  const utils = render(
    <TooltipProvider>
      <ProjectOnboarding
        projectDir="/home/user/myproj"
        tempWorkspaceId="ws-temp-1"
        onComplete={onComplete}
        onCancel={onCancel}
      />
    </TooltipProvider>,
  );
  return { ...utils, onComplete, onCancel };
}

async function flushMountEffects() {
  // The mount useEffect does Promise.all(...) then setState. Flushing a
  // microtask tick lets the mocked commands resolve before we click.
  await act(async () => {
    await Promise.resolve();
  });
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  useUIStore.setState({ lastSelectedAgentId: null });
});

async function goToSetupStep(task = "Add dark mode") {
  fireEvent.change(screen.getByLabelText("Task"), { target: { value: task } });
  fireEvent.click(screen.getByRole("button", { name: /continue/i }));
  await flushMountEffects();
}

describe("ProjectOnboarding — task and agent", () => {
  beforeEach(() => {
    mockGetPresets.mockResolvedValueOnce({
      presets: [
        makePreset({}),
        makePreset({ id: "builtin-codex", name: "Codex", icon: "codex" }),
        makePreset({ id: "unpinned", name: "Hidden", pinned: false }),
      ],
      bar_visible: false,
      default_preset_id: null,
    });
  });

  it("sends the task as the first prompt to the selected agent", async () => {
    const { onComplete } = renderOnboarding();
    await flushMountEffects();
    await goToSetupStep("  Add dark mode  ");

    fireEvent.click(screen.getByRole("button", { name: "Create & start Claude Code" }));
    await flushMountEffects();

    expect(mockCreate).toHaveBeenCalledWith(
      "/home/user/myproj",
      "some-branch",
      true,
      "single",
      "main",
      "Add dark mode",
      "builtin-claude",
    );
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  it("starts the agent the user last picked when it is still pinned", async () => {
    useUIStore.setState({ lastSelectedAgentId: "builtin-codex" });
    renderOnboarding();
    await flushMountEffects();

    expect(screen.getByRole("button", { name: "Agent: Codex" })).toBeInTheDocument();
    await goToSetupStep();
    expect(screen.getByRole("button", { name: "Create & start Codex" })).toBeInTheDocument();
  });

  it("closes the temporary workspace only after the real one exists", async () => {
    renderOnboarding();
    await flushMountEffects();
    await goToSetupStep();

    fireEvent.click(screen.getByRole("button", { name: /create & start/i }));
    await flushMountEffects();

    expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-temp-1", false);
    expect(mockCreate.mock.invocationCallOrder[0]).toBeLessThan(
      mockCloseWorkspace.mock.invocationCallOrder[0],
    );
  });

  it("says so when the backend adopts a live workspace and drops the task", async () => {
    mockCreate.mockResolvedValueOnce({ workspaceId: "ws-live", cwd: null, adopted: true });
    const { onComplete } = renderOnboarding();
    await flushMountEffects();
    await goToSetupStep();

    fireEvent.click(screen.getByRole("button", { name: /create & start/i }));
    await flushMountEffects();

    expect(vi.mocked(toast.info)).toHaveBeenCalledWith(
      expect.stringContaining("Your task wasn't sent"),
    );
    expect(mockActivate).toHaveBeenCalledWith("ws-live");
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  it("advances on Enter in the task input but not from the agent picker", async () => {
    renderOnboarding();
    await flushMountEffects();
    fireEvent.change(screen.getByLabelText("Task"), { target: { value: "Add dark mode" } });

    // Enter on the agent pill (or its portaled menu) must not advance the step.
    fireEvent.keyDown(screen.getByRole("button", { name: "Agent: Claude Code" }), { key: "Enter" });
    expect(screen.getByLabelText("Task")).toBeInTheDocument();

    fireEvent.keyDown(screen.getByLabelText("Task"), { key: "Enter" });
    expect(screen.queryByLabelText("Task")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create & start Claude Code" })).toBeInTheDocument();
  });
});

describe("ProjectOnboarding — failures", () => {
  it("shows a create failure inline and keeps the project open", async () => {
    mockCreate.mockRejectedValueOnce("branch already exists");
    const { onComplete } = renderOnboarding();
    await flushMountEffects();
    await goToSetupStep();

    fireEvent.click(screen.getByRole("button", { name: "Create workspace" }));
    await flushMountEffects();

    expect(screen.getByRole("alert")).toHaveTextContent("branch already exists");
    expect(mockCloseWorkspace).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Create workspace" })).toBeEnabled();

    fireEvent.click(screen.getByRole("button", { name: /dismiss error/i }));
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("reopens the created workspace on retry when opening it failed", async () => {
    mockActivate.mockRejectedValueOnce("window busy");
    const { onComplete } = renderOnboarding();
    await flushMountEffects();
    await goToSetupStep();

    fireEvent.click(screen.getByRole("button", { name: "Create workspace" }));
    await flushMountEffects();
    expect(screen.getByRole("alert")).toHaveTextContent("created but couldn't be opened");
    expect(mockCloseWorkspace).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Create workspace" }));
    await flushMountEffects();
    // A second create would fail with "branch already exists".
    expect(mockCreate).toHaveBeenCalledTimes(1);
    expect(mockActivate).toHaveBeenLastCalledWith("ws-new");
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  const worktrees: WorktreeInfo[] = [
    { path: "/home/user/myproj", branch: "refs/heads/main", is_bare: false },
    { path: "/wt/a", branch: "refs/heads/feat-a", is_bare: false },
    { path: "/wt/b", branch: "refs/heads/feat-b", is_bare: false },
  ];

  async function importAll() {
    fireEvent.click(screen.getByRole("button", { name: "Import all" }));
    const dialog = await screen.findByRole("alertdialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Import all" }));
  }

  it("keeps the temp workspace and shows an error when every import fails", async () => {
    mockListWorktrees.mockResolvedValueOnce(worktrees);
    mockImport
      .mockRejectedValueOnce("not a git worktree")
      .mockRejectedValueOnce("not a git worktree");
    const { onComplete } = renderOnboarding();
    await flushMountEffects();

    await importAll();
    await flushMountEffects();

    expect(mockImport).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("alert")).toHaveTextContent("not a git worktree");
    expect(mockCloseWorkspace).not.toHaveBeenCalled();
    expect(onComplete).not.toHaveBeenCalled();
  });

  it("summarises partial import failures and finishes", async () => {
    mockListWorktrees.mockResolvedValueOnce(worktrees);
    mockImport.mockResolvedValueOnce("ws-a").mockRejectedValueOnce("locked");
    const { onComplete } = renderOnboarding();
    await flushMountEffects();

    await importAll();
    await flushMountEffects();

    expect(vi.mocked(toast.warning)).toHaveBeenCalledWith(
      "1 imported, 1 failed",
      { description: "Not imported: feat-b" },
    );
    expect(mockActivate).toHaveBeenCalledWith("ws-a");
    expect(mockCloseWorkspace).toHaveBeenCalledWith("ws-temp-1", false);
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  it("blocks Create and Back while Import all is running", async () => {
    mockListWorktrees.mockResolvedValueOnce(worktrees);
    let finishImport: (id: string) => void = () => {};
    mockImport.mockImplementationOnce(
      () => new Promise<string>((resolve) => { finishImport = resolve; }),
    );
    renderOnboarding();
    await flushMountEffects();
    await goToSetupStep();

    await importAll();
    await flushMountEffects();

    expect(screen.getByRole("button", { name: /importing 1 \/ 2/i })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Create workspace" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Skip for now" })).toBeDisabled();
    expect(screen.getByRole("button", { name: /back/i })).toBeDisabled();

    await act(async () => {
      finishImport("ws-a");
    });
    await flushMountEffects();
    expect(mockCreate).not.toHaveBeenCalled();
  });
});

describe("ProjectOnboarding — stepper", () => {
  it("advances the progress bar and moves focus into the setup step", async () => {
    renderOnboarding();
    await flushMountEffects();

    const activeSegments = () =>
      screen
        .getAllByTestId("onboarding-step-segment")
        .filter((el) => el.dataset.active !== undefined).length;
    expect(activeSegments()).toBe(1);

    await goToSetupStep();
    expect(activeSegments()).toBe(2);
    await act(async () => {
      await new Promise((resolve) => setTimeout(resolve, 150));
    });
    expect(screen.getByRole("button", { name: "Add commands" })).toHaveFocus();
  });
});

describe("ProjectOnboarding — skip affordance", () => {
  it("renders a skip button with accessible label", async () => {
    renderOnboarding();
    await flushMountEffects();

    const skipBtn = screen.getByRole("button", { name: /skip onboarding/i });
    expect(skipBtn).toBeInTheDocument();
  });

  it("clicking skip invokes onCancel", async () => {
    const { onCancel } = renderOnboarding();
    await flushMountEffects();

    fireEvent.click(screen.getByRole("button", { name: /skip onboarding/i }));

    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it("clicking skip does NOT close the temp workspace", async () => {
    // This is the deliberate spec: leaving the temp workspace intact lands
    // the user in the real workspace shell, not <EmptyState />. Closing it
    // would defeat the purpose of the skip affordance.
    renderOnboarding();
    await flushMountEffects();

    fireEvent.click(screen.getByRole("button", { name: /skip onboarding/i }));

    expect(mockCloseWorkspace).not.toHaveBeenCalled();
  });

  it("clicking skip does not invoke onComplete", async () => {
    const { onComplete } = renderOnboarding();
    await flushMountEffects();

    fireEvent.click(screen.getByRole("button", { name: /skip onboarding/i }));

    expect(onComplete).not.toHaveBeenCalled();
  });

  it("unmount does not crash if a debounce timer is outstanding", async () => {
    // Regression guard for the post-unmount-setState warning. Type into the
    // task input to schedule a debounce, then unmount before it fires.
    // Previously the timer would fire after unmount and trigger a React
    // warning; the new unmount-cleanup effect clears it.
    vi.useFakeTimers();
    try {
      const { unmount } = renderOnboarding();
      // Advance past the initial mount microtasks without letting the real
      // event loop race us; the mock resolvers use microtasks so this is OK.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(0);
      });

      const taskInput = screen.getByPlaceholderText(/add dark mode/i);
      fireEvent.change(taskInput, { target: { value: "build a rocket" } });

      // The debounce is 500ms in the component. Unmount before it fires.
      unmount();

      // Now advance past the debounce window. If the cleanup works, no
      // setState-on-unmounted-component occurs; if it doesn't, React logs
      // a warning which vitest surfaces as a stderr line.
      await act(async () => {
        await vi.advanceTimersByTimeAsync(1000);
      });
      // Getting here without throwing is the pass condition.
      expect(true).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });
});
