/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";

import type { SubagentView, ToolCallItem, UserMessageItem } from "@/lib/agent-chat/types";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useProviderCapabilities } from "@/stores/provider-capabilities-store";
import type { FollowUpTaskRow } from "@/tauri/commands";
import type { ChatModelInfo, ProviderChatCapabilities } from "@/tauri/types";

import { AsyncQuestionThreadContext } from "./AsyncQuestionPanel";
import { ChatProviderContext } from "./chat-provider-context";
import { SuggestedTasksCard } from "./SuggestedTasksCard";
import { buildTranscriptSlots } from "./transcript-slots";
import { useFollowUpStore } from "./follow-up-store";

vi.mock("@/assets/preset-icons/claude.svg", () => ({ default: "/mock/claude.svg" }));
vi.mock("@/assets/preset-icons/codex.svg", () => ({ default: "/mock/codex.svg" }));

const commands = vi.hoisted(() => ({
  followUpList: vi.fn(),
  followUpProviders: vi.fn(),
  followUpStart: vi.fn(),
  followUpMergeAll: vi.fn(),
  agentChatSendTurn: vi.fn(),
  agentChatOpenSearchResult: vi.fn(),
  agentChatInterruptTurn: vi.fn(),
  // Opening a picker asks for fresh catalogs; keep them pending so the
  // seeded store below is what the card reads.
  listChatProviderCapabilities: vi.fn(() => new Promise(() => {})),
  agentChatProviderHealth: vi.fn(() => new Promise(() => {})),
}));
vi.mock("@/tauri/commands", () => commands);
vi.mock("@/tauri/events", () => ({ onFollowUpChanged: vi.fn().mockResolvedValue(() => {}) }));
vi.mock("@/lib/toast", () => ({ toast: { error: vi.fn() } }));

const TASKS = [
  { title: "Share the token cache", task: "Move the token cache to a shared store.", provider: "claude" },
  { title: "Add refresh retry tests", task: "Add table tests for refresh retries.", provider: "codex" },
];

function model(overrides: Partial<ChatModelInfo>): ChatModelInfo {
  return {
    id: "m",
    label: "M",
    description: null,
    effort_levels: [],
    default_effort: null,
    prompt_injected_effort_levels: [],
    context_window_options: [],
    supports_adaptive_thinking: false,
    supports_thinking_toggle: false,
    supports_fast_mode: false,
    supports_images: false,
    sub_provider: null,
    is_free: false,
    ...overrides,
  };
}

function caps(models: ChatModelInfo[]): ProviderChatCapabilities {
  return {
    models,
    effort_granularity: "per_session",
    effort_label_map: { low: "Low", medium: "Medium", high: "High", max: "Max" },
    permission_modes: [],
    default_permission_mode: null,
    permission_granularity: "per_session",
  };
}

const CLAUDE = caps([
  model({ id: "claude-opus-5-5", label: "Opus 5.5", effort_levels: ["low", "medium", "high", "max"], default_effort: "medium" }),
  model({ id: "claude-sonnet-5", label: "Sonnet 5", effort_levels: ["low", "medium", "high"], default_effort: "medium" }),
]);
const CODEX = caps([
  model({ id: "gpt-6", label: "GPT-6", effort_levels: ["low", "medium", "high"], default_effort: "low" }),
]);

function call(): ToolCallItem {
  return {
    kind: "tool_call",
    id: "tc-1",
    seq: 2,
    turn_id: "t1",
    tool_use_id: "call-1",
    tool_name: "mcp__codemux__mcp__codemux__suggest_tasks",
    input: { tasks: TASKS },
    status: "done",
    result_content: null,
    approval_request_id: null,
    started_at: 1,
  } as ToolCallItem;
}

function row(overrides: Partial<FollowUpTaskRow> = {}): FollowUpTaskRow {
  return {
    child_thread_id: "child-1",
    parent_thread_id: "parent",
    tool_call_id: "call-1",
    index: 1,
    workspace_id: "ws-child",
    parent_workspace_id: "ws-parent",
    repo_path: "/repo",
    worktree_path: "/wt",
    branch: "add-refresh-retry-tests",
    base_sha: "a".repeat(40),
    provider: "codex",
    title: "Add refresh retry tests",
    outcome: "running",
    additions: null,
    deletions: null,
    commits: null,
    uncommitted: null,
    merged: false,
    created_at_ms: 1,
    ...overrides,
  };
}

function renderCard(views: Array<SubagentView | null> = [null, null], thread: string | null = "parent") {
  return render(
    <AsyncQuestionThreadContext.Provider value={thread}>
      <ChatProviderContext.Provider value="claude">
        <SuggestedTasksCard call={call()} views={views} />
      </ChatProviderContext.Provider>
    </AsyncQuestionThreadContext.Provider>,
  );
}

beforeEach(() => {
  useFollowUpStore.setState({ rows: {}, providers: null });
  useProviderCapabilities.setState({ claude: CLAUDE, codex: CODEX });
  // The parent chat runs Opus 5.5 on High.
  useAgentChatStore.getState().ensureThread("parent");
  useAgentChatStore.setState((state) => ({
    threads: {
      ...state.threads,
      parent: { ...state.threads.parent, model: "claude-opus-5-5", effort: "high" },
    },
  }));
  for (const fn of Object.values(commands)) fn.mockReset();
  commands.followUpList.mockResolvedValue([]);
  commands.followUpProviders.mockResolvedValue([
    { id: "claude", label: "Claude", installed: true },
    { id: "codex", label: "Codex", installed: true },
  ]);
  commands.followUpStart.mockResolvedValue({ child_thread_id: "c", workspace_id: "w", branch: "b" });
  commands.agentChatSendTurn.mockResolvedValue({ turn_id: "t", queued_id: null });
});

afterEach(() => {
  cleanup();
});

describe("SuggestedTasksCard", () => {
  it("lists the tasks with a header, Start all, and per-task actions", async () => {
    renderCard();
    const card = screen.getByTestId("suggested-tasks-card");
    expect(within(card).getByText("2 suggested tasks")).toBeInTheDocument();
    expect(screen.getByTestId("suggested-tasks-start-all")).toBeInTheDocument();
    const rows = screen.getAllByTestId("suggested-task-row");
    expect(rows).toHaveLength(2);
    expect(within(rows[0]).getByText("Share the token cache")).toBeInTheDocument();
    expect(within(rows[0]).getByTestId("suggested-task-do-here")).toBeInTheDocument();
  });

  it("defaults to the parent chat's model and effort, or the suggested provider's defaults", async () => {
    renderCard();
    const rows = screen.getAllByTestId("suggested-task-row");
    // Same provider as the chat: the chat's own model and effort.
    expect(within(rows[0]).getByTestId("multi-provider-model-picker-trigger")).toHaveTextContent("Opus 5.5");
    expect(within(rows[0]).getByRole("button", { name: /^Reasoning: High/ })).toBeInTheDocument();
    expect(within(rows[0]).getByTestId("suggested-task-start")).toHaveAttribute(
      "title",
      "Start in its own worktree with Opus 5.5 · High",
    );
    // The agent named Codex: Codex's default model at its default effort.
    expect(within(rows[1]).getByTestId("multi-provider-model-picker-trigger")).toHaveTextContent("GPT-6");
    expect(within(rows[1]).getByRole("button", { name: /^Reasoning: Low/ })).toBeInTheDocument();
  });

  it("starts one task with its provider, model and effort", async () => {
    renderCard();
    const rows = screen.getAllByTestId("suggested-task-row");
    await act(async () => {
      fireEvent.click(within(rows[0]).getByTestId("suggested-task-start"));
    });
    expect(commands.followUpStart).toHaveBeenCalledWith({
      parentThreadId: "parent",
      toolCallId: "call-1",
      index: 0,
      provider: "claude",
      model: "claude-opus-5-5",
      effort: "high",
    });
    await act(async () => {
      fireEvent.click(within(rows[1]).getByTestId("suggested-task-start"));
    });
    expect(commands.followUpStart).toHaveBeenLastCalledWith({
      parentThreadId: "parent",
      toolCallId: "call-1",
      index: 1,
      provider: "codex",
      model: "gpt-6",
      effort: "low",
    });
  });

  it("changing effort is sent, and changing provider resets effort to the new model's default", async () => {
    const user = userEvent.setup();
    renderCard();
    const row = () => screen.getAllByTestId("suggested-task-row")[0];
    // Pick Medium in the effort picker: GPT-6 accepts it too, so only the
    // reset (not the "model doesn't accept it" fallback) can turn it to Low.
    await user.click(within(row()).getByRole("button", { name: /^Reasoning: High/ }));
    await user.click(await screen.findByRole("option", { name: /Medium/ }));
    expect(within(row()).getByRole("button", { name: /^Reasoning: Medium/ })).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(within(row()).getByTestId("suggested-task-start"));
    });
    expect(commands.followUpStart).toHaveBeenLastCalledWith(
      expect.objectContaining({ index: 0, provider: "claude", model: "claude-opus-5-5", effort: "medium" }),
    );
    commands.followUpStart.mockClear();
    // Switch the row to Codex: GPT-6 at its default, Low.
    await user.click(within(row()).getByTestId("multi-provider-model-picker-trigger"));
    await user.click(await screen.findByTestId("provider-rail-codex"));
    const gpt = (await screen.findAllByTestId("multi-provider-model-row")).find((r) =>
      r.textContent?.includes("GPT-6"),
    )!;
    await user.click(gpt);
    await waitFor(() =>
      expect(within(row()).getByTestId("multi-provider-model-picker-trigger")).toHaveTextContent("GPT-6"),
    );
    expect(within(row()).getByRole("button", { name: /^Reasoning: Low/ })).toBeInTheDocument();
    await act(async () => {
      fireEvent.click(within(row()).getByTestId("suggested-task-start"));
    });
    expect(commands.followUpStart).toHaveBeenCalledWith(
      expect.objectContaining({ index: 0, provider: "codex", model: "gpt-6", effort: "low" }),
    );
  });

  it("Do it here sends a normal user turn to this chat", async () => {
    renderCard();
    await act(async () => {
      fireEvent.click(screen.getAllByTestId("suggested-task-do-here")[0]);
    });
    expect(commands.agentChatSendTurn).toHaveBeenCalledWith(
      "claude",
      expect.objectContaining({
        thread_id: "parent",
        display_text: "Do the suggested task here: Share the token cache",
      }),
    );
    expect(commands.agentChatSendTurn.mock.calls[0][1].text).toContain(TASKS[0].task);
  });

  it("shows a started task as a live status row with Open, and offers Merge all when done", async () => {
    commands.followUpList.mockResolvedValue([row()]);
    const live = {
      id: "delegate:child-1",
      parentItemId: "call-1#1",
      agentType: "codex",
      status: "running",
      activity: "Working in its workspace",
      model: "gpt-6",
      effort: "high",
    } as SubagentView;
    const { rerender } = renderCard([null, live]);
    await waitFor(() => expect(screen.getByTestId("suggested-task-status")).toBeInTheDocument());
    const rows = screen.getAllByTestId("suggested-task-row");
    expect(rows[1]).toHaveAttribute("data-state", "working");
    expect(within(rows[1]).getByText("Working")).toBeInTheDocument();
    expect(within(rows[1]).getByTestId("suggested-task-open")).toBeInTheDocument();
    // Which model ran it.
    expect(within(rows[1]).getByTestId("suggested-task-model")).toHaveTextContent("GPT-6 · High");
    expect(within(rows[1]).queryByTestId("suggested-task-start")).toBeNull();
    // Only one task left to start: no Start all.
    expect(screen.queryByTestId("suggested-tasks-start-all")).toBeNull();
    expect(screen.queryByTestId("suggested-tasks-merge-all")).toBeNull();

    act(() => {
      useFollowUpStore.setState({
        rows: { parent: [row({ outcome: "completed", additions: 40, deletions: 2 })] },
      });
    });
    rerender(
      <AsyncQuestionThreadContext.Provider value="parent">
        <ChatProviderContext.Provider value="claude">
          <SuggestedTasksCard call={call()} views={[null, { ...live, status: "completed" }]} />
        </ChatProviderContext.Provider>
      </AsyncQuestionThreadContext.Provider>,
    );
    expect(screen.getByText(/add-refresh-retry-tests · \+40 −2/)).toBeInTheDocument();
    commands.followUpMergeAll.mockResolvedValue({
      target_branch: "main",
      merged: [{ child_thread_id: "child-1", title: "Add refresh retry tests", branch: "b" }],
      skipped: [],
      conflict: null,
    });
    await act(async () => {
      fireEvent.click(screen.getByTestId("suggested-tasks-merge-all"));
    });
    expect(commands.followUpMergeAll).toHaveBeenCalledWith("parent");
    expect(screen.getByTestId("suggested-tasks-merge-result")).toHaveTextContent(
      "Merged 1 branch into main.",
    );
  });

  it("is read-only without a writable thread", () => {
    renderCard([null, null], null);
    expect(screen.queryByTestId("suggested-task-start")).toBeNull();
    expect(screen.queryByTestId("suggested-tasks-start-all")).toBeNull();
    expect(screen.getAllByTestId("suggested-task-row")).toHaveLength(2);
  });
});

describe("transcript slots", () => {
  it("renders a suggest_tasks call as one card that never folds, with started rows claimed", () => {
    const user = {
      kind: "user_message",
      id: "u1",
      seq: 1,
      text: "Fix it",
      created_at: 1,
    } as UserMessageItem;
    const started = {
      id: "delegate:child-1",
      parentItemId: "call-1#1",
      agentType: "codex",
      status: "running",
    } as SubagentView;
    const slots = buildTranscriptSlots([
      user,
      call(),
      { kind: "subagent_run", id: "run-1", seq: 3, subagents: [started] } as never,
      { kind: "turn_ended", id: "e1", seq: 4, turn_id: "t1", status: { kind: "success" }, completed_at: 9 } as never,
    ]);
    const bodies = slots.map((slot) => slot.body.kind);
    expect(bodies).toContain("suggested_tasks");
    expect(bodies).not.toContain("delegation");
    const card = slots.find((slot) => slot.body.kind === "suggested_tasks")!;
    expect(card.key).toBe("suggested:tc-1");
    if (card.body.kind !== "suggested_tasks") throw new Error("unreachable");
    expect(card.body.views).toEqual([null, started]);
  });
});
