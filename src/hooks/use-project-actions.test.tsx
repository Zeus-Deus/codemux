/// <reference types="@testing-library/jest-dom/vitest" />
import { beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, act } from "@testing-library/react";

// ── Tauri command mocks ──
vi.mock("@/tauri/commands", () => ({
  pickFolderDialog: vi.fn(),
  checkIsGitRepo: vi.fn(),
  initGitRepo: vi.fn(),
  dbAddRecentProject: vi.fn().mockResolvedValue(undefined),
  gitCloneRepo: vi.fn(),
  createEmptyWorkspace: vi.fn().mockResolvedValue("ws-new"),
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
}));

// ── Feature-flags store mock ──
let enableAgentChatFlag = false;
let enableLazyFlag = false;
vi.mock("@/stores/feature-flags", () => ({
  useFeatureFlags: Object.assign(vi.fn(), {
    getState: () => ({
      enableAgentChat: enableAgentChatFlag,
      enableLazyWorkspaceCreation: enableLazyFlag,
      loaded: true,
    }),
  }),
}));

// ── App-store mock — drives `hasWorkspaces` lookup ──
let workspaceCount = 0;
vi.mock("@/stores/app-store", () => ({
  useAppStore: Object.assign(vi.fn(), {
    getState: () => ({
      appState: {
        workspaces: Array.from({ length: workspaceCount }, (_, i) => ({
          workspace_id: `ws-${i}`,
        })),
      },
    }),
  }),
}));

// ── UI-store mock — records setOnboardingProjectDir calls ──
const setOnboardingProjectDirMock = vi.fn();
const setShowCloneDialogMock = vi.fn();
vi.mock("@/stores/ui-store", () => ({
  useUIStore: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) =>
      selector({ setShowCloneDialog: setShowCloneDialogMock }),
    ),
    {
      getState: () => ({
        setOnboardingProjectDir: setOnboardingProjectDirMock,
      }),
    },
  ),
}));

import { useProjectActions } from "./use-project-actions";
import {
  pickFolderDialog,
  checkIsGitRepo,
  gitCloneRepo,
} from "@/tauri/commands";

describe("useProjectActions — onboarding gate", () => {
  beforeEach(() => {
    enableAgentChatFlag = false;
    enableLazyFlag = false;
    workspaceCount = 0;
    setOnboardingProjectDirMock.mockClear();
    setShowCloneDialogMock.mockClear();
    vi.mocked(pickFolderDialog).mockResolvedValue("/projects/newthing");
    vi.mocked(checkIsGitRepo).mockResolvedValue(true);
    vi.mocked(gitCloneRepo).mockResolvedValue("/projects/cloned");
  });

  describe("openProject", () => {
    it("legacy path: triggers onboarding when no workspaces exist and flags are OFF", async () => {
      enableAgentChatFlag = false;
      enableLazyFlag = false;
      workspaceCount = 0;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });
      expect(setOnboardingProjectDirMock).toHaveBeenCalledWith(
        "/projects/newthing",
      );
    });

    it("agent-chat path: SKIPS onboarding when both feature flags are ON", async () => {
      enableAgentChatFlag = true;
      enableLazyFlag = true;
      workspaceCount = 0;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });
      // Onboarding wizard must NOT fire — useEnsureDraftWhenEmpty will
      // auto-spawn an agent_chat pane on the empty workspace instead.
      expect(setOnboardingProjectDirMock).not.toHaveBeenCalled();
    });

    it("does not skip when only enableAgentChat is on (lazy flag off)", async () => {
      enableAgentChatFlag = true;
      enableLazyFlag = false;
      workspaceCount = 0;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });
      expect(setOnboardingProjectDirMock).toHaveBeenCalledWith(
        "/projects/newthing",
      );
    });

    it("does not skip when only enableLazyWorkspaceCreation is on (agent-chat flag off)", async () => {
      enableAgentChatFlag = false;
      enableLazyFlag = true;
      workspaceCount = 0;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });
      expect(setOnboardingProjectDirMock).toHaveBeenCalledWith(
        "/projects/newthing",
      );
    });

    it("never triggers onboarding when workspaces already exist, flags irrelevant", async () => {
      enableAgentChatFlag = false;
      enableLazyFlag = false;
      workspaceCount = 3;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.openProject();
      });
      expect(setOnboardingProjectDirMock).not.toHaveBeenCalled();
    });

    it("bails out cleanly when the folder picker is cancelled", async () => {
      vi.mocked(pickFolderDialog).mockResolvedValueOnce(null as unknown as string);
      const { result } = renderHook(() => useProjectActions());
      let res;
      await act(async () => {
        res = await result.current.openProject();
      });
      expect(res).toEqual({ success: false });
      expect(setOnboardingProjectDirMock).not.toHaveBeenCalled();
    });
  });

  describe("cloneProject", () => {
    it("legacy path: triggers onboarding when no workspaces exist and flags are OFF", async () => {
      enableAgentChatFlag = false;
      enableLazyFlag = false;
      workspaceCount = 0;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.cloneProject(
          "https://github.com/x/y.git",
          "/projects",
        );
      });
      expect(setOnboardingProjectDirMock).toHaveBeenCalledWith(
        "/projects/cloned",
      );
    });

    it("agent-chat path: SKIPS onboarding when both flags are ON", async () => {
      enableAgentChatFlag = true;
      enableLazyFlag = true;
      workspaceCount = 0;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.cloneProject(
          "https://github.com/x/y.git",
          "/projects",
        );
      });
      expect(setOnboardingProjectDirMock).not.toHaveBeenCalled();
    });

    it("never triggers onboarding when workspaces already exist", async () => {
      workspaceCount = 1;
      const { result } = renderHook(() => useProjectActions());
      await act(async () => {
        await result.current.cloneProject(
          "https://github.com/x/y.git",
          "/projects",
        );
      });
      expect(setOnboardingProjectDirMock).not.toHaveBeenCalled();
    });
  });
});
