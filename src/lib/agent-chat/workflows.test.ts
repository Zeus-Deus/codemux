import { describe, it, expect } from "vitest";

import type { WorkflowSnapshot } from "@/tauri/events";

import {
  activeWorkflowRun,
  countRunningWorkflowAgents,
  latestWorkflowRun,
  mergeWorkflowSnapshot,
  mergeWorkflowStatus,
  newWorkflowRunItem,
  subagentFindingBadge,
  workflowPhaseStats,
  workflowPhaseStatus,
  workflowRunItems,
  workflowRunStats,
} from "./workflows";
import type { ChatViewItem, SubagentView, WorkflowPhaseView, WorkflowRunItem } from "./types";

function agent(overrides: Partial<SubagentView> = {}): SubagentView {
  return {
    id: `s-${Math.random()}`,
    status: "running",
    items: [],
    toneIndex: 0,
    ...overrides,
  };
}

function phase(overrides: Partial<WorkflowPhaseView> = {}): WorkflowPhaseView {
  return {
    title: "Explore",
    detail: null,
    agents: [],
    ...overrides,
  };
}

function run(overrides: Partial<WorkflowRunItem> = {}): WorkflowRunItem {
  return {
    kind: "workflow_run",
    id: "wf-item-1",
    seq: 0,
    workflowId: "wf1",
    status: "running",
    name: null,
    description: null,
    script: null,
    plannedPhases: [],
    phases: [],
    resultText: null,
    totalTokens: null,
    agentCount: null,
    startedAt: 0,
    durationMs: null,
    approvalRequestId: null,
    ...overrides,
  };
}

describe("newWorkflowRunItem", () => {
  it("builds planned + live phases from the snapshot", () => {
    const snap: WorkflowSnapshot = {
      workflow_id: "wf1",
      status: "running",
      name: "Bug Hunt",
      description: "Find bugs",
      script: "export const meta = {}",
      phases: [
        { title: "Explore", detail: "scan" },
        { title: "Fix", detail: null },
      ],
    };
    const item = newWorkflowRunItem("wf-item-1", 0, 1000, snap);
    expect(item).toMatchObject({
      kind: "workflow_run",
      workflowId: "wf1",
      status: "running",
      name: "Bug Hunt",
      description: "Find bugs",
      script: "export const meta = {}",
      startedAt: 1000,
      approvalRequestId: null,
    });
    expect(item.plannedPhases).toEqual([
      { title: "Explore", detail: "scan" },
      { title: "Fix", detail: null },
    ]);
    expect(item.phases).toEqual([
      { title: "Explore", detail: "scan", agents: [] },
      { title: "Fix", detail: null, agents: [] },
    ]);
  });

  it("falls back to 'running' for an unrecognized wire status", () => {
    const item = newWorkflowRunItem("id", 0, 0, {
      workflow_id: "wf1",
      status: "some-future-status",
    });
    expect(item.status).toBe("running");
  });
});

describe("mergeWorkflowStatus", () => {
  it("never regresses a terminal status back to running/pending_approval", () => {
    expect(mergeWorkflowStatus("completed", "running")).toBe("completed");
    expect(mergeWorkflowStatus("failed", "pending_approval")).toBe("failed");
  });

  it("advances from pending_approval to running to terminal", () => {
    expect(mergeWorkflowStatus("pending_approval", "running")).toBe("running");
    expect(mergeWorkflowStatus("running", "completed")).toBe("completed");
  });
});

describe("mergeWorkflowSnapshot", () => {
  it("merges non-null fields and keeps status monotonic", () => {
    const item = run({ status: "running", name: "A" });
    const next = mergeWorkflowSnapshot(item, {
      workflow_id: "wf1",
      status: "completed",
      result_text: "done",
      total_tokens: 42,
    });
    expect(next.status).toBe("completed");
    expect(next.resultText).toBe("done");
    expect(next.totalTokens).toBe(42);
    expect(next.name).toBe("A");
  });

  it("sets planned phases only once, preserving already-attributed agents", () => {
    const existingAgent = agent({ id: "a1" });
    const item = run({
      plannedPhases: [{ title: "Run", detail: null }],
      phases: [{ title: "Run", detail: null, agents: [existingAgent] }],
    });
    const next = mergeWorkflowSnapshot(item, {
      workflow_id: "wf1",
      status: "running",
      phases: [{ title: "Explore", detail: "scan" }],
    });
    // Planned phases were already set — a later phases payload is ignored.
    expect(next.plannedPhases).toEqual([{ title: "Run", detail: null }]);
    expect(next.phases[0].agents).toEqual([existingAgent]);
  });

  it("preserves already-attributed agents when phases are set for the first time", () => {
    const existingAgent = agent({ id: "a1" });
    const item = run({
      plannedPhases: [],
      phases: [{ title: "Explore", detail: null, agents: [existingAgent] }],
    });
    const next = mergeWorkflowSnapshot(item, {
      workflow_id: "wf1",
      status: "running",
      phases: [
        { title: "Explore", detail: "scan the code" },
        { title: "Fix", detail: null },
      ],
    });
    expect(next.plannedPhases.map((p) => p.title)).toEqual(["Explore", "Fix"]);
    const explore = next.phases.find((p) => p.title === "Explore")!;
    expect(explore.agents).toEqual([existingAgent]);
  });
});

describe("workflowPhaseStats", () => {
  it("rolls up counts and tokens", () => {
    const p = phase({
      agents: [
        agent({ status: "running" }),
        agent({ status: "completed", totalTokens: 100 }),
        agent({ status: "failed", totalTokens: 50 }),
      ],
    });
    const stats = workflowPhaseStats(p, 1000);
    expect(stats.total).toBe(3);
    expect(stats.running).toBe(1);
    expect(stats.done).toBe(1);
    expect(stats.failed).toBe(1);
    expect(stats.tokens).toBe(150);
  });

  it("computes elapsedMs as the span from earliest start to latest end", () => {
    const p = phase({
      agents: [
        agent({ startedAt: 1000, durationMs: 2000 }), // ends at 3000
        agent({ startedAt: 1500, durationMs: 500 }), // ends at 2000
      ],
    });
    const stats = workflowPhaseStats(p, 9999);
    expect(stats.elapsedMs).toBe(2000); // 3000 - 1000
  });

  it("returns zero elapsed for a phase with no timed agents", () => {
    expect(workflowPhaseStats(phase({ agents: [] })).elapsedMs).toBe(0);
  });
});

describe("workflowPhaseStatus", () => {
  it("is running when any agent is running or pending", () => {
    expect(
      workflowPhaseStatus(phase({ agents: [agent({ status: "running" })] }), "running"),
    ).toBe("running");
    expect(
      workflowPhaseStatus(phase({ agents: [agent({ status: "pending" })] }), "running"),
    ).toBe("running");
  });

  it("is done when every agent is terminal and none failed", () => {
    expect(
      workflowPhaseStatus(phase({ agents: [agent({ status: "completed" })] }), "running"),
    ).toBe("done");
  });

  it("is failed when every agent is terminal and one failed", () => {
    expect(
      workflowPhaseStatus(
        phase({ agents: [agent({ status: "completed" }), agent({ status: "failed" })] }),
        "running",
      ),
    ).toBe("failed");
  });

  it("is pending when the phase has no agents yet and the run is still going", () => {
    expect(workflowPhaseStatus(phase({ agents: [] }), "running")).toBe("pending");
  });

  it("resolves an empty planned phase alongside a terminal run", () => {
    expect(workflowPhaseStatus(phase({ agents: [] }), "completed")).toBe("done");
    expect(workflowPhaseStatus(phase({ agents: [] }), "failed")).toBe("failed");
  });
});

describe("workflowRunStats", () => {
  it("uses durationMs once terminal, else now - startedAt", () => {
    const terminal = run({ status: "completed", startedAt: 1000, durationMs: 5000 });
    expect(workflowRunStats(terminal, 999_999).elapsedMs).toBe(5000);

    const live = run({ status: "running", startedAt: 1000 });
    expect(workflowRunStats(live, 4000).elapsedMs).toBe(3000);
  });

  it("counts agents and sums tokens across phases when no rollup is provided", () => {
    const item = run({
      totalTokens: null,
      phases: [
        phase({ title: "Explore", agents: [agent({ totalTokens: 10 }), agent({ totalTokens: 20 })] }),
        phase({ title: "Fix", agents: [agent({ totalTokens: 5 })] }),
      ],
    });
    const stats = workflowRunStats(item, 0);
    expect(stats.agents).toBe(3);
    expect(stats.tokens).toBe(35);
  });

  it("prefers the provider-reported total_tokens rollup when present", () => {
    const item = run({ totalTokens: 999, phases: [phase({ agents: [agent({ totalTokens: 1 })] })] });
    expect(workflowRunStats(item, 0).tokens).toBe(999);
  });

  it("derives phasesDone / phasesTotal / currentPhaseIndex", () => {
    const item = run({
      status: "running",
      plannedPhases: [
        { title: "Explore", detail: null },
        { title: "Fix", detail: null },
      ],
      phases: [
        phase({ title: "Explore", agents: [agent({ status: "completed" })] }),
        phase({ title: "Fix", agents: [agent({ status: "running" })] }),
      ],
    });
    const stats = workflowRunStats(item, 0);
    expect(stats.phasesTotal).toBe(2);
    expect(stats.phasesDone).toBe(1);
    expect(stats.currentPhaseIndex).toBe(1);
  });
});

describe("subagentFindingBadge", () => {
  it("returns a red 'N issues' badge for a non-empty findings array", () => {
    const badge = subagentFindingBadge(
      agent({ resultText: JSON.stringify({ findings: [{}, {}] }) }),
    );
    expect(badge).toEqual({ label: "2 issues", tone: "red" });
  });

  it("returns a singular label for exactly one issue", () => {
    const badge = subagentFindingBadge(agent({ resultText: JSON.stringify({ bugs: [{}] }) }));
    expect(badge).toEqual({ label: "1 issue", tone: "red" });
  });

  it("returns a green 'clean' badge for an empty issues array", () => {
    const badge = subagentFindingBadge(agent({ resultText: JSON.stringify({ issues: [] }) }));
    expect(badge).toEqual({ label: "clean", tone: "green" });
  });

  it("returns null for non-JSON result text", () => {
    expect(subagentFindingBadge(agent({ resultText: "All done, no issues found." }))).toBeNull();
  });

  it("returns null when the result JSON carries none of the known keys", () => {
    expect(subagentFindingBadge(agent({ resultText: JSON.stringify({ summary: "ok" }) }))).toBeNull();
  });

  it("returns null when there is no result text yet", () => {
    expect(subagentFindingBadge(agent({ resultText: undefined }))).toBeNull();
  });
});

describe("whole-thread workflow lookups", () => {
  it("workflowRunItems filters to workflow_run kind only", () => {
    const messages: ChatViewItem[] = [
      run({ id: "wf-a" }),
      { kind: "user_message", id: "u1", seq: 1, text: "hi" },
      run({ id: "wf-b" }),
    ];
    expect(workflowRunItems(messages).map((w) => w.id)).toEqual(["wf-a", "wf-b"]);
  });

  it("activeWorkflowRun returns the latest running/pending_approval run", () => {
    const messages: ChatViewItem[] = [
      run({ id: "wf-a", status: "completed" }),
      run({ id: "wf-b", status: "running" }),
    ];
    expect(activeWorkflowRun(messages)?.id).toBe("wf-b");
  });

  it("activeWorkflowRun returns null when every run is terminal", () => {
    const messages: ChatViewItem[] = [run({ id: "wf-a", status: "completed" })];
    expect(activeWorkflowRun(messages)).toBeNull();
  });

  it("latestWorkflowRun returns the most recently started run regardless of status", () => {
    const messages: ChatViewItem[] = [
      run({ id: "wf-a", status: "completed" }),
      run({ id: "wf-b", status: "stopped" }),
    ];
    expect(latestWorkflowRun(messages)?.id).toBe("wf-b");
  });

  it("countRunningWorkflowAgents sums running agents across every run's phases", () => {
    const messages: ChatViewItem[] = [
      run({
        phases: [
          phase({ agents: [agent({ status: "running" }), agent({ status: "completed" })] }),
        ],
      }),
      run({ phases: [phase({ agents: [agent({ status: "running" })] })] }),
    ];
    expect(countRunningWorkflowAgents(messages)).toBe(2);
  });
});
