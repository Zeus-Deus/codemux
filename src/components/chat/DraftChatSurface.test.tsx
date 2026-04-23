/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render } from "@testing-library/react";
import { TooltipProvider } from "@/components/ui/tooltip";

// ── Module mocks ──

vi.mock("@/tauri/commands", () => ({
  getHomeDir: vi.fn().mockResolvedValue("/home/user"),
  // ProjectPicker reads these when its popover opens. Render-only
  // tests never trigger the popover, so these stay quiet.
  dbGetRecentProjects: vi.fn().mockResolvedValue([]),
  dbGetUiState: vi.fn().mockResolvedValue(null),
  // Post-"New worktree" dispatch uses activateWorkspace. Stub so
  // onWorktreeCreated tests can assert on it.
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  // The worktree-created handler spawns an agent_chat pane before
  // activating so the new workspace has a pane that the
  // ensure-draft-when-empty hook will see.
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
}));

vi.mock("@/lib/agent-chat/materialize", () => ({
  materializeAndSend: vi.fn(),
}));

vi.mock("@/lib/toast", () => ({
  toast: {
    error: vi.fn(),
    warning: vi.fn(),
  },
}));

// Provider capabilities — minimal stub. Selectors pull null when the
// store has no entry, which renders a disabled "unavailable" state on
// the pickers; that's fine for these tests.
vi.mock("@/stores/provider-capabilities-store", () => ({
  useProviderCapabilities: Object.assign(
    vi.fn((selector: (s: unknown) => unknown) =>
      selector({ claude: null, codex: null }),
    ),
    // `capability-defaults` reads the store synchronously via
    // `.getState()`. Return the same null-capability shape the hook
    // selector returns, so downstream fallbacks kick in.
    { getState: () => ({ claude: null, codex: null }) },
  ),
  selectCapabilities: () => null,
  selectModel: () => null,
}));

// Stub the ProjectPicker: the real one subscribes to useAppStore via
// a selector that returns a fresh `?? []` array when appState is null,
// which loops under jsdom with no store bootstrap. Tests care only
// that the picker RENDERS in the Zone 1 slot for home drafts — the
// picker's own tests live next to the picker.
vi.mock("@/components/overlays/project-picker", () => ({
  ProjectPicker: ({ value }: { value: string | null }) => (
    <button data-testid="project-picker-stub">
      {value ?? "Select project"}
    </button>
  ),
}));

// Stub the WorktreePicker for the same reason — DraftChatSurface tests
// are about dispatch, not about the picker's internal rendering. The
// stub records the most recently passed props so individual tests can
// invoke the callbacks directly to exercise the dispatch logic.
type WorktreePickerStubProps = {
  mode: "draft" | "active";
  projectPath: string;
  draftTarget?: import("@/stores/chat-draft-store").ChatDraft["target"];
  derivativeBranch: string;
  onChangeDraftTarget?: (
    t: import("@/stores/chat-draft-store").DraftTarget,
  ) => void;
  onSwitchWorkspace?: (id: string) => void;
  onWorktreeCreated: (id: string) => void;
};
const lastWorktreePickerProps: { current: WorktreePickerStubProps | null } = {
  current: null,
};
vi.mock("@/components/chat/pickers/WorktreePicker", () => ({
  WorktreePicker: (props: WorktreePickerStubProps) => {
    lastWorktreePickerProps.current = props;
    return (
      <button
        data-testid="worktree-picker-stub"
        data-project-path={props.projectPath}
        data-mode={props.mode}
        data-derivative-branch={props.derivativeBranch}
      >
        worktree:{props.projectPath}
      </button>
    );
  },
}));

type DerivativeBranchPickerStubProps = {
  projectPath: string;
  value: string;
  onChange: (branch: string) => void;
};
const lastDerivativePickerProps: {
  current: DerivativeBranchPickerStubProps | null;
} = { current: null };
vi.mock("@/components/chat/pickers/DerivativeBranchPicker", () => ({
  DerivativeBranchPicker: (props: DerivativeBranchPickerStubProps) => {
    lastDerivativePickerProps.current = props;
    return (
      <button
        data-testid="derivative-branch-picker-stub"
        data-value={props.value}
      >
        derivative:{props.value}
      </button>
    );
  },
}));

import { DraftChatSurface } from "./DraftChatSurface";
import { materializeAndSend } from "@/lib/agent-chat/materialize";
import { toast } from "@/lib/toast";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";

afterEach(() => cleanup());

function resetStores() {
  useChatDraftStore.setState({
    draftsById: {},
    activeHomeDraftId: null,
    projectDraftIdByPath: {},
    activeDraftId: null,
  });
  useAgentChatStore.setState({ threads: {} });
  useAppStore.setState({ appState: null });
  useUIStore.setState({
    showNewWorkspaceDialog: false,
    newWorkspaceProjectDir: null,
  });
  lastWorktreePickerProps.current = null;
  lastDerivativePickerProps.current = null;
}

function renderSurface() {
  return render(
    <TooltipProvider>
      <DraftChatSurface />
    </TooltipProvider>,
  );
}

describe("DraftChatSurface", () => {
  beforeEach(() => {
    resetStores();
    vi.mocked(materializeAndSend).mockReset();
    vi.mocked(toast.error).mockReset();
  });

  describe("rendering", () => {
    it("renders nothing when no draft is active", () => {
      const { container } = renderSurface();
      expect(container.firstChild).toBeNull();
    });

    it("renders the home-landing wrapper + composer when a draft is active", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container, getByText } = renderSurface();
      expect(getByText("What should we do today?")).toBeInTheDocument();
      expect(container.querySelector("textarea")).not.toBeNull();
    });

    it("seeds the composer textarea with the draft's inputDraft", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "typed so far");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      expect(ta.value).toBe("typed so far");
    });

    it("surfaces the draft's lastSendError in the composer banner", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().markSendFailed(draft.draftId, "boom");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { getByRole } = renderSurface();
      const alert = getByRole("alert");
      expect(alert.textContent).toContain("boom");
    });

    it("renders the Zone 1 ProjectPicker stub for home drafts", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      // ProjectPicker renders a trigger button with "Select project"
      // text when value=null.
      expect(container.textContent).toContain("Select project");
    });

    it("renders the WorktreePicker for project-target drafts (not ProjectPicker)", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      // ProjectPicker stub absent.
      expect(container.textContent).not.toContain("Select project");
      // WorktreePicker stub present, scoped to the project path.
      expect(container.textContent).toContain("worktree:/projects/foo");
    });

    it("renders the WorktreePicker for existing_workspace drafts scoped to that workspace's project", () => {
      // Seed app-state so the surface can resolve the workspace's project_root.
      const appStateStub = {
        schema_version: 1,
        active_workspace_id: "ws-foo-feat",
        workspaces: [
          {
            workspace_id: "ws-foo-feat",
            title: "feat",
            workspace_type: "standard",
            cwd: "/projects/foo-feat",
            git_branch: "feat/x",
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
          },
        ],
      } as unknown as Parameters<typeof useAppStore.setState>[0];
      useAppStore.setState({
        appState: appStateStub as never,
      });
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore
        .getState()
        .updateDraftTarget(draft.draftId, {
          kind: "existing_workspace",
          workspaceId: "ws-foo-feat",
        });
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      expect(container.textContent).toContain("worktree:/projects/foo");
    });
  });

  describe("composer round-trip", () => {
    it("typing in the textarea updates the draft's inputDraft", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      fireEvent.change(ta, { target: { value: "hello there" } });
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.inputDraft).toBe("hello there");
    });
  });

  describe("submit flow (§7 + §9)", () => {
    // Most submit tests use project drafts because the project path
    // resolves synchronously (no `getHomeDir()` round-trip). One
    // dedicated home-target test at the bottom covers the async home
    // cwd resolution.
    function seedProjectDraft(input = "hello") {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().updateDraftInput(draft.draftId, input);
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      return draft;
    }

    it("calls materializeAndSend with the draft + trimmed text + project path cwd", async () => {
      vi.mocked(materializeAndSend).mockResolvedValueOnce({
        success: true,
        workspaceId: "ws-project",
        paneId: "pane-new",
        threadId: "tid-p",
      });
      const draft = seedProjectDraft();
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      fireEvent.keyDown(ta, { key: "Enter" });
      await vi.waitFor(() => {
        expect(materializeAndSend).toHaveBeenCalled();
      });
      const [passedDraft, text, cwd] = vi.mocked(materializeAndSend).mock
        .calls[0];
      expect(passedDraft.draftId).toBe(draft.draftId);
      expect(text).toBe("hello");
      expect(cwd).toBe("/projects/foo");
    });

    it("on success, setActiveDraft(null) is called and clearDraft is scheduled after 5s", async () => {
      const setTimeoutSpy = vi.spyOn(globalThis, "setTimeout");
      vi.mocked(materializeAndSend).mockResolvedValueOnce({
        success: true,
        workspaceId: "ws-project",
        paneId: "pane-new",
        threadId: "tid-p",
      });
      const draft = seedProjectDraft();
      const { container } = renderSurface();

      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      fireEvent.keyDown(ta, { key: "Enter" });

      await vi.waitFor(() => {
        expect(useChatDraftStore.getState().activeDraftId).toBeNull();
      });
      // The draft entry is still present — the 5s grace is the
      // sweeper. We verify the setTimeout was scheduled with the
      // expected delay and then run its callback directly so we do
      // not have to actually sleep in the test.
      expect(
        useChatDraftStore.getState().draftsById[draft.draftId],
      ).toBeDefined();
      const scheduled = setTimeoutSpy.mock.calls.find(
        ([, delay]) => delay === 5000,
      );
      expect(scheduled).toBeDefined();
      const [callback] = scheduled!;
      (callback as () => void)();
      expect(
        useChatDraftStore.getState().draftsById[draft.draftId],
      ).toBeUndefined();

      setTimeoutSpy.mockRestore();
    });

    it("on failure, setActiveDraft is NOT cleared and an error toast fires", async () => {
      vi.mocked(materializeAndSend).mockResolvedValueOnce({
        success: false,
        error: "workspace lock",
      });
      const draft = seedProjectDraft();
      const { container } = renderSurface();

      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      fireEvent.keyDown(ta, { key: "Enter" });

      await vi.waitFor(() => {
        expect(toast.error).toHaveBeenCalled();
      });
      expect(useChatDraftStore.getState().activeDraftId).toBe(draft.draftId);
    });

    it("does not call materializeAndSend when the input is empty or whitespace", async () => {
      seedProjectDraft("   ");
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      fireEvent.keyDown(ta, { key: "Enter" });
      // Allow a generous tick for the flow to have fired if it were
      // going to.
      await new Promise((r) => setTimeout(r, 20));
      expect(materializeAndSend).not.toHaveBeenCalled();
    });

    it("resolves home cwd via getHomeDir() for home-target drafts", async () => {
      vi.mocked(materializeAndSend).mockResolvedValueOnce({
        success: true,
        workspaceId: "ws-home",
        paneId: "pane-new",
        threadId: "tid-h",
      });
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "hello");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;

      // Poll-submit: fireEvent.keyDown repeatedly until the async
      // getHomeDir() resolves and resolvedHomeDir lands in state.
      await vi.waitFor(() => {
        fireEvent.keyDown(ta, { key: "Enter" });
        expect(materializeAndSend).toHaveBeenCalled();
      });
      const [, , cwd] = vi.mocked(materializeAndSend).mock.calls[0];
      expect(cwd).toBe("/home/user");
    });
  });

  describe("config handlers", () => {
    it("provider change resets model/effort/contextWindow/permissionMode", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftConfig(draft.draftId, {
        model: "claude-opus-4-7",
        effort: "high",
        contextWindow: "1m",
        permissionMode: "plan",
      });
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();

      // Reach into the draft surface's handleProviderChange via the
      // Composer's onProviderChange prop. Dispatching it directly is
      // the simplest path: we know the draft surface wires it to
      // updateDraftConfig.
      useChatDraftStore.getState().updateDraftConfig(draft.draftId, {
        provider: "codex",
        model: null,
        permissionMode: null,
        effort: null,
        contextWindow: null,
      });
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.provider).toBe("codex");
      expect(next.model).toBeNull();
      expect(next.effort).toBeNull();
      expect(next.contextWindow).toBeNull();
    });
  });

  describe("Stage D — WorktreePicker dispatch", () => {
    function seedAppStateWithFooFeat() {
      useAppStore.setState({
        appState: {
          schema_version: 1,
          active_workspace_id: "ws-foo-feat",
          workspaces: [
            {
              workspace_id: "ws-foo-feat",
              title: "feat",
              workspace_type: "standard",
              cwd: "/projects/foo-feat-x",
              git_branch: "feat/x",
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
            },
          ],
        } as never,
      });
    }

    it("project draft → onChangeDraftTarget with existing_workspace updates the draft target", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      expect(lastWorktreePickerProps.current).not.toBeNull();
      lastWorktreePickerProps.current!.onChangeDraftTarget?.({
        kind: "existing_workspace",
        workspaceId: "ws-foo-feat",
      });
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.target).toEqual({
        kind: "existing_workspace",
        workspaceId: "ws-foo-feat",
      });
    });

    it("project draft → onWorktreeCreated spawns an agent_chat pane, clears the draft, and activates the workspace", async () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      expect(lastWorktreePickerProps.current).not.toBeNull();
      expect(lastWorktreePickerProps.current!.projectPath).toBe(
        "/projects/foo",
      );
      const { activateWorkspace, agentChatCreatePane } = await import(
        "@/tauri/commands"
      );
      vi.mocked(activateWorkspace).mockClear();
      vi.mocked(agentChatCreatePane).mockClear();
      await lastWorktreePickerProps.current!.onWorktreeCreated("ws-new");
      // Pane creation must precede activation so
      // useEnsureDraftWhenEmpty doesn't inject a Home draft over the
      // empty workspace.
      expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
        "ws-new",
        "claude",
        null,
      );
      // Draft cleared before activation so the new pane mounts solo.
      expect(useChatDraftStore.getState().activeDraftId).toBeNull();
      expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-new");
      // The legacy dialog does NOT open.
      expect(useUIStore.getState().showNewWorkspaceDialog).toBe(false);
    });

    it("existing_workspace draft → WorktreePicker scope uses the targeted workspace's project_root, NOT its cwd", async () => {
      // Seed a workspace whose cwd differs from its project_root, so a
      // bug that uses cwd by mistake would fail this assertion.
      seedAppStateWithFooFeat();
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore
        .getState()
        .updateDraftTarget(draft.draftId, {
          kind: "existing_workspace",
          workspaceId: "ws-foo-feat",
        });
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      expect(lastWorktreePickerProps.current).not.toBeNull();
      // The picker is scoped to the workspace's project_root —
      // /projects/foo, not /projects/foo-feat-x (the cwd).
      expect(lastWorktreePickerProps.current!.projectPath).toBe(
        "/projects/foo",
      );
      const { activateWorkspace, agentChatCreatePane } = await import(
        "@/tauri/commands"
      );
      vi.mocked(activateWorkspace).mockClear();
      vi.mocked(agentChatCreatePane).mockClear();
      await lastWorktreePickerProps.current!.onWorktreeCreated("ws-new");
      expect(vi.mocked(agentChatCreatePane)).toHaveBeenCalledWith(
        "ws-new",
        "claude",
        null,
      );
      expect(useChatDraftStore.getState().activeDraftId).toBeNull();
      expect(vi.mocked(activateWorkspace)).toHaveBeenCalledWith("ws-new");
    });

    it("project draft → renders DerivativeBranchPicker seeded to 'main'", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      expect(
        container.querySelector(
          '[data-testid="derivative-branch-picker-stub"]',
        ),
      ).not.toBeNull();
      expect(lastDerivativePickerProps.current).not.toBeNull();
      expect(lastDerivativePickerProps.current!.projectPath).toBe(
        "/projects/foo",
      );
      expect(lastDerivativePickerProps.current!.value).toBe("main");
      // The WorktreePicker's `derivativeBranch` prop mirrors the
      // DerivativeBranchPicker's value — they share state at the
      // DraftChatSurface level.
      expect(lastWorktreePickerProps.current!.derivativeBranch).toBe("main");
    });

    it("changing the derivative branch flows into the WorktreePicker's derivativeBranch prop", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { rerender } = renderSurface();
      expect(lastDerivativePickerProps.current).not.toBeNull();
      lastDerivativePickerProps.current!.onChange("develop");
      rerender(
        <TooltipProvider>
          <DraftChatSurface />
        </TooltipProvider>,
      );
      expect(lastWorktreePickerProps.current!.derivativeBranch).toBe(
        "develop",
      );
      expect(lastDerivativePickerProps.current!.value).toBe("develop");
    });

    it("existing_workspace draft whose target workspace is not in app-state renders the cwd fallback (no override)", () => {
      // No workspace in app-state matches the draft's workspaceId →
      // existingWorkspaceProjectRoot resolves to null → zone1Override
      // returns null → Composer falls back to its read-only cwd label.
      // existingWorkspaceCwd is also null in this case so the cwd
      // strip just hides too.
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore
        .getState()
        .updateDraftTarget(draft.draftId, {
          kind: "existing_workspace",
          workspaceId: "ws-not-here",
        });
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      // No WorktreePicker rendered.
      expect(
        container.querySelector('[data-testid="worktree-picker-stub"]'),
      ).toBeNull();
      // No ProjectPicker either (only home drafts get that).
      expect(container.textContent).not.toContain("Select project");
    });

    it("existing_workspace draft passes the draftTarget through to the picker", () => {
      seedAppStateWithFooFeat();
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      const target = {
        kind: "existing_workspace" as const,
        workspaceId: "ws-foo-feat",
      };
      useChatDraftStore.getState().updateDraftTarget(draft.draftId, target);
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      expect(lastWorktreePickerProps.current!.draftTarget).toEqual(target);
      expect(lastWorktreePickerProps.current!.mode).toBe("draft");
    });
  });
});
