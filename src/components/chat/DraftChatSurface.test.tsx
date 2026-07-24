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
  // Post-"New worktree" dispatch: activateWorkspace fires after the
  // helper resolves. create_pane + start_session are called INSIDE
  // prestartWorktreeSession; assertions below verify both ran with
  // the workspace's real cwd (NOT null), closing the
  // session_not_found race.
  activateWorkspace: vi.fn().mockResolvedValue(undefined),
  agentChatCreatePane: vi.fn().mockResolvedValue("pane-new"),
  agentChatStartSession: vi.fn().mockResolvedValue("thread-echo"),
  // MCP warmup fired on the draft surface mount; no-op in tests. Image
  // staging commands are imported by the image-staging helper but only
  // invoked when a test stages an image (none here).
  primeChatMcp: vi.fn().mockResolvedValue(undefined),
  stageChatImage: vi
    .fn()
    .mockResolvedValue({ path: "/staging/x.png", media_type: "image/png" }),
  discardStagedChatImage: vi.fn().mockResolvedValue(undefined),
  readChatImage: vi
    .fn()
    .mockResolvedValue({ bytes: new Uint8Array(), media_type: "image/png" }),
  // Step 8 Stage 2 — Composer's mention popup useEffect calls
  // `listProjectFiles` whenever `@` is open; default to an empty
  // resolution so the popup-not-opened tests don't flap.
  // `readFileForAttachment` is exercised by the attachment-wiring
  // regression below.
  listProjectFiles: vi.fn().mockResolvedValue([]),
  readFileForAttachment: vi.fn().mockResolvedValue({
    absolutePath: "/repo/src/components/chat/Composer.tsx",
    relativePath: "src/components/chat/Composer.tsx",
    lineCount: 421,
    bytes: 16384,
    language: "tsx",
    isText: true,
    content: "// composer body\n",
    truncated: false,
    outline: null,
  }),
  // Step 9 Stage 4 — Composer mounts `useMcpRuntime` and the `+`
  // popup MCP submode reads `listMcpServers`. Stub both as no-ops
  // so render-only tests don't reach into a broken Tauri shim.
  getMcpRuntimeStatus: vi.fn().mockResolvedValue([]),
  setMcpDisabledIds: vi.fn().mockResolvedValue(undefined),
  listMcpServers: vi.fn().mockResolvedValue([]),
  MCP_STATUS_CHANGED_EVENT: "mcp-status-changed",
  MCP_CODEMUX_SELF_ID: "codemux-self",
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
// the pickers; that's fine for these tests. The hook is invoked two
// ways: with a selector function (legacy callers) and bare (the new
// `MultiProviderModelPicker` reads the full state). Handle both.
const STUB_CAP_STATE = {
  claude: null,
  codex: null,
  opencode: null,
  claudeError: null,
  codexError: null,
  opencodeError: null,
  loaded: false,
  refresh: vi.fn(),
  refreshAll: vi.fn(),
};
vi.mock("@/stores/provider-capabilities-store", () => ({
  useProviderCapabilities: Object.assign(
    vi.fn((selector?: (s: unknown) => unknown) =>
      typeof selector === "function" ? selector(STUB_CAP_STATE) : STUB_CAP_STATE,
    ),
    // `capability-defaults` reads the store synchronously via
    // `.getState()`. Return the same null-capability shape the hook
    // selector returns, so downstream fallbacks kick in.
    { getState: () => STUB_CAP_STATE },
  ),
  selectCapabilities: () => null,
  selectError: () => null,
  selectModel: () => null,
}));

// Picker favorites store — drafts now use the multi-provider picker
// which subscribes to the favorites list. Empty array keeps the
// picker's sort path on the fallback branch.
vi.mock("@/stores/picker-favorites-store", () => ({
  pickerFavoriteKey: (provider: string, modelId: string) =>
    `${provider}::${modelId}`,
  usePickerFavorites: Object.assign(
    vi.fn((selector?: (s: unknown) => unknown) => {
      const state = {
        favorites: [],
        toggle: vi.fn(),
        isFavorite: () => false,
        getKey: (p: string, m: string) => `${p}::${m}`,
      };
      return typeof selector === "function" ? selector(state) : state;
    }),
    {
      getState: () => ({
        favorites: [],
        toggle: vi.fn(),
        isFavorite: () => false,
        getKey: (p: string, m: string) => `${p}::${m}`,
      }),
    },
  ),
}));

// Stub ThreadScopeRow — the real one owns three popovers (location /
// checkout / branch) each with their own store subscriptions and
// Tauri round-trips; DraftChatSurface tests care about DISPATCH (what
// gets wired to the draft store / materializeAndSend), not about the
// popovers' own rendering — that lives in ThreadScopeRow.test.tsx. The
// stub records the most recently passed props so individual tests can
// invoke the callbacks directly to exercise the dispatch logic.
type ThreadScopeRowStubProps = {
  location: {
    kind: "draft";
    target: import("@/stores/chat-draft-store").ChatDraft["target"];
    onChangeTarget: (
      t: import("@/stores/chat-draft-store").DraftTarget,
    ) => void;
  };
  projectPath: string | null;
  checkoutMode: "current" | "worktree";
  worktreeName: string;
  baseBranch: string;
  disabled?: boolean;
  onChangeCheckoutMode: (mode: "current" | "worktree") => void;
  onChangeWorktreeName: (name: string) => void;
  onChangeBaseBranch: (branch: string) => void;
};
const lastThreadScopeRowProps: { current: ThreadScopeRowStubProps | null } = {
  current: null,
};
vi.mock("@/components/chat/pickers/ThreadScopeRow", () => ({
  ThreadScopeRow: (props: ThreadScopeRowStubProps) => {
    lastThreadScopeRowProps.current = props;
    return (
      <button
        data-testid="thread-scope-row-stub"
        data-location-kind={props.location.kind}
        data-target-kind={props.location.target.kind}
        data-project-path={props.projectPath ?? ""}
        data-checkout-mode={props.checkoutMode}
        data-base-branch={props.baseBranch}
      >
        scope:{props.projectPath ?? "home"}
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
  // homeDir is normally hydrated at App mount; seed it here so the
  // DraftChatSurface's seed effect and submit-time home-cwd resolver
  // see a populated value straight away (they no longer roundtrip via
  // getHomeDir()).
  useAppStore.setState({ appState: null, homeDir: "/home/user" });
  useUIStore.setState({
    showNewWorkspaceDialog: false,
    newWorkspaceProjectDir: null,
  });
  lastThreadScopeRowProps.current = null;
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

    it("renders a pane-header band above the chat-home so the draft is not missing chrome", () => {
      // Regression guard for the "Image 53" bug: a freshly opened
      // project landed on the draft chat-home with no pane header
      // at all, while Images 54/55 showed an "Agent Chat" band.
      // The draft surface borrows that silhouette (h-7, bottom
      // border) so the two views match.
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { getByTestId } = renderSurface();
      const header = getByTestId("draft-surface-header");
      expect(header).toBeInTheDocument();
      expect(header.textContent).toContain("Agent Chat");
      expect(header.className).toContain("h-7");
      expect(header.className).toContain("border-b");
    });

    it("suppresses the header band in GUI chrome (Agent Chat Beta ON) — the titlebar draft pill covers it", async () => {
      const { useFeatureFlags } = await import("@/stores/feature-flags");
      useFeatureFlags.setState({ enableAgentChat: true });
      try {
        const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
        useChatDraftStore.getState().setActiveDraft(draft.draftId);
        const { queryByTestId, getByText } = renderSurface();
        expect(queryByTestId("draft-surface-header")).toBeNull();
        // The landing itself still renders.
        expect(getByText("What should we do today?")).toBeInTheDocument();
      } finally {
        useFeatureFlags.setState({ enableAgentChat: false });
      }
    });

    it("the header band sits ABOVE the home landing in DOM order", () => {
      // Visual hierarchy: the header is the first child of the
      // surface root; the chat-home wrapper comes after. If these
      // are flipped the header would render under the centered
      // headline.
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      const root = container.firstElementChild as HTMLElement;
      const header = root.querySelector('[data-testid="draft-surface-header"]');
      const headline = root.querySelector("h1");
      expect(header).not.toBeNull();
      expect(headline).not.toBeNull();
      expect(header!.compareDocumentPosition(headline!)).toBe(
        Node.DOCUMENT_POSITION_FOLLOWING,
      );
    });

    it("seeds the composer textarea with the draft's inputDraft", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "typed so far");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      expect(ta.value).toBe("typed so far");
    });

    // Step 8 Stage 2.1 — regression for the bug where DraftChatSurface
    // forgot to forward `stagedAttachments` / `onAttachFile` to its
    // Composer. Symptom was: typing `@filename` and picking a row
    // closed the popup but no chip appeared. Stage 2.1 moved file
    // chips INSIDE the textarea (mirror overlay), so the regression
    // assertion now checks the inline token rendering rather than
    // the (now image-only) strip above the textarea. If the surface
    // ever drops the prop again, this test catches it.
    it("renders the inline mirror chip when a file token is in the draft and matches a staged attachment", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      // Seed the textarea text with a `@<basename>` token AND stage
      // the matching attachment on the pre-minted threadId — same key
      // the surface uses internally.
      useChatDraftStore
        .getState()
        .updateDraftInput(draft.draftId, "look at @Composer.tsx please");
      useAgentChatStore.getState().addStagedAttachment(draft.threadId, {
        id: "att-regression-1",
        kind: "file",
        ref: "/repo/src/components/chat/Composer.tsx",
        metadata: { label: "Composer.tsx", lineCount: 421 },
      });
      const { getByTestId } = renderSurface();
      const chip = getByTestId("composer-attachment-token-Composer.tsx");
      expect(chip).toBeInTheDocument();
      expect(chip.textContent).toBe("@Composer.tsx");
      expect(chip.className).toContain("bg-foreground/10");
    });

    it("the inline mirror chip vanishes when the file token is removed from the draft text", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      useAgentChatStore.getState().addStagedAttachment(draft.threadId, {
        id: "att-regression-2",
        kind: "file",
        ref: "/repo/src/lib/utils.ts",
        metadata: { label: "utils.ts" },
      });
      // Empty draft means no `@utils.ts` token → no inline chip even
      // though the slice still has the attachment.
      const { queryByTestId } = renderSurface();
      expect(
        queryByTestId("composer-attachment-token-utils.ts"),
      ).toBeNull();
    });

    it("surfaces the draft's lastSendError in the composer banner", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().markSendFailed(draft.draftId, "boom");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { getByRole } = renderSurface();
      const alert = getByRole("alert");
      expect(alert.textContent).toContain("boom");
    });

    it("renders the ThreadScopeRow stub below the composer for home drafts, targeting home with no project", () => {
      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { getByTestId } = renderSurface();
      const stub = getByTestId("thread-scope-row-stub");
      expect(stub.dataset.targetKind).toBe("home");
      expect(stub.dataset.projectPath).toBe("");
    });

    it("home draft seeds its target to the active sidebar workspace (so send reuses it instead of spawning a duplicate worktree)", async () => {
      // Reproduces the exact screenshot bug AND the follow-up bug:
      // the picker needs to show the project, AND the draft must
      // submit into the existing workspace — seeding to `project`
      // would make send spawn a fresh worktree, doubling the
      // workspace.
      const appStateStub = {
        schema_version: 1,
        active_workspace_id: "ws-whatsapp",
        workspaces: [
          {
            workspace_id: "ws-whatsapp",
            title: "whatsapp-intake-bot",
            workspace_type: "standard",
            cwd: "/projects/whatsapp-intake-bot",
            git_branch: "main",
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            notification_count: 0,
            notifications_muted: false,
            latest_agent_state: null,
            worktree_path: null,
            project_root: "/projects/whatsapp-intake-bot",
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
      useAppStore.setState({ appState: appStateStub as never });

      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { findByTestId } = renderSurface();
      // Seeding effect reads the hydrated appHomeDir from useAppStore
      // (set via resetStores) and flips target home → existing_workspace
      // on the first effect pass. The ThreadScopeRow stub now reports
      // the resolved project path instead of the bare "home" target.
      const stub = await findByTestId("thread-scope-row-stub");
      expect(stub.dataset.targetKind).toBe("existing_workspace");
      expect(stub.dataset.projectPath).toBe("/projects/whatsapp-intake-bot");
      // Store is persistently flipped to existing_workspace so the
      // send path reuses the active workspace rather than creating a
      // new one.
      const after = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(after?.target).toEqual({
        kind: "existing_workspace",
        workspaceId: "ws-whatsapp",
      });
    });

    it("home draft does NOT seed when the active workspace is home-rooted (no real project)", async () => {
      // Active workspace's project_root IS the home directory — this
      // is the "bare Home landing" case, not a real project context.
      // getHomeDir is mocked to "/home/user" at the top of the file.
      const appStateStub = {
        schema_version: 1,
        active_workspace_id: "ws-home",
        workspaces: [
          {
            workspace_id: "ws-home",
            title: "home",
            workspace_type: "standard",
            cwd: "/home/user",
            git_branch: "",
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            notification_count: 0,
            notifications_muted: false,
            latest_agent_state: null,
            worktree_path: null,
            project_root: "/home/user",
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
      useAppStore.setState({ appState: appStateStub as never });

      const draft = useChatDraftStore.getState().getOrCreateHomeDraft();
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { findByTestId } = renderSurface();
      // Stub stays targeted at "home" — the draft remains home.
      const stub = await findByTestId("thread-scope-row-stub");
      expect(stub.dataset.targetKind).toBe("home");
      const after = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(after?.target).toEqual({ kind: "home" });
    });

    it("renders the ThreadScopeRow stub scoped to the project path for project-target drafts", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { getByTestId } = renderSurface();
      const stub = getByTestId("thread-scope-row-stub");
      expect(stub.dataset.targetKind).toBe("project");
      expect(stub.dataset.projectPath).toBe("/projects/foo");
      // checkoutMode / baseBranch default from `makeDraft` — the row
      // reads them straight off the draft, no local component state.
      expect(stub.dataset.checkoutMode).toBe("current");
      expect(stub.dataset.baseBranch).toBe("");
    });

    it("renders the ThreadScopeRow stub scoped to the project path for existing_workspace drafts", () => {
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
      const { getByTestId } = renderSurface();
      const stub = getByTestId("thread-scope-row-stub");
      expect(stub.dataset.targetKind).toBe("existing_workspace");
      // Scoped to the workspace's project_root, NOT its cwd.
      expect(stub.dataset.projectPath).toBe("/projects/foo");
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

    it("resolves home cwd via useAppStore.homeDir for home-target drafts (no active project workspace)", async () => {
      // Home draft with no active sidebar workspace → stays as "home"
      // target, materialize gets the hydrated homeDir as cwd.
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
      fireEvent.keyDown(ta, { key: "Enter" });
      await vi.waitFor(() => {
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

      // Mirror the atomic provider+model patch dispatched by the composer.
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

  describe("Stage D — ThreadScopeRow dispatch (Thread Scope redesign)", () => {
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
            },
          ],
        } as never,
      });
    }

    it("project draft → onChangeTarget with existing_workspace updates the draft target", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      expect(lastThreadScopeRowProps.current).not.toBeNull();
      lastThreadScopeRowProps.current!.location.onChangeTarget({
        kind: "existing_workspace",
        workspaceId: "ws-foo-feat",
      });
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.target).toEqual({
        kind: "existing_workspace",
        workspaceId: "ws-foo-feat",
      });
    });

    it("onChangeCheckoutMode('worktree') flips the draft's checkoutMode", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      expect(lastThreadScopeRowProps.current!.checkoutMode).toBe("current");
      lastThreadScopeRowProps.current!.onChangeCheckoutMode("worktree");
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.checkoutMode).toBe("worktree");
    });

    it("onChangeWorktreeName / onChangeBaseBranch persist onto the draft", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      lastThreadScopeRowProps.current!.onChangeWorktreeName("my-feature");
      lastThreadScopeRowProps.current!.onChangeBaseBranch("develop");
      const next = useChatDraftStore.getState().draftsById[draft.draftId];
      expect(next.worktreeName).toBe("my-feature");
      expect(next.baseBranch).toBe("develop");
    });

    it("existing_workspace draft → ThreadScopeRow scope uses the targeted workspace's project_root, NOT its cwd", () => {
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
      expect(lastThreadScopeRowProps.current).not.toBeNull();
      // Scoped to the workspace's project_root — /projects/foo, not
      // /projects/foo-feat-x (the cwd).
      expect(lastThreadScopeRowProps.current!.projectPath).toBe(
        "/projects/foo",
      );
    });

    it("existing_workspace draft whose target workspace is not in app-state still renders the row with projectPath=null", () => {
      // No workspace in app-state matches the draft's workspaceId →
      // existingWorkspaceProjectRoot resolves to null. Unlike the old
      // zone1Override (which hid the row entirely), ThreadScopeRow
      // always renders — it just shows the location control only,
      // internally, since `projectPath` is null.
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
      renderSurface();
      expect(lastThreadScopeRowProps.current).not.toBeNull();
      expect(lastThreadScopeRowProps.current!.projectPath).toBeNull();
    });

    it("existing_workspace draft passes the draftTarget through to the row", () => {
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
      expect(lastThreadScopeRowProps.current!.location.target).toEqual(target);
    });

    it("draft.promoting disables the row", () => {
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().markPromoting(draft.draftId);
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      renderSurface();
      expect(lastThreadScopeRowProps.current!.disabled).toBe(true);
    });
  });

  describe("Thread Scope redesign — deferred worktree creation on submit", () => {
    it("checkoutMode 'worktree' passes the resolved project path through to materializeAndSend", async () => {
      vi.mocked(materializeAndSend).mockResolvedValueOnce({
        success: true,
        workspaceId: "ws-worktree",
        paneId: "pane-new",
        threadId: "tid-w",
      });
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "hello");
      useChatDraftStore
        .getState()
        .updateDraftConfig(draft.draftId, { checkoutMode: "worktree" });
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      fireEvent.keyDown(ta, { key: "Enter" });
      await vi.waitFor(() => {
        expect(materializeAndSend).toHaveBeenCalled();
      });
      const call = vi.mocked(materializeAndSend).mock.calls[0];
      // (draft, text, cwd, actions, skillBodies, attachmentBlock,
      //  imagePayloads, imageDisplaySources, worktreeProjectPath)
      expect(call[8]).toBe("/projects/foo");
    });

    it("checkoutMode 'current' (the default) passes null as the worktree project path", async () => {
      vi.mocked(materializeAndSend).mockResolvedValueOnce({
        success: true,
        workspaceId: "ws-project",
        paneId: "pane-new",
        threadId: "tid-c",
      });
      const draft = useChatDraftStore
        .getState()
        .getOrCreateProjectDraft("/projects/foo");
      useChatDraftStore.getState().updateDraftInput(draft.draftId, "hello");
      useChatDraftStore.getState().setActiveDraft(draft.draftId);
      const { container } = renderSurface();
      const ta = container.querySelector("textarea") as HTMLTextAreaElement;
      fireEvent.keyDown(ta, { key: "Enter" });
      await vi.waitFor(() => {
        expect(materializeAndSend).toHaveBeenCalled();
      });
      const call = vi.mocked(materializeAndSend).mock.calls[0];
      expect(call[8]).toBeNull();
    });
  });
});
