/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Profiler } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { DEFAULT_WORKFLOW_LIMITS, type WorkflowRunSnapshot, type WorkflowTaskSnapshot } from "@/tauri/workflows";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import { useUIStore } from "@/stores/ui-store";
import { useWorkflowUIStore } from "@/stores/workflow-ui-store";

const mocks = vi.hoisted(() => ({ get: vi.fn(), cancel: vi.fn() }));
vi.mock("@/tauri/workflows", async (original) => ({
  ...await original<typeof import("@/tauri/workflows")>(),
  workflowGet: mocks.get,
  workflowCancel: mocks.cancel,
}));
vi.mock("@/components/chat/ChatMarkdown", () => ({
  ChatMarkdown: ({ children }: { children: string }) => <div data-testid="markdown">{children}</div>,
}));
import { ChatWorkflowCard } from "./chat-workflow-card";

const usage = { input_tokens: 0, output_tokens: 0, total_tokens: 12_000, reserved_tokens: 0, estimated_tokens: 0, tokens_unknown: false, cost_usd: null, cost_unknown: true };
function task(id: string, title: string, status: WorkflowTaskSnapshot["status"], result: unknown = null): WorkflowTaskSnapshot {
  return {
    retired: false, generation: 1, status, depth: 0, parent_task_id: null, current_attempt: null, result, error: null, waiting_for: [], messages: [],
    spec: { id, title, prompt: "", route_id: id === "work-2" ? "claude-haiku" : "claude-opus", access: "read_only", dependencies: [], scope: [], required: true },
    attempts: status === "queued" ? [] : [{ id: `${id}-a`, generation: 1, operation_id: "op", status: status === "succeeded" ? "succeeded" : "running", route_id: "claude-opus", started_at_ms: 1_000, finished_at_ms: status === "succeeded" ? 63_000 : null, cancel_requested: false, reserved_tokens: 0, external_ref: null, output: null, error: null, usage, artifacts: [] }],
  };
}
function run(overrides: Partial<WorkflowRunSnapshot> = {}): WorkflowRunSnapshot {
  return {
    id: "run-1", status: "running", cancel_requested: false, pause_requested: false, error: null, revision: 3, created_at_ms: 1_000, updated_at_ms: 2_000,
    spec: { workspace_id: "ws", title: "Review auth", goal: "Review auth", mode: "live", allow_writes: false, tasks: [], limits: DEFAULT_WORKFLOW_LIMITS, routes: [{ id: "claude-opus", provider: "claude", model: "opus" }, { id: "claude-haiku", provider: "claude", model: "haiku" }] },
    tasks: [task("plan", "Plan the work", "succeeded"), task("work-1", "Session handling", "succeeded"), task("work-2", "Test coverage", "running")],
    usage, resolved_limits: DEFAULT_WORKFLOW_LIMITS, script: { status: "running", phase: "Work", result: null, error: null },
    ...overrides,
  };
}
function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><ChatWorkflowCard runId="run-1" workspaceId="ws" threadId="thread-1" /></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  useWorkflowUIStore.setState({ selectedRuns: {} });
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
});

describe("ChatWorkflowCard", () => {
  it("shows live progress per phase and stops the run", async () => {
    mocks.get.mockResolvedValue(run());
    mocks.cancel.mockResolvedValue(run({ status: "cancelled", revision: 4 }));
    mount();
    expect(await screen.findByText("Review auth")).toBeInTheDocument();
    expect(screen.getByText("Workflow · 1 of 2 agents done")).toBeInTheDocument();
    expect(screen.getByTestId("chat-workflow-status")).toHaveAttribute("data-status", "running");
    expect(screen.getByText("Session handling")).toBeInTheDocument();
    expect(screen.getByText("Test coverage")).toBeInTheDocument();
    expect(screen.getByText("1m 02s")).toBeInTheDocument();
    expect(screen.getByText("1/2")).toBeInTheDocument();
    expect(screen.getByText("after the agents finish")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(mocks.cancel).toHaveBeenCalledWith("run-1"));
    expect(await screen.findByText("Workflow · Stopped")).toBeInTheDocument();
  });

  it("shows the report and hands it to the chat or the details pane", async () => {
    mocks.get.mockResolvedValue(run({
      status: "completed",
      tasks: [task("plan", "Plan the work", "succeeded"), task("work-1", "Session handling", "succeeded"), task("report", "Write the report", "succeeded", { summary: "Refresh tokens survive logout." })],
      script: { status: "completed", phase: "Report", result: { summary: "Refresh tokens survive logout." }, error: null },
    }));
    mount();
    expect(await screen.findByTestId("markdown")).toHaveTextContent("Refresh tokens survive logout.");
    expect(screen.getByText("Workflow · 1 agent · report ready")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();

    fireEvent.click(screen.getByRole("button", { name: /Continue in chat/ }));
    expect(useAgentChatStore.getState().threads["thread-1"]?.inputDraft).toBe('Here is the report from the workflow "Review auth":\n\nRefresh tokens survive logout.\n\n');

    fireEvent.click(screen.getByRole("button", { name: /Details/ }));
    expect(useWorkflowUIStore.getState().selectedRuns.ws).toBe("run-1");
    expect(useUIStore.getState().rightPanelTabs.ws).toBe("workflows");
  });

  it("surfaces a failed run's error", async () => {
    mocks.get.mockResolvedValue(run({ status: "failed", error: "Planning failed: no model", tasks: [task("plan", "Plan the work", "failed")], script: { status: "failed", phase: "Plan", result: null, error: "Planning failed: no model" } }));
    mount();
    expect(await screen.findByRole("alert")).toHaveTextContent("Planning failed: no model");
    expect(screen.getByText("Workflow · Stopped with an error")).toBeInTheDocument();
  });

  it("explains a run this device no longer has", async () => {
    mocks.get.mockRejectedValue(new Error("Workflow run not found."));
    mount();
    expect(await screen.findByText("This workflow is no longer available on this device.")).toBeInTheDocument();
  });

  describe("elapsed labels", () => {
    beforeEach(() => {
      vi.useFakeTimers();
      vi.setSystemTime(60_000);
    });

    function mountSnapshot(snapshot: WorkflowRunSnapshot) {
      mocks.get.mockResolvedValue(snapshot);
      const client = new QueryClient({ defaultOptions: { queries: { retry: false, staleTime: Infinity, gcTime: Infinity } } });
      const key = ["workflows", "ws", "run", "run-1"];
      client.setQueryData(key, snapshot);
      const onRender = vi.fn();
      const view = render(<QueryClientProvider client={client}>
        <Profiler id="workflow" onRender={onRender}>
          <ChatWorkflowCard runId="run-1" workspaceId="ws" threadId="thread-1" />
        </Profiler>
      </QueryClientProvider>);
      return { client, key, onRender, view };
    }

    it.each(["running", "paused", "stopping", "unknown"] as const)("advances %s labels without committing the card", async (status) => {
      const { onRender } = mountSnapshot(run({ status, tasks: [
        ...run().tasks,
        task("work-3", "Needs confirmation", "unknown"),
        task("work-4", "Waiting for input", "waiting"),
        task("work-5", "Failed worker", "failed"),
        task("work-6", "Queued worker", "queued"),
      ] }));
      const pill = screen.getByTestId("chat-workflow-status");
      expect(pill).toHaveTextContent("0m 59s");
      const commits = onRender.mock.calls.length;
      expect(commits).toBeGreaterThan(0);

      await act(() => vi.advanceTimersByTimeAsync(3000));

      expect(pill).toHaveTextContent("1m 02s");
      for (const title of ["Test coverage", "Needs confirmation", "Waiting for input"]) {
        expect(screen.getByText(title).closest("li")).toHaveTextContent("1m 02s");
      }
      expect(screen.getByText("Session handling").closest("li")).toHaveTextContent("1m 02s");
      expect(screen.getByText("Failed worker").closest("li")).toHaveTextContent("failed");
      expect(screen.getByText("Queued worker").closest("li")).toHaveTextContent("queued");
      expect(onRender).toHaveBeenCalledTimes(commits);
    });

    it("uses attempt history for durations even when current_attempt differs", async () => {
      const live = task("work-1", "Live history", "unknown");
      live.current_attempt = { ...live.attempts[0], finished_at_ms: 30_000, status: "succeeded" };
      live.attempts = [
        { ...live.attempts[0], id: "old-generation", generation: 0, started_at_ms: 0, finished_at_ms: 500 },
        { ...live.attempts[0], id: "first-attempt", finished_at_ms: 30_000, status: "failed" },
        { ...live.attempts[0], started_at_ms: 31_000 },
      ];
      const settled = task("work-2", "Settled history", "succeeded");
      settled.current_attempt = { ...settled.attempts[0], finished_at_ms: null, status: "running" };
      const replaced = task("work-3", "Replacement generation", "queued");
      replaced.generation = 2;
      replaced.attempts = live.attempts;
      mountSnapshot(run({ tasks: [live, settled, replaced] }));
      expect(screen.getByText("Live history").closest("li")).toHaveTextContent("0m 59s");

      await act(() => vi.advanceTimersByTimeAsync(5000));

      expect(screen.getByText("Live history").closest("li")).toHaveTextContent("1m 04s");
      expect(screen.getByText("Settled history").closest("li")).toHaveTextContent("1m 02s");
      expect(screen.getByText("Replacement generation").closest("li")).toHaveTextContent("queued");
    });

    it.each(["completed", "failed", "cancelled"] as const)("paints settled durations and stops ticking when the run becomes %s", async (status) => {
      const { client, key, onRender } = mountSnapshot(run());
      await act(() => vi.advanceTimersByTimeAsync(3000));
      expect(screen.getByText("Test coverage").closest("li")).toHaveTextContent("1m 02s");
      const settled = task("work-2", "Test coverage", "succeeded");
      settled.attempts[0].finished_at_ms = 62_000;
      const snapshot = run({ status, revision: 4, updated_at_ms: 66_000, tasks: [settled] });
      mocks.get.mockResolvedValue(snapshot);
      act(() => client.setQueryData(key, snapshot));
      await act(() => vi.advanceTimersByTimeAsync(0));
      const pill = screen.getByTestId("chat-workflow-status");
      expect(pill).toHaveAttribute("data-status", status);
      expect(pill).toHaveTextContent("1m 05s");
      expect(screen.getByText("Test coverage").closest("li")).toHaveTextContent("1m 01s");
      const commits = onRender.mock.calls.length;

      await act(() => vi.advanceTimersByTimeAsync(10_000));

      expect(pill).toHaveTextContent("1m 05s");
      expect(screen.getByText("Test coverage").closest("li")).toHaveTextContent("1m 01s");
      expect(onRender).toHaveBeenCalledTimes(commits);
    });
  });
});
