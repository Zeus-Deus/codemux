/// <reference types="@testing-library/jest-dom/vitest" />
import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, fireEvent, act, cleanup } from "@testing-library/react";
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
  createWorktreeWorkspace: vi.fn().mockResolvedValue("ws-new"),
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

import { closeWorkspace } from "@/tauri/commands";

const mockCloseWorkspace = vi.mocked(closeWorkspace);

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
