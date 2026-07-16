/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";
import { useAppStore } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import type { AgentBrowserSession, AppStateSnapshot, WorkspaceSnapshot } from "@/tauri/types";

import { MessageList } from "./MessageList";

// The assistant-turn avatar renders the provider's branded mark via
// ProviderLogo, which imports the SVG assets at module load. vitest's
// jsdom env doesn't serve `?import` URLs, so stub the marks for a
// predictable path string (same pattern as provider-logo.test.tsx).
vi.mock("@/assets/preset-icons/claude.svg", () => ({
  default: "/mock/claude.svg",
}));
vi.mock("@/assets/preset-icons/codex.svg", () => ({
  default: "/mock/codex.svg",
}));
vi.mock("@/assets/preset-icons/opencode.svg", () => ({
  default: "/mock/opencode.svg",
}));

// Send → jump-to-latest: `ScrollToBottomOnSend` calls the scroller
// engine's imperative `scrollToEnd` (which snaps to the tail AND re-pins
// following-bottom). Stub the hook so we can assert the wiring without a
// live layout. `scrollToEnd` is a stable ref so the effect never refires
// on unrelated re-renders (only on a real signal change).
const { scrollToEndSpy } = vi.hoisted(() => ({ scrollToEndSpy: vi.fn() }));
vi.mock("@/components/ui/message-scroller", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/components/ui/message-scroller")>();
  return {
    ...actual,
    useMessageScroller: () => ({
      scrollToEnd: scrollToEndSpy,
      scrollToMessage: vi.fn(),
      scrollToStart: vi.fn(),
    }),
  };
});

afterEach(() => cleanup());

// The MessageScroller renders every row into the DOM (perf comes from
// `content-visibility:auto`, not row unmounting), so a plain render is
// enough to assert on grouping / dispatch / marker DOM — no virtualizer
// mock context is needed.
function renderList(
  messages: ChatViewItem[],
  extra?: {
    showThinking?: boolean;
    streaming?: boolean;
    sessionStartedAt?: number;
    stalled?: { silentForSecs: number } | null;
    interrupted?: boolean;
  },
) {
  return render(
    <MessageList messages={messages} {...extra} {...noopHandlers} />,
  );
}

function planReq(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-p",
    seq: 0,
    request_id: "req-p",
    turn_id: "turn-1",
    request_kind: "plan",
    payload: { plan: "# Refactor\n\n- Step one" },
    tool_use_id: null,
    resolution: { state: "pending" },
    ...overrides,
  };
}

function askReq(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-a",
    seq: 0,
    request_id: "req-a",
    turn_id: "turn-1",
    request_kind: "user-input",
    payload: {
      questions: [
        {
          header: "F",
          question: "Framework?",
          multiSelect: false,
          options: [{ label: "React", description: "" }],
        },
      ],
    },
    tool_use_id: null,
    resolution: { state: "pending" },
    ...overrides,
  };
}

function genericReq(
  overrides: Partial<PermissionRequestItem> = {},
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id: "req-g",
    seq: 0,
    request_id: "req-g",
    turn_id: "turn-1",
    request_kind: "mcp-tool-use", // hypothetical future kind
    payload: { tool_name: "SomeFutureTool" },
    tool_use_id: null,
    resolution: { state: "pending" },
    ...overrides,
  };
}

function readCall(seq: number, path: string): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Read",
    input: { file_path: path },
    status: "done",
    result_content: null,
    approval_request_id: null,
  };
}

function workflowRunItem(overrides: Partial<WorkflowRunItem> = {}): WorkflowRunItem {
  return {
    kind: "workflow_run",
    id: "wf-1",
    seq: 0,
    workflowId: "wf-1",
    status: "pending_approval",
    name: "Audit route auth",
    description: null,
    script: null,
    plannedPhases: [{ title: "Discover route files", detail: null }],
    phases: [{ title: "Discover route files", detail: null, agents: [] }],
    resultText: null,
    totalTokens: null,
    agentCount: null,
    startedAt: Date.now(),
    durationMs: null,
    approvalRequestId: "req-wf",
    ...overrides,
  };
}

const noopHandlers = {
  onRespondToRequest: vi.fn(),
  onAcceptPlan: vi.fn(),
  onRejectPlan: vi.fn(),
};

describe("MessageList dispatch", () => {
  it("routes request_kind=plan to PlanProposalBlock", () => {
    renderList([planReq()]);
    expect(screen.getByText("Plan proposed")).toBeInTheDocument();
    expect(screen.getByText("Accept & execute")).toBeInTheDocument();
  });

  it("reduces request_kind=user-input to a transcript marker; full panel lives with the composer", () => {
    renderList([askReq()]);
    expect(
      screen.getByText(/Input requested — answer above the composer/),
    ).toBeInTheDocument();
    expect(screen.queryByText("React")).toBeNull();
    expect(screen.queryByText("Submit")).toBeNull();
    expect(screen.queryByText(/Approval requested/)).toBeNull();
  });

  it("shows a submitting marker while a user-input request is responding", () => {
    renderList([
      askReq({
        resolution: {
          state: "responding",
          decision: { decision: "allow", updated_input: { answers: {} } },
        },
      }),
    ]);
    expect(screen.getByText(/Submitting answers/)).toBeInTheDocument();
  });

  it("echoes the user's answer as a reply once a user-input request resolves", () => {
    renderList([
      askReq({
        resolution: {
          state: "resolved",
          decision: {
            decision: "allow",
            updated_input: {
              questions: [],
              answers: { "Framework?": "React" },
            },
          },
        },
      }),
    ]);
    // The chosen option is now visible in the transcript...
    expect(screen.getByText("React")).toBeInTheDocument();
    // ...instead of the old opaque "Answered" marker.
    expect(screen.queryByText("Answered")).toBeNull();
  });

  it("labels each answer by its question header when several were asked", () => {
    renderList([
      askReq({
        payload: {
          questions: [
            { header: "Framework", question: "Framework?", options: [] },
            { header: "Styling", question: "Styling?", options: [] },
          ],
        },
        resolution: {
          state: "resolved",
          decision: {
            decision: "allow",
            updated_input: {
              answers: { "Framework?": "React", "Styling?": "Tailwind" },
            },
          },
        },
      }),
    ]);
    expect(screen.getByText("React")).toBeInTheDocument();
    expect(screen.getByText("Tailwind")).toBeInTheDocument();
    expect(screen.getByText("Framework")).toBeInTheDocument();
    expect(screen.getByText("Styling")).toBeInTheDocument();
  });

  it("falls back to the plain marker when a resolved user-input carries no answer", () => {
    renderList([
      askReq({
        resolution: { state: "resolved", decision: { decision: "cancel" } },
      }),
    ]);
    expect(screen.getByText("Answered")).toBeInTheDocument();
  });

  it("falls back to PermissionRequestBlock for unknown request_kind", () => {
    renderList([genericReq()]);
    expect(screen.getByText(/Approval requested/)).toBeInTheDocument();
  });

  describe("specialized request kinds are not swallowed by the tool_use_id merge", () => {
    it("plan PermissionRequestItem renders PlanProposalBlock even with an orphan ExitPlanMode ToolCallItem", () => {
      const tool: ToolCallItem = {
        kind: "tool_call",
        id: "orphan-tool",
        seq: 0,
        tool_use_id: "tu-plan-x",
        tool_name: "ExitPlanMode",
        input: { plan: "# Refactor" },
        status: "running",
        result_content: null,
        approval_request_id: null,
      };
      const req = planReq({ request_id: "tu-plan-x", tool_use_id: "tu-plan-x" });
      renderList([tool, req]);
      expect(screen.getByText("Plan proposed")).toBeInTheDocument();
      expect(screen.getByText("Accept & execute")).toBeInTheDocument();
      expect(screen.queryByText(/Approval requested/)).toBeNull();
    });

    it("user-input PermissionRequestItem renders the transcript marker even with an orphan AskUserQuestion ToolCallItem", () => {
      const tool: ToolCallItem = {
        kind: "tool_call",
        id: "orphan-tool",
        seq: 0,
        tool_use_id: "tu-ask-x",
        tool_name: "AskUserQuestion",
        input: { questions: [] },
        status: "running",
        result_content: null,
        approval_request_id: null,
      };
      const req = askReq({ request_id: "tu-ask-x", tool_use_id: "tu-ask-x" });
      renderList([tool, req]);
      expect(
        screen.getByText(/Input requested — answer above the composer/),
      ).toBeInTheDocument();
      expect(screen.queryByText(/Approval requested/)).toBeNull();
    });
  });
});

describe("MessageList activity blocks", () => {
  it("folds a run of ≥2 completed tool calls into one settled activity block", () => {
    const messages: ChatViewItem[] = [
      readCall(0, "/a"),
      readCall(1, "/b"),
      readCall(2, "/c"),
    ];
    renderList(messages);
    // Settled header: derived summary + step meta + Details toggle. Step
    // rows stay hidden until expand.
    expect(screen.getByText("Explored the codebase")).toBeInTheDocument();
    expect(screen.getByText("3 steps")).toBeInTheDocument();
    expect(screen.getByText("Details")).toBeInTheDocument();
    expect(screen.queryByText("/a")).toBeNull();

    // Expand → per-step rows appear; the toggle flips to "Hide".
    fireEvent.click(screen.getByText("Explored the codebase"));
    expect(screen.getByText("/a")).toBeInTheDocument();
    expect(screen.getByText("/c")).toBeInTheDocument();
    expect(screen.getByText("Hide")).toBeInTheDocument();
  });

  it("renders a lone completed tool call as a single card, not an activity block", () => {
    renderList([readCall(0, "/only")]);
    expect(screen.queryByText(/steps/)).toBeNull();
    expect(screen.queryByText("Details")).toBeNull();
    // Single card shows the mono summary via ToolCallStatus.
    expect(screen.getByText("Read")).toBeInTheDocument();
    expect(screen.getByText("/only")).toBeInTheDocument();
  });

  it("shows a WORKING header (live action + counter) for the streaming tail run", () => {
    const messages: ChatViewItem[] = [
      readCall(0, "/a"),
      {
        kind: "tool_call",
        id: "tc-run",
        seq: 1,
        tool_use_id: "tu-run",
        tool_name: "Bash",
        input: { command: "cargo test" },
        status: "running",
        result_content: null,
        approval_request_id: null,
      },
    ];
    renderList(messages, { streaming: true, showThinking: false });
    expect(screen.getByText("Working")).toBeInTheDocument();
    // Live action = the running step; counter = 1 done · 1 running.
    expect(screen.getByText("1 done · 1 running")).toBeInTheDocument();
    expect(screen.getByText("run cargo test")).toBeInTheDocument();
  });

  it("a non-tool row breaks the run into two independent activity blocks", () => {
    const messages: ChatViewItem[] = [
      readCall(0, "/x0"),
      readCall(1, "/x1"),
      {
        kind: "assistant_message",
        id: "am-1",
        seq: 2,
        turn_id: "t1",
        text: "between bursts",
        streaming: false,
      },
      readCall(3, "/y0"),
      readCall(4, "/y1"),
    ];
    renderList(messages);
    expect(screen.getAllByText("Explored the codebase")).toHaveLength(2);
    expect(screen.getByText("between bursts")).toBeInTheDocument();
  });

  it("never swallows a pending-approval tool call into an activity block", () => {
    const pending: ToolCallItem = {
      kind: "tool_call",
      id: "tc-guard",
      seq: 2,
      tool_use_id: "tu-guard",
      tool_name: "Bash",
      input: { command: "rm -rf build" },
      status: "running",
      result_content: null,
      approval_request_id: "req-1",
    };
    const approval: PermissionRequestItem = {
      kind: "permission_request",
      id: "req-1",
      seq: 3,
      request_id: "req-1",
      turn_id: "turn-1",
      request_kind: "command",
      payload: { tool_name: "Bash", tool_input: { command: "rm -rf build" } },
      tool_use_id: "tu-guard",
      resolution: { state: "pending" },
    };
    renderList([readCall(0, "/a"), readCall(1, "/b"), pending, approval]);
    // Two completed reads fold into an activity block; the gated Bash call
    // stays a standalone card with its approval footer visible.
    expect(screen.getByText("Explored the codebase")).toBeInTheDocument();
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();
  });
});

describe("MessageList provider avatar", () => {
  const assistantTurn: ChatViewItem[] = [
    {
      kind: "assistant_message",
      id: "am-1",
      seq: 0,
      turn_id: "t1",
      text: "hello from the agent",
      streaming: false,
    },
  ];

  it("threads the provider through to the assistant-turn avatar mark", () => {
    const { container } = render(
      <MessageList messages={assistantTurn} provider="codex" {...noopHandlers} />,
    );
    const img = container.querySelector(
      "img[data-provider]",
    ) as HTMLImageElement;
    expect(img).not.toBeNull();
    expect(img.getAttribute("data-provider")).toBe("codex");
    expect(img.getAttribute("src")).toContain("codex.svg");
  });

  it("falls back to the sparkle avatar when no provider is passed", () => {
    const { container } = render(
      <MessageList messages={assistantTurn} {...noopHandlers} />,
    );
    // No branded mark image; the ember sparkle (inline svg) shows instead.
    expect(container.querySelector("img[data-provider]")).toBeNull();
  });
});

describe("MessageList chrome", () => {
  it("renders a session-start divider (plain label without a timestamp)", () => {
    renderList([readCall(0, "/a")]);
    expect(screen.getByText("Session started")).toBeInTheDocument();
  });

  it("renders a dated session-start marker when sessionStartedAt is provided", () => {
    renderList([readCall(0, "/a")], {
      sessionStartedAt: new Date().getTime(),
    });
    expect(screen.queryByText("Session started")).toBeNull();
    expect(screen.getByText(/Today ·/)).toBeInTheDocument();
  });

  it("wires up a jump-to-latest control", () => {
    renderList([readCall(0, "/a")]);
    expect(screen.getByText("Jump to latest")).toBeInTheDocument();
  });

  it("renders the streaming marker as a row when showThinking is set", () => {
    renderList(
      [
        {
          kind: "user_message",
          id: "um-1",
          seq: 0,
          text: "do the thing",
        },
      ],
      { showThinking: true },
    );
    expect(screen.getByRole("status", { name: "Agent is working" })).toBeInTheDocument();
    expect(screen.getByText("Working…")).toBeInTheDocument();
  });

  it("does not render the streaming marker when showThinking is unset", () => {
    renderList([readCall(0, "/a")]);
    expect(screen.queryByRole("status", { name: "Agent is working" })).toBeNull();
  });
});

describe("MessageList workflow_run dispatch", () => {
  it("renders the approval card full-width and suppresses the generic permission_request row it owns", () => {
    const req: PermissionRequestItem = {
      kind: "permission_request",
      id: "req-wf",
      seq: 1,
      request_id: "req-wf",
      turn_id: "t1",
      request_kind: "workflow",
      payload: {},
      tool_use_id: null,
      resolution: { state: "pending" },
    };
    renderList([workflowRunItem(), req]);
    expect(screen.getByTestId("workflow-approval-card")).toBeInTheDocument();
    expect(screen.getByText("Run as a workflow?")).toBeInTheDocument();
    // The generic PermissionRequestBlock fallback ("Approval requested…")
    // must not also render for the request the workflow card owns.
    expect(screen.queryByText(/Approval requested/)).toBeNull();
  });

  it("still renders an unrelated permission_request normally alongside a workflow card", () => {
    const unrelated: PermissionRequestItem = {
      kind: "permission_request",
      id: "req-other",
      seq: 1,
      request_id: "req-other",
      turn_id: "t1",
      request_kind: "mcp-tool-use",
      payload: { tool_name: "SomeTool" },
      tool_use_id: null,
      resolution: { state: "pending" },
    };
    renderList([workflowRunItem(), unrelated]);
    expect(screen.getByTestId("workflow-approval-card")).toBeInTheDocument();
    expect(screen.getByText(/Approval requested/)).toBeInTheDocument();
  });

  it("routes a running workflow to the inline progress row", () => {
    renderList([workflowRunItem({ status: "running", approvalRequestId: null })]);
    expect(screen.getByTestId("workflow-run-card")).toBeInTheDocument();
    expect(screen.getByText("Workflow running")).toBeInTheDocument();
  });

  it("routes a completed workflow to the summary row", () => {
    renderList([
      workflowRunItem({
        status: "completed",
        approvalRequestId: null,
        agentCount: 3,
        durationMs: 5000,
      }),
    ]);
    expect(screen.getByText(/Workflow complete/)).toBeInTheDocument();
  });
});

// ── GUI-mode background browser chip ──
// (docs/features/browser.md "Background browser in GUI mode")

function makeWorkspace(overrides: Partial<WorkspaceSnapshot> = {}): WorkspaceSnapshot {
  return {
    workspace_id: "ws-1",
    title: "Test",
    workspace_type: "standard",
    cwd: "/path/to/project",
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
    project_root: null,
    pr_number: null,
    pr_state: null,
    pr_url: null,
    linked_issue: null,
    tabs: [],
    active_tab_id: "",
    active_surface_id: "",
    surfaces: [],
    ...overrides,
  };
}

function makeBackgroundSession(
  overrides: Partial<AgentBrowserSession> = {},
): AgentBrowserSession {
  return {
    session_id: "abs-1",
    workspace_id: "ws-1",
    cli_session_name: "ws-abc123",
    stream_url: "ws://localhost:9223",
    current_url: "https://example.com/dashboard",
    is_active: true,
    pane_id: null,
    browser_id: null,
    user_dismissed: false,
    ...overrides,
  };
}

function setAppStateForBrowserChip(
  workspaceOverrides: Partial<WorkspaceSnapshot> = {},
  sessions: AgentBrowserSession[] = [],
) {
  const ws = makeWorkspace(workspaceOverrides);
  useAppStore.setState({
    appState: {
      schema_version: 1,
      active_workspace_id: ws.workspace_id,
      workspaces: [ws],
      terminal_sessions: [],
      browser_sessions: [],
      agent_browser_sessions: sessions,
      notifications: [],
      detected_ports: [],
      pane_statuses: {},
      persistence: {
        schema_version: 1,
        stores_layout_metadata: true,
        stores_terminal_metadata: true,
        stores_live_process_state: false,
      },
      config: {} as AppStateSnapshot["config"],
    },
  });
}

describe("MessageList background browser chip", () => {
  afterEach(() => {
    useAppStore.setState({ appState: null });
    useFeatureFlags.setState({ enableAgentChat: false });
  });

  it("renders the inline chip with the LIVE badge and current URL when the workspace has a live background session (GUI mode on)", () => {
    useFeatureFlags.setState({ enableAgentChat: true });
    setAppStateForBrowserChip({}, [makeBackgroundSession()]);
    render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-1"
        {...noopHandlers}
      />,
    );
    expect(screen.getByText("Browser opened in background")).toBeInTheDocument();
    expect(screen.getByText("Live")).toBeInTheDocument();
    expect(
      screen.getByText(/https:\/\/example\.com\/dashboard/),
    ).toBeInTheDocument();
  });

  it("does not render the chip when the Agent Chat beta flag is off (flag-off byte-identical path)", () => {
    useFeatureFlags.setState({ enableAgentChat: false });
    setAppStateForBrowserChip({}, [makeBackgroundSession()]);
    render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-1"
        {...noopHandlers}
      />,
    );
    expect(screen.queryByText("Browser opened in background")).toBeNull();
  });

  it("does not render the chip for an OpenFlow workspace even with the flag on", () => {
    useFeatureFlags.setState({ enableAgentChat: true });
    setAppStateForBrowserChip({ workspace_type: "open_flow" }, [
      makeBackgroundSession(),
    ]);
    render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-1"
        {...noopHandlers}
      />,
    );
    expect(screen.queryByText("Browser opened in background")).toBeNull();
  });

  it("does not render the chip once the session is attached to a pane (promoted, no longer background)", () => {
    useFeatureFlags.setState({ enableAgentChat: true });
    setAppStateForBrowserChip({}, [
      makeBackgroundSession({ pane_id: "pane-1", browser_id: "browser-1" }),
    ]);
    render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-1"
        {...noopHandlers}
      />,
    );
    expect(screen.queryByText("Browser opened in background")).toBeNull();
  });

  it("does not render the chip without a workspaceId prop, even with a matching session in state", () => {
    useFeatureFlags.setState({ enableAgentChat: true });
    setAppStateForBrowserChip({}, [makeBackgroundSession()]);
    render(<MessageList messages={[readCall(0, "/a")]} {...noopHandlers} />);
    expect(screen.queryByText("Browser opened in background")).toBeNull();
  });

  it("does not render the chip once the session is inactive (browser closed — is_active false)", () => {
    // Backs the close-action wiring: control.rs flips `is_active` to false
    // on a successful `close`, and the chip must stop showing entirely
    // (LIVE badge included) instead of blinking forever.
    useFeatureFlags.setState({ enableAgentChat: true });
    setAppStateForBrowserChip({}, [makeBackgroundSession({ is_active: false })]);
    render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-1"
        {...noopHandlers}
      />,
    );
    expect(screen.queryByText("Browser opened in background")).toBeNull();
    expect(screen.queryByText("Live")).toBeNull();
  });
});

describe("MessageList dead-run detection (issue #154)", () => {
  const userTurn: ChatViewItem = {
    kind: "user_message",
    id: "um-1",
    seq: 0,
    text: "do the thing",
  };

  describe("stall notice", () => {
    it("renders the amber notice with floored minutes while stalled mid-turn", () => {
      renderList([userTurn], {
        streaming: true,
        stalled: { silentForSecs: 700 },
      });
      const notice = screen.getByTestId("run-stalled-notice");
      // 700s / 60 = 11.67 → floors to 11.
      expect(notice).toHaveTextContent(
        "No activity for 11m — the agent may have stopped.",
      );
    });

    it("clamps sub-minute silences up to 1m (never '0m')", () => {
      renderList([userTurn], {
        streaming: true,
        stalled: { silentForSecs: 30 },
      });
      expect(screen.getByTestId("run-stalled-notice")).toHaveTextContent(
        "No activity for 1m",
      );
    });

    it("does not render when the thread is not streaming", () => {
      // A stale `stalled` value on a settled thread (e.g. the terminal
      // event raced the sweep) must not show a mid-turn notice.
      renderList([userTurn], {
        streaming: false,
        stalled: { silentForSecs: 700 },
      });
      expect(screen.queryByTestId("run-stalled-notice")).toBeNull();
    });

    it("suppresses the StreamingMarker while the notice shows (mutual exclusion)", () => {
      renderList([userTurn], {
        showThinking: true,
        streaming: true,
        stalled: { silentForSecs: 700 },
      });
      expect(screen.getByTestId("run-stalled-notice")).toBeInTheDocument();
      // The shimmer marker would otherwise render for showThinking=true
      // (see the "streaming marker" cases above) — the amber notice
      // replaces it rather than stacking under it.
      expect(
        screen.queryByRole("status", { name: "Agent is working" }),
      ).toBeNull();
      expect(screen.queryByText("Working…")).toBeNull();
    });
  });

  describe("run-interrupted divider", () => {
    it("renders the divider with its label when interrupted and not streaming", () => {
      renderList([userTurn], { interrupted: true, streaming: false });
      const divider = screen.getByTestId("run-interrupted-divider");
      expect(divider).toHaveTextContent("Run interrupted");
    });

    it("does not render while streaming (a live turn owns the tail)", () => {
      renderList([userTurn], { interrupted: true, streaming: true });
      expect(screen.queryByTestId("run-interrupted-divider")).toBeNull();
    });

    it("does not render when the thread is not interrupted", () => {
      renderList([userTurn], { interrupted: false, streaming: false });
      expect(screen.queryByTestId("run-interrupted-divider")).toBeNull();
    });
  });
});

describe("MessageList send → jump-to-latest", () => {
  const userTurn: ChatViewItem = {
    kind: "user_message",
    id: "um-1",
    seq: 0,
    text: "first prompt",
  };
  const nextTurn: ChatViewItem = {
    kind: "user_message",
    id: "um-2",
    seq: 1,
    text: "second prompt",
  };

  afterEach(() => scrollToEndSpy.mockClear());

  it("does not force a scroll on the initial render", () => {
    render(
      <MessageList
        messages={[userTurn]}
        scrollToBottomSignal={0}
        {...noopHandlers}
      />,
    );
    expect(scrollToEndSpy).not.toHaveBeenCalled();
  });

  it("does not force a scroll when messages change but the signal does not", () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn]}
        scrollToBottomSignal={3}
        {...noopHandlers}
      />,
    );
    scrollToEndSpy.mockClear();
    // A streamed token (new/changed messages) must never yank a reader who
    // is scrolled up in history — only an explicit send does.
    rerender(
      <MessageList
        messages={[userTurn, nextTurn]}
        scrollToBottomSignal={3}
        {...noopHandlers}
      />,
    );
    expect(scrollToEndSpy).not.toHaveBeenCalled();
  });

  it("snaps to the tail (instant) and re-pins following when the send signal increments", () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn]}
        scrollToBottomSignal={0}
        {...noopHandlers}
      />,
    );
    scrollToEndSpy.mockClear();
    // Mirrors a send: the optimistic user bubble appends AND the signal
    // bumps in the same commit.
    rerender(
      <MessageList
        messages={[userTurn, nextTurn]}
        scrollToBottomSignal={1}
        {...noopHandlers}
      />,
    );
    expect(scrollToEndSpy).toHaveBeenCalledTimes(1);
    // `behavior: "auto"` = instant catch-up (no smooth-scroll lag on send).
    expect(scrollToEndSpy).toHaveBeenCalledWith({ behavior: "auto" });
  });
});
