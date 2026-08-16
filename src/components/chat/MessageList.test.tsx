/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, describe, it, expect, vi } from "vitest";
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";
import {
  getTitlebarContentUnder,
  getTitlebarTranscriptElements,
} from "@/lib/titlebar-content-under";
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

// jsdom has no layout, so use a transparent LegendList test double for the
// dispatch tests. Dedicated virtualization tests below use a bounded window.
// The double also models a *measured* list — row positions/sizes, viewport
// length, and the `isAtEnd` listener — which is the only way to exercise the
// new-turn scroll contract's geometry without a real browser viewport.
const {
  scrollToEndSpy,
  scrollToIndexSpy,
  scrollToOffsetSpy,
  lastListProps,
  listState,
  emitIsAtEnd,
  scrollDoubleTo,
  resetListDouble,
} = vi.hoisted(() => {
  const listeners = new Map<string, Set<(value: boolean) => void>>();
  const ROW_HEIGHT = 100;
  const state = {
    isAtEnd: false,
    // The signal the component actually subscribes to. LegendList derives it
    // from `onEndReachedThreshold` (default 0.5 — within half a viewport of
    // the end), which is why it can read true while the anchor glide is still
    // travelling. Modeled separately from `isAtEnd` so that gap is testable.
    isNearEnd: false,
    data: [] as unknown[],
    scroll: 0,
    scrollLength: 500,
    rowHeight: ROW_HEIGHT,
    positionAtIndex(index: number) {
      return index * this.rowHeight;
    },
    sizeAtIndex() {
      return this.rowHeight;
    },
    elementAtIndex: () => null,
    listen(type: string, cb: (value: boolean) => void) {
      let set = listeners.get(type);
      if (!set) {
        set = new Set();
        listeners.set(type, set);
      }
      set.add(cb);
      return () => set!.delete(cb);
    },
  };
  const notify = (type: string, value: boolean) => {
    for (const cb of [...(listeners.get(type) ?? [])]) cb(value);
  };
  return {
    scrollToEndSpy: vi.fn(),
    scrollToIndexSpy: vi.fn(),
    scrollToOffsetSpy: vi.fn(),
    lastListProps: { current: null as Record<string, any> | null },
    listState: state,
    /** Both edge signals move together — the shape most tests care about. */
    emitIsAtEnd: (value: boolean) => {
      state.isAtEnd = value;
      state.isNearEnd = value;
      notify("isNearEnd", value);
      notify("isAtEnd", value);
    },
    /** Move the viewport and let `isNearEnd` fall out of the real 0.5
     *  threshold against the modeled content height — the only way to
     *  exercise a mid-glide near-end transition, which is where the follow
     *  branch used to steal the viewport from the anchor glide. */
    scrollDoubleTo: (offset: number) => {
      state.scroll = offset;
      const contentHeight = state.data.length * state.rowHeight;
      const distanceToEnd = contentHeight - (offset + state.scrollLength);
      const near = distanceToEnd <= 0.5 * state.scrollLength;
      state.isAtEnd = distanceToEnd <= 0;
      if (near === state.isNearEnd) return;
      state.isNearEnd = near;
      notify("isNearEnd", near);
    },
    resetListDouble: () => {
      listeners.clear();
      state.isAtEnd = false;
      state.isNearEnd = false;
      state.scroll = 0;
      state.scrollLength = 500;
      state.rowHeight = ROW_HEIGHT;
      state.data = [];
    },
  };
});
vi.mock("@legendapp/list/react", async () => {
  const React = await import("react");
  return {
    LegendList: React.forwardRef(function LegendListMock(
      props: Record<string, any>,
      ref: React.ForwardedRef<any>,
    ) {
      lastListProps.current = props;
      listState.data = props.data;
      const nodeRef = React.useRef<HTMLDivElement>(null);
      React.useImperativeHandle(ref, () => ({
        getScrollableNode: () => nodeRef.current,
        getState: () => listState,
        scrollToEnd: (options: unknown) => {
          scrollToEndSpy(options);
          return Promise.resolve();
        },
        scrollToIndex: (params: unknown) => {
          scrollToIndexSpy(params);
          return Promise.resolve();
        },
        scrollToOffset: (params: unknown) => {
          scrollToOffsetSpy(params);
          return Promise.resolve();
        },
      }));
      return (
        <div ref={nodeRef} data-slot="transcript-list">
          {props.ListHeaderComponent}
          {props.data.map((item: unknown, index: number) => (
            <React.Fragment key={props.keyExtractor(item, index)}>
              {props.renderItem({ item, index })}
            </React.Fragment>
          ))}
          {props.ListFooterComponent}
        </div>
      );
    }),
  };
});

afterEach(() => {
  cleanup();
  resetListDouble();
  lastListProps.current = null;
  scrollToEndSpy.mockClear();
  scrollToIndexSpy.mockClear();
  scrollToOffsetSpy.mockClear();
});

// The transparent test double above renders all rows so component dispatch
// and marker behavior can be asserted without browser layout.
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

describe("MessageList titlebar scroll edge", () => {
  it("publishes only after the transcript scrolls beneath the overlay", () => {
    const { unmount } = render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-scroll-edge"
        {...noopHandlers}
      />,
    );
    const viewport = document.querySelector<HTMLElement>(
      '[data-slot="transcript-list"]',
    );
    expect(viewport).not.toBeNull();

    expect(getTitlebarContentUnder("ws-scroll-edge")).toBe(false);
    viewport!.scrollTop = 12;
    fireEvent.scroll(viewport!);
    expect(getTitlebarContentUnder("ws-scroll-edge")).toBe(true);

    viewport!.scrollTop = 0;
    fireEvent.scroll(viewport!);
    expect(getTitlebarContentUnder("ws-scroll-edge")).toBe(false);

    viewport!.scrollTop = 12;
    fireEvent.scroll(viewport!);
    unmount();
    expect(getTitlebarContentUnder("ws-scroll-edge")).toBe(false);
  });

  it("registers its live viewport so the titlebar never measures a detached node", () => {
    // `PaneContainer` renders only the active surface, so a tab switch
    // unmounts this list entirely. The titlebar keys its overlap
    // measurement on this registry — if the node stayed registered after
    // unmount (or was never registered) the raised treatment would latch
    // onto a detached element and stop firing after any navigation.
    const { unmount } = render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-registry"
        {...noopHandlers}
      />,
    );
    const viewport = document.querySelector<HTMLElement>(
      '[data-slot="transcript-list"]',
    );
    expect(getTitlebarTranscriptElements()).toContain(viewport);

    unmount();
    expect(getTitlebarTranscriptElements()).not.toContain(viewport);
  });

  it("registers even without a workspace id, since the overlap check is geometric", () => {
    const { unmount } = render(
      <MessageList messages={[readCall(0, "/a")]} {...noopHandlers} />,
    );
    const viewport = document.querySelector<HTMLElement>(
      '[data-slot="transcript-list"]',
    );
    expect(getTitlebarTranscriptElements()).toContain(viewport);
    unmount();
  });
});

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

  it("shows the durable expiry explanation for a stale user-input request", () => {
    renderList([
      askReq({
        resolution: {
          state: "failed",
          reason: "stale_provider_callback",
          message: "This question expired after restart.",
        },
      }),
    ]);
    expect(
      screen.getByText("This question expired after restart."),
    ).toBeInTheDocument();
    expect(screen.queryByText(/answer above the composer/)).toBeNull();
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
  it("settles a completed turn to one fold plus its final answer", () => {
    renderList([
      {
        kind: "user_message",
        id: "user-settled",
        seq: 0,
        text: "Inspect the renderer",
        created_at: 1_000,
      },
      {
        kind: "assistant_message",
        id: "commentary-settled",
        seq: 1,
        turn_id: "turn-settled",
        text: "I’ll inspect the implementation first.",
        streaming: false,
      },
      { ...readCall(2, "src/components/chat/MessageList.tsx"), turn_id: "turn-settled" },
      {
        kind: "assistant_message",
        id: "final-settled",
        seq: 3,
        turn_id: "turn-settled",
        text: "The final answer is now the primary surface.",
        streaming: false,
      },
      {
        kind: "turn_ended",
        id: "ended-settled",
        seq: 4,
        turn_id: "turn-settled",
        status: { kind: "success" },
        completed_at: 6_000,
      },
    ]);

    expect(screen.getByText("Worked for 5s")).toBeInTheDocument();
    expect(screen.getByText("The final answer is now the primary surface.")).toBeInTheDocument();
    expect(screen.queryByText("I’ll inspect the implementation first.")).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: "Worked for 5s" }));
    expect(screen.getByText("I’ll inspect the implementation first.")).toBeInTheDocument();
    expect(screen.getByText("src/components/chat/MessageList.tsx")).toBeInTheDocument();
  });

  it("shows only the newest completed tool call until earlier work is requested", () => {
    const messages: ChatViewItem[] = [
      readCall(0, "/a"),
      readCall(1, "/b"),
      readCall(2, "/c"),
    ];
    renderList(messages);
    expect(screen.getByText("/c")).toBeInTheDocument();
    expect(screen.getByText("+2 previous tool calls")).toBeInTheDocument();
    expect(screen.queryByText("/a")).toBeNull();

    fireEvent.click(screen.getByText("+2 previous tool calls"));
    expect(screen.getByText("/a")).toBeInTheDocument();
    expect(screen.getByText("/c")).toBeInTheDocument();
    expect(screen.getByText("Show fewer work entries")).toBeInTheDocument();
  });

  it("keeps a lone successful observational call silent", () => {
    renderList([readCall(0, "/only")]);
    expect(screen.queryByText(/previous tool call/)).toBeNull();
    expect(screen.queryByText("Details")).toBeNull();
    expect(screen.queryByText("read")).toBeNull();
    expect(screen.queryByText("/only")).toBeNull();
  });

  it("shows one live orb beside the newest streaming action", () => {
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
    const { container } = renderList(messages, {
      streaming: true,
      showThinking: false,
    });
    expect(container.querySelector('canvas[data-orb-state="working"]')).not.toBeNull();
    expect(screen.getByText("run")).toBeInTheDocument();
    expect(screen.getByText("cargo test")).toBeInTheDocument();
    expect(screen.getByText("+1 previous tool call")).toBeInTheDocument();
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
    expect(screen.getAllByText("+1 previous tool call")).toHaveLength(2);
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
    // Two completed reads stay in one compact log; the gated Bash call stays
    // a standalone actionable card with its approval footer visible.
    expect(screen.getByText("+1 previous tool call")).toBeInTheDocument();
    expect(screen.getByText("Allow")).toBeInTheDocument();
    expect(screen.getByText("Deny")).toBeInTheDocument();
  });
});

describe("MessageList provider identity", () => {
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

  it("keeps provider identity on the transcript without adding an avatar rail", () => {
    const { container } = render(
      <MessageList messages={assistantTurn} provider="codex" {...noopHandlers} />,
    );
    expect(container.querySelector('[data-provider="codex"]')).not.toBeNull();
    expect(container.querySelector("img[data-provider]")).toBeNull();
  });

  it("omits provider metadata when no provider is passed", () => {
    const { container } = render(
      <MessageList messages={assistantTurn} {...noopHandlers} />,
    );
    expect(container.querySelector("[data-provider]")).toBeNull();
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

  it("keeps the jump-to-latest control out of the way until the reader leaves", () => {
    // Presence of the control is covered by "MessageList jump-to-latest
    // pill" below; a bare mount must not show it, because the list reports
    // isAtEnd=false while `initialScrollAtEnd` is still settling.
    renderList([readCall(0, "/a")]);
    expect(screen.queryByText("Jump to latest")).toBeNull();
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

  it("renders the compact work-log browser event and current URL when the workspace has a live background session (GUI mode on)", () => {
    useFeatureFlags.setState({ enableAgentChat: true });
    setAppStateForBrowserChip({}, [makeBackgroundSession()]);
    render(
      <MessageList
        messages={[readCall(0, "/a")]}
        workspaceId="ws-1"
        {...noopHandlers}
      />,
    );
    expect(screen.getByText("Opened the browser")).toBeInTheDocument();
    expect(screen.getByText("work log")).toBeInTheDocument();
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

describe("MessageList new-turn scroll contract", () => {
  const userTurn: ChatViewItem = {
    kind: "user_message",
    id: "um-1",
    seq: 0,
    text: "first prompt",
  };
  /** The optimistic bubble a send appends, carrying its correlation token. */
  const sentTurn: ChatViewItem = {
    kind: "user_message",
    id: "um-2",
    seq: 1,
    text: "second prompt",
    clientNonce: "nonce-send",
  };
  /** A queued follow-up landing *after* the sent prompt — the reason the
   *  anchor is resolved by nonce rather than by "the last row". */
  const queuedAfter: ChatViewItem = {
    kind: "user_message",
    id: "um-3",
    seq: 2,
    text: "queued follow-up",
    clientNonce: "nonce-queued",
    queued: { queuedId: "q-1" },
  };
  const answer: ChatViewItem = {
    kind: "assistant_message",
    id: "am-1",
    seq: 3,
    turn_id: "turn-1",
    text: "answer",
    streaming: true,
  };

  /** Let the component's nested rAF chains run. */
  async function flushFrames(count = 4) {
    for (let i = 0; i < count; i++) {
      await act(
        () => new Promise<void>((r) => requestAnimationFrame(() => r())),
      );
    }
  }

  function viewport() {
    return document.querySelector<HTMLElement>('[data-slot="transcript-list"]')!;
  }

  const anchor = (nonce: number) => ({ clientNonce: "nonce-send", nonce });

  it("reserves no end space until a send names a row", () => {
    render(<MessageList messages={[userTurn]} {...noopHandlers} />);
    expect(lastListProps.current?.anchoredEndSpace).toBeUndefined();
    expect(scrollToIndexSpy).not.toHaveBeenCalled();
  });

  it("anchors the exact prompt the send named, not the last row", () => {
    render(
      <MessageList
        messages={[userTurn, sentTurn, queuedAfter]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    // Slot order is header-less: userTurn, sentTurn, queuedAfter.
    expect(lastListProps.current?.anchoredEndSpace).toMatchObject({
      anchorIndex: 1,
      anchorOffset: 16,
    });
  });

  /** Offset the anchored row (slot index 1, 100px rows) is parked at: its
   *  top, less the contract's 16px. This is what the glide is aiming for and
   *  therefore what a genuine `scrollend` must report. */
  const ANCHOR_LANDING = 84;

  /** The glide's settle handshake: the browser reports the smooth scroll
   *  finished. Production listens for `scrollend` (with a timer fallback) and
   *  only trusts an event whose landed offset matches the glide's target, so
   *  the double places the viewport there first; jsdom never emits the event,
   *  so tests drive it explicitly. */
  const settleGlide = (landedOffset = ANCHOR_LANDING) =>
    act(() => {
      listState.scroll = landedOffset;
      viewport().dispatchEvent(new Event("scrollend"));
    });

  it("glides the measured row to 16px below the transcript top", async () => {
    render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    // LegendList reports the row measured and the end space sized.
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();

    expect(scrollToIndexSpy).toHaveBeenCalledTimes(1);
    expect(scrollToIndexSpy).toHaveBeenCalledWith({
      index: 1,
      animated: true,
      viewPosition: 0,
      viewOffset: 16,
    });

    // The settle handshake re-pins the landed offset instantly, killing any
    // residual smooth-scroll momentum before stream-advance takes over.
    listState.scroll = 84;
    settleGlide();
    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 84,
      animated: false,
    });
  });

  it("positions instantly when the reader prefers reduced motion", async () => {
    const originalMatchMedia = window.matchMedia;
    window.matchMedia = ((query: string) => ({
      matches: query.includes("prefers-reduced-motion"),
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia;
    try {
      render(
        <MessageList
          messages={[userTurn, sentTurn]}
          sendAnchor={anchor(1)}
          {...noopHandlers}
        />,
      );
      act(() => {
        lastListProps.current?.anchoredEndSpace?.onReady?.({
          anchorIndex: 1,
          anchorKey: "um-2",
          size: 400,
        });
      });
      await flushFrames();

      expect(scrollToIndexSpy).toHaveBeenCalledWith({
        index: 1,
        animated: false,
        viewPosition: 0,
        viewOffset: 16,
      });
    } finally {
      window.matchMedia = originalMatchMedia;
    }
  });

  it("positions once per send, not on every re-measure", async () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    const fire = () =>
      act(() => {
        lastListProps.current?.anchoredEndSpace?.onReady?.({
          anchorIndex: 1,
          anchorKey: "um-2",
          size: 400,
        });
      });
    fire();
    await flushFrames();
    // Streaming content re-sizes the reserved space and re-fires onReady.
    rerender(
      <MessageList
        messages={[userTurn, sentTurn, answer]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    fire();
    await flushFrames();

    expect(scrollToIndexSpy).toHaveBeenCalledTimes(1);
  });

  it("keeps the prompt parked while the turn still fits the viewport", async () => {
    // 3 rows x 100px against a 500px viewport: nothing below the fold.
    render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    scrollToOffsetSpy.mockClear();
    await flushFrames();

    expect(scrollToOffsetSpy).not.toHaveBeenCalled();
  });

  it("advances only enough to reveal the tail once the turn overflows", async () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    settleGlide();
    scrollToOffsetSpy.mockClear();

    // The answer grows past the viewport: 8 rows x 100px = 800 content.
    listState.rowHeight = 100;
    listState.scroll = 84;
    const grown: ChatViewItem[] = [userTurn, sentTurn];
    for (let i = 0; i < 6; i++) {
      grown.push({ ...answer, id: `am-${i}`, seq: 3 + i });
    }
    rerender(
      <MessageList messages={grown} sendAnchor={anchor(1)} {...noopHandlers} />,
    );
    await flushFrames();

    // lastBottom 800 - usable (500 - 16) = 316 target, from scroll 84.
    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 316,
      animated: false,
    });
  });

  it("does not move for content that arrives without a send intent", async () => {
    const { rerender } = render(
      <MessageList messages={[userTurn]} {...noopHandlers} />,
    );
    // A real reader gesture drops live follow.
    await flushFrames(2);
    act(() => {
      fireEvent.wheel(viewport());
    });
    scrollToEndSpy.mockClear();
    scrollToOffsetSpy.mockClear();

    // Tokens keep streaming in; the reader must not be yanked.
    listState.rowHeight = 300;
    rerender(
      <MessageList messages={[userTurn, answer]} {...noopHandlers} />,
    );
    await flushFrames();

    expect(scrollToEndSpy).not.toHaveBeenCalled();
    expect(scrollToOffsetSpy).not.toHaveBeenCalled();
  });

  it("keeps following through a press on a row inside the transcript", async () => {
    // Accepting a plan, answering an approval, expanding a tool card, or
    // starting a text selection all land on a descendant of the scroller.
    // While an anchor is mounted the built-in end pin is off, so treating
    // those as navigation would freeze the viewport for the rest of the run.
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    settleGlide();
    scrollToOffsetSpy.mockClear();

    const row = document.querySelector<HTMLElement>("[data-message-id]")!;
    act(() => {
      fireEvent.pointerDown(row);
    });

    listState.scroll = 84;
    const grown: ChatViewItem[] = [userTurn, sentTurn];
    for (let i = 0; i < 6; i++) {
      grown.push({ ...answer, id: `am-${i}`, seq: 3 + i });
    }
    rerender(
      <MessageList messages={grown} sendAnchor={anchor(1)} {...noopHandlers} />,
    );
    await flushFrames();

    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 316,
      animated: false,
    });
  });

  it("releases follow for a press on the scroll container itself", async () => {
    // A scrollbar drag targets the scroller, not a row — that IS navigation.
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    scrollToOffsetSpy.mockClear();

    act(() => {
      fireEvent.pointerDown(viewport());
    });

    listState.scroll = 84;
    const grown: ChatViewItem[] = [userTurn, sentTurn];
    for (let i = 0; i < 6; i++) {
      grown.push({ ...answer, id: `am-${i}`, seq: 3 + i });
    }
    rerender(
      <MessageList messages={grown} sendAnchor={anchor(1)} {...noopHandlers} />,
    );
    await flushFrames();

    expect(scrollToOffsetSpy).not.toHaveBeenCalled();
  });

  it("restores the built-in end pin when the anchor is dropped", () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    expect(lastListProps.current?.maintainScrollAtEnd).toBe(false);

    // The pane cleared the anchor (a rollback, or a thread switch handing
    // this list fresh props). LegendList's own pin has to come back: it
    // covers item/footer layout growth (a late-loading image) whenever no
    // anchor is reserving space.
    rerender(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={null}
        {...noopHandlers}
      />,
    );
    expect(lastListProps.current?.anchoredEndSpace).toBeUndefined();
    expect(lastListProps.current?.maintainScrollAtEnd).toEqual({
      animated: false,
      on: {
        dataChange: true,
        footerLayout: true,
        itemLayout: true,
        layout: true,
      },
    });
  });

  it("does not steal the viewport back when the anchor clears mid-history-read", async () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();

    // The reader scrolls off into history mid-turn and gets their way back.
    act(() => {
      fireEvent.wheel(viewport());
      emitIsAtEnd(false);
    });
    await waitFor(() =>
      expect(
        screen.queryByRole("button", { name: /jump to latest/i }),
      ).not.toBeNull(),
    );
    scrollToEndSpy.mockClear();
    scrollToOffsetSpy.mockClear();

    // The pane clears the anchor (rollback). That is a lifecycle event, not
    // a navigation intent: re-claiming here would silently drop the
    // reader's follow state and take away the pill they are still using.
    listState.rowHeight = 300;
    rerender(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={null}
        {...noopHandlers}
      />,
    );
    await flushFrames();

    expect(
      screen.queryByRole("button", { name: /jump to latest/i }),
    ).not.toBeNull();
    expect(scrollToEndSpy).not.toHaveBeenCalled();
    expect(scrollToOffsetSpy).not.toHaveBeenCalled();
  });

  it("releases the reserved end space when a failed send rolls the anchor back", () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    expect(lastListProps.current?.anchoredEndSpace).toBeDefined();
    expect(lastListProps.current?.maintainScrollAtEnd).toBe(false);

    // Rollback: the optimistic bubble is removed and the anchor cleared.
    rerender(
      <MessageList messages={[userTurn]} sendAnchor={null} {...noopHandlers} />,
    );

    expect(lastListProps.current?.anchoredEndSpace).toBeUndefined();
    // The built-in end pin takes over again — no phantom reserved space.
    expect(lastListProps.current?.maintainScrollAtEnd).toMatchObject({
      animated: false,
    });
  });

  it("does not re-run the positioning scroll on a remount under a still-live anchor", async () => {
    // The anchor outlives the turn, so a MessageList remount (the subagent
    // drill-in/out swap) sees it again. The pane-owned positioned record is
    // what tells the fresh mount the prompt was already parked: the reserved
    // space comes back, the scroll does not.
    const positioned = { current: null as number | null };
    const { unmount } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        positionedNonceRef={positioned}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    expect(scrollToIndexSpy).toHaveBeenCalledTimes(1);
    expect(positioned.current).toBe(1);
    unmount();

    render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        positionedNonceRef={positioned}
        {...noopHandlers}
      />,
    );
    // The space is reserved again…
    expect(lastListProps.current?.anchoredEndSpace).toBeDefined();
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    // …but the prompt is not re-parked.
    expect(scrollToIndexSpy).toHaveBeenCalledTimes(1);
  });

  /** The anchored turn grown past the viewport: 8 rows x 100px = 800 of
   *  content against a 500px viewport, so the reveal target is
   *  800 - (500 - 16) = 316. */
  const grownTurn = (): ChatViewItem[] => {
    const grown: ChatViewItem[] = [userTurn, sentTurn];
    for (let i = 0; i < 6; i++) {
      grown.push({ ...answer, id: `am-${i}`, seq: 3 + i });
    }
    return grown;
  };

  it("still advances the stream after a remount under a still-live anchor", async () => {
    // Regression guard. "Positioned" is pane-owned but "settled" is
    // per-mount, so the fresh mount saw an already-positioned nonce with no
    // settle handshake in flight — and the advance decision, which waits for
    // the settle mark, then blocked every advance for the rest of the turn.
    // The reply streamed on with the viewport frozen.
    const positioned = { current: null as number | null };
    const fireReady = () =>
      act(() => {
        lastListProps.current?.anchoredEndSpace?.onReady?.({
          anchorIndex: 1,
          anchorKey: "um-2",
          size: 400,
        });
      });

    const first = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        positionedNonceRef={positioned}
        {...noopHandlers}
      />,
    );
    fireReady();
    await flushFrames();
    settleGlide();
    first.unmount();

    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        positionedNonceRef={positioned}
        {...noopHandlers}
      />,
    );
    fireReady();
    await flushFrames();
    // Still exactly one positioning scroll across both mounts.
    expect(scrollToIndexSpy).toHaveBeenCalledTimes(1);
    scrollToOffsetSpy.mockClear();

    // The answer keeps streaming into the remounted list.
    listState.scroll = 84;
    rerender(
      <MessageList
        messages={grownTurn()}
        sendAnchor={anchor(1)}
        positionedNonceRef={positioned}
        {...noopHandlers}
      />,
    );
    await flushFrames();

    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 316,
      animated: false,
    });
  });

  it("holds still for a token that lands mid-glide, and resumes once it settles", async () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    expect(scrollToIndexSpy).toHaveBeenCalledTimes(1);
    scrollToOffsetSpy.mockClear();

    // The first token arrives while the glide is still travelling. Advancing
    // here would cut the animation to wherever it happened to be.
    listState.scroll = 40;
    rerender(
      <MessageList
        messages={grownTurn()}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    await flushFrames();
    expect(scrollToOffsetSpy).not.toHaveBeenCalled();

    // The glide lands and the handshake re-pins it; from here the advance is
    // armed again.
    settleGlide();
    scrollToOffsetSpy.mockClear();
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(120);
    });
    await flushFrames();

    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 316,
      animated: false,
    });
  });

  it("does not let the near-end signal cut the glide with a jump to the tail", async () => {
    // `isNearEnd` is LegendList's half-a-viewport threshold, so it flips true
    // partway through the glide. Promoting that to `following-end` would hand
    // the next token the follow branch, whose instant `scrollToEnd` is a
    // harder cut than an early anchored advance.
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    scrollToEndSpy.mockClear();
    scrollToOffsetSpy.mockClear();

    // The reply grows to 800px of content against the 500px viewport while
    // the glide is still travelling.
    rerender(
      <MessageList
        messages={grownTurn()}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    await flushFrames();
    expect(scrollToOffsetSpy).not.toHaveBeenCalled();

    // Partway through the travel the viewport crosses the half-a-viewport
    // threshold and the list starts reporting near-end — truthfully.
    act(() => scrollDoubleTo(40));
    expect(listState.isNearEnd).toBe(false);
    act(() => scrollDoubleTo(60));
    expect(listState.isNearEnd).toBe(true);

    // The next token must still find the viewport under the glide's control.
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(120);
    });
    await flushFrames();
    expect(scrollToEndSpy).not.toHaveBeenCalled();
    expect(scrollToOffsetSpy).not.toHaveBeenCalled();

    // Proof the turn is still anchored rather than following: growth after
    // the settle advances by the measured delta, it does not snap to the end.
    settleGlide();
    scrollToOffsetSpy.mockClear();
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(120);
    });
    await flushFrames();
    expect(scrollToEndSpy).not.toHaveBeenCalled();
    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 316,
      animated: false,
    });
  });

  it("re-issues the instant placement when no scrollend arrives", async () => {
    // WebKitGTK never emits `scrollend`, so the fallback timer is the primary
    // settle path there — and the animation may still be mid-travel when it
    // fires. Pinning the current offset would park the prompt at an arbitrary
    // point; the contract's placement has to be re-issued instead.
    vi.useFakeTimers();
    try {
      render(
        <MessageList
          messages={[userTurn, sentTurn]}
          sendAnchor={anchor(1)}
          {...noopHandlers}
        />,
      );
      act(() => {
        lastListProps.current?.anchoredEndSpace?.onReady?.({
          anchorIndex: 1,
          anchorKey: "um-2",
          size: 400,
        });
      });
      await act(async () => {
        vi.advanceTimersByTime(100);
      });
      expect(scrollToIndexSpy).toHaveBeenCalledWith({
        index: 1,
        animated: true,
        viewPosition: 0,
        viewOffset: 16,
      });
      scrollToIndexSpy.mockClear();
      scrollToOffsetSpy.mockClear();

      // Mid-travel when the fallback fires.
      listState.scroll = 37;
      await act(async () => {
        vi.advanceTimersByTime(1000);
      });

      expect(scrollToOffsetSpy).not.toHaveBeenCalled();
      expect(scrollToIndexSpy).toHaveBeenCalledWith({
        index: 1,
        animated: false,
        viewPosition: 0,
        viewOffset: 16,
      });

      // And the nonce is settled, so the stream advances again.
      scrollToOffsetSpy.mockClear();
      listState.scroll = 84;
      listState.data = new Array(8);
      act(() => {
        lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(120);
      });
      await act(async () => {
        vi.advanceTimersByTime(100);
      });
      expect(scrollToOffsetSpy).toHaveBeenCalledWith({
        offset: 316,
        animated: false,
      });
    } finally {
      vi.useRealTimers();
    }
  });

  it("tears down its settle listener and fallback timer on unmount", async () => {
    vi.useFakeTimers();
    try {
      const { unmount } = render(
        <MessageList
          messages={[userTurn, sentTurn]}
          sendAnchor={anchor(1)}
          {...noopHandlers}
        />,
      );
      const node = viewport();
      const removeListener = vi.spyOn(node, "removeEventListener");
      act(() => {
        lastListProps.current?.anchoredEndSpace?.onReady?.({
          anchorIndex: 1,
          anchorKey: "um-2",
          size: 400,
        });
      });
      await act(async () => {
        vi.advanceTimersByTime(100);
      });
      expect(scrollToIndexSpy).toHaveBeenCalledTimes(1);
      scrollToIndexSpy.mockClear();
      scrollToOffsetSpy.mockClear();

      unmount();
      expect(removeListener).toHaveBeenCalledWith(
        "scrollend",
        expect.any(Function),
      );

      // Neither a late `scrollend` nor the fallback timer may drive a dead
      // component's viewport.
      act(() => {
        node.dispatchEvent(new Event("scrollend"));
      });
      await act(async () => {
        vi.advanceTimersByTime(2000);
      });
      expect(scrollToIndexSpy).not.toHaveBeenCalled();
      expect(scrollToOffsetSpy).not.toHaveBeenCalled();
    } finally {
      vi.useRealTimers();
    }
  });

  it("hands the built-in end pin back once the reserved space is spent", async () => {
    // The anchor no longer expires, so "pin off while an anchor is mounted"
    // would otherwise be permanent: once the reply outgrows the reserved
    // space LegendList clamps the spacer at 0 and stops calling back, and
    // late layout growth would never reveal the tail again.
    render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    expect(lastListProps.current?.maintainScrollAtEnd).toBe(false);

    // Settled, but the spacer is still holding room below the prompt: the pin
    // must stay off, or re-engaging it would yank the parked prompt.
    settleGlide();
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(120);
    });
    await flushFrames();
    expect(lastListProps.current?.maintainScrollAtEnd).toBe(false);

    // The reply has eaten the whole spacer — nothing left to yank.
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(0);
    });
    await flushFrames();
    expect(lastListProps.current?.maintainScrollAtEnd).toMatchObject({
      animated: false,
    });
    // The anchor itself is untouched: it still outlives the turn.
    expect(lastListProps.current?.anchoredEndSpace).toBeDefined();
  });

  it("re-runs the advance decision when the reserved space resizes with no data change", async () => {
    // A late-loading image grows a row without any slot rebuild. While an
    // anchor is mounted the built-in end pin is off, so the spacer's resize
    // callback is the only signal left that the tail may have sunk below
    // the fold.
    render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onReady?.({
        anchorIndex: 1,
        anchorKey: "um-2",
        size: 400,
      });
    });
    await flushFrames();
    settleGlide();
    scrollToOffsetSpy.mockClear();

    // Rows silently grew: 2 rows x 400px = 800 content against a 500px
    // viewport, scroll still at 84.
    listState.rowHeight = 400;
    listState.scroll = 84;
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(0);
    });
    await flushFrames();

    // lastBottom 800 - usable (500 - 16) = 316 target, from scroll 84.
    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 316,
      animated: false,
    });
  });

  it("restores a hairline scroll drift from a spacer resize while free-scrolling", async () => {
    render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        {...noopHandlers}
      />,
    );
    await flushFrames(2);
    // The reader has gestured away — the resize must be imperceptible.
    act(() => {
      fireEvent.wheel(viewport());
    });
    listState.scroll = 100;
    act(() => {
      lastListProps.current?.anchoredEndSpace?.onSizeChanged?.(120);
    });
    // The browser drifted the offset by a hair while the spacer resized.
    listState.scroll = 101;
    await flushFrames();

    expect(scrollToOffsetSpy).toHaveBeenCalledWith({
      offset: 100,
      animated: false,
    });
  });

  it("keeps the anchor mounted after the turn settles", () => {
    // Settling is invisible to this component now: the pane simply keeps
    // the same anchor props, so the reserved space stays and nothing scrolls.
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        streaming
        {...noopHandlers}
      />,
    );
    expect(lastListProps.current?.anchoredEndSpace).toBeDefined();
    rerender(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        streaming={false}
        {...noopHandlers}
      />,
    );
    expect(lastListProps.current?.anchoredEndSpace).toBeDefined();
    expect(lastListProps.current?.maintainScrollAtEnd).toBe(false);
    expect(scrollToEndSpy).not.toHaveBeenCalled();
  });

  it("drops the anchor when the pane switches threads", () => {
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchor(1)}
        threadKey="thread-a"
        {...noopHandlers}
      />,
    );
    expect(lastListProps.current?.anchoredEndSpace).toBeDefined();
    rerender(
      <MessageList
        messages={[userTurn]}
        sendAnchor={null}
        threadKey="thread-b"
        {...noopHandlers}
      />,
    );
    expect(lastListProps.current?.anchoredEndSpace).toBeUndefined();
  });
});

describe("MessageList jump-to-latest pill", () => {
  const userTurn: ChatViewItem = {
    kind: "user_message",
    id: "um-1",
    seq: 0,
    text: "first prompt",
  };
  const sentTurn: ChatViewItem = {
    kind: "user_message",
    id: "um-2",
    seq: 1,
    text: "second prompt",
    clientNonce: "nonce-send",
  };
  const streamedAnswer: ChatViewItem = {
    kind: "assistant_message",
    id: "am-1",
    seq: 2,
    turn_id: "turn-1",
    text: "answer",
    streaming: true,
  };

  async function flushFrames(count = 3) {
    for (let i = 0; i < count; i++) {
      await act(
        () => new Promise<void>((r) => requestAnimationFrame(() => r())),
      );
    }
  }
  const pill = () => screen.queryByRole("button", { name: /jump to latest/i });

  it("stays hidden through mount and layout settling", async () => {
    render(<MessageList messages={[userTurn]} {...noopHandlers} />);
    // The list reports isAtEnd=false while `initialScrollAtEnd` settles.
    act(() => emitIsAtEnd(false));
    await flushFrames();
    expect(pill()).toBeNull();
  });

  it("appears only after a real reader gesture leaves the tail", async () => {
    render(<MessageList messages={[userTurn]} {...noopHandlers} />);
    await flushFrames(2);
    act(() => {
      fireEvent.wheel(
        document.querySelector<HTMLElement>('[data-slot="transcript-list"]')!,
      );
      emitIsAtEnd(false);
    });
    await waitFor(() => expect(pill()).not.toBeNull());
  });

  it("is absent during programmatic anchoring after a send", async () => {
    const { rerender } = render(
      <MessageList messages={[userTurn]} {...noopHandlers} />,
    );
    await flushFrames(2);
    act(() => {
      fireEvent.wheel(
        document.querySelector<HTMLElement>('[data-slot="transcript-list"]')!,
      );
      emitIsAtEnd(false);
    });
    await waitFor(() => expect(pill()).not.toBeNull());

    // Sending from deep in history hides the pill in the same commit as the
    // optimistic bubble, and anchoring keeps it hidden while the viewport
    // moves under our control.
    rerender(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={{ clientNonce: "nonce-send", nonce: 1 }}
        {...noopHandlers}
      />,
    );
    expect(pill()).toBeNull();
    act(() => emitIsAtEnd(false));
    await flushFrames();
    expect(pill()).toBeNull();
  });

  it("appears when a gesture interrupts an anchored stream", async () => {
    // The list had already reported "not at end" while we were driving the
    // anchored turn, so that value is edge-triggered *away* — no further
    // event will fire when the reader takes over. The pill still has to
    // show, or they are stranded in history with no way back.
    render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={{ clientNonce: "nonce-send", nonce: 1 }}
        {...noopHandlers}
      />,
    );
    await flushFrames(2);
    act(() => emitIsAtEnd(false));
    expect(pill()).toBeNull();

    act(() => {
      fireEvent.wheel(
        document.querySelector<HTMLElement>('[data-slot="transcript-list"]')!,
      );
    });
    await waitFor(() => expect(pill()).not.toBeNull());
  });

  it("re-claims follow and hides itself when the reader scrolls back to the edge", async () => {
    const anchored = { clientNonce: "nonce-send", nonce: 1 };
    const { rerender } = render(
      <MessageList
        messages={[userTurn, sentTurn]}
        sendAnchor={anchored}
        {...noopHandlers}
      />,
    );
    await flushFrames(2);
    act(() => emitIsAtEnd(false));
    act(() => {
      fireEvent.wheel(
        document.querySelector<HTMLElement>('[data-slot="transcript-list"]')!,
      );
    });
    await waitFor(() => expect(pill()).not.toBeNull());

    // Back at the tail: follow resumes rather than staying released.
    act(() => emitIsAtEnd(true));
    expect(pill()).toBeNull();

    scrollToEndSpy.mockClear();
    listState.rowHeight = 300; // real content now overflows the viewport
    rerender(
      <MessageList
        messages={[userTurn, sentTurn, streamedAnswer]}
        sendAnchor={anchored}
        {...noopHandlers}
      />,
    );
    await flushFrames();
    // Proof the re-claim took: the next token drives the tail again instead
    // of leaving the reader behind.
    expect(scrollToEndSpy).toHaveBeenCalledWith({ animated: false });
  });

  it("glides back to the live edge when used", async () => {
    render(<MessageList messages={[userTurn]} {...noopHandlers} />);
    await flushFrames(2);
    act(() => {
      fireEvent.wheel(
        document.querySelector<HTMLElement>('[data-slot="transcript-list"]')!,
      );
      emitIsAtEnd(false);
    });
    await waitFor(() => expect(pill()).not.toBeNull());

    fireEvent.click(pill()!);
    expect(scrollToEndSpy).toHaveBeenCalledWith({ animated: true });
    expect(pill()).toBeNull();
  });
});

describe("MessageList viewport edge fade", () => {
  // A mask composites the element's whole rendering, and a scroll container
  // renders its own scrollbar — so a single full-width fade dissolved the
  // scrollbar's end stop along with the content, and a fully-scrolled
  // transcript still looked like it had somewhere left to go. The fade must
  // stop short of the bar's column and leave that strip opaque.
  it("holds the edge fade off the scrollbar's own column", () => {
    render(<MessageList messages={[readCall(0, "/a")]} {...noopHandlers} />);
    const style = lastListProps.current?.style as Record<string, string>;
    expect(style).toBeDefined();

    for (const key of ["maskImage", "WebkitMaskImage"] as const) {
      // Two layers: the vertical fade, then an opaque strip.
      expect(style[key]).toContain("linear-gradient(#000, #000)");
    }
    for (const key of ["maskSize", "WebkitMaskSize"] as const) {
      // The fade is inset by the measured bar width; the strip covers it.
      expect(style[key]).toBe(
        "calc(100% - var(--transcript-sbw, 0px)) 100%, var(--transcript-sbw, 0px) 100%",
      );
    }
    for (const key of ["maskPosition", "WebkitMaskPosition"] as const) {
      expect(style[key]).toBe("left top, right top");
    }
  });

  it("publishes the measured scrollbar width the mask reads", () => {
    render(<MessageList messages={[readCall(0, "/a")]} {...noopHandlers} />);
    const viewport = document.querySelector<HTMLElement>(
      '[data-slot="transcript-list"]',
    )!;
    // CSS cannot ask how wide a scrollbar is, so the mask depends on this
    // variable existing. jsdom reports zero-size boxes, so the value is
    // "0px" here — the assertion is that it is published at all, which is
    // what keeps the mask from falling back to spanning the full width.
    expect(viewport.style.getPropertyValue("--transcript-sbw")).toBe("0px");
  });
});
