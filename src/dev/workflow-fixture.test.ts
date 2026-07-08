import { describe, it, expect } from "vitest";

import { replayPayloads } from "@/lib/agent-chat/hydrate";
import { subagentFindingBadge, workflowRunItems } from "@/lib/agent-chat/workflows";
import type { WorkflowRunItem } from "@/lib/agent-chat/types";

import {
  workflowApprovalEnvelopes,
  workflowCompleteEnvelopes,
  workflowRunningEnvelopes,
} from "./mock-fixtures";

/**
 * Guards the three seeded "/workflow" demo threads (design fixture
 * `.design/workflow-orchestration.dc.html`, "Audit route auth") against
 * reducer drift: the same hydrate path the mock uses
 * (`agent_chat_list_messages` → JSON strings → `replayPayloads`) must
 * rebuild the pending-approval / running / completed `WorkflowRunItem`
 * shapes the design calls for.
 */
describe("workflow demo fixtures", () => {
  function replay(envelopes: unknown[]): ReturnType<typeof replayPayloads> {
    return replayPayloads(envelopes.map((e) => JSON.stringify(e)));
  }

  function soleWorkflow(state: ReturnType<typeof replayPayloads>): WorkflowRunItem {
    const items = workflowRunItems(state.messages);
    expect(items).toHaveLength(1);
    return items[0];
  }

  function phase(item: WorkflowRunItem, title: string) {
    const p = item.phases.find((ph) => ph.title === title);
    expect(p).toBeDefined();
    return p!;
  }

  it("approval thread: pending_approval with planned phases, script, and a linked open request", () => {
    const state = replay(workflowApprovalEnvelopes("t-approval"));
    const wf = soleWorkflow(state);

    expect(wf.status).toBe("pending_approval");
    expect(wf.name).toBe("Audit route auth");
    expect(wf.script).toContain("export const meta");
    expect(wf.plannedPhases.map((p) => p.title)).toEqual([
      "Discover route files",
      "Audit each file for missing auth",
      "Adversarially verify findings",
    ]);
    expect(wf.approvalRequestId).toBe("req-workflow-approval");

    const req = state.messages.find((m) => m.kind === "permission_request");
    expect(req).toBeDefined();
    if (req?.kind === "permission_request") {
      expect(req.resolution.state).toBe("pending");
      expect(req.tool_use_id).toBe(wf.workflowId);
    }
  });

  it("running thread: phase 1 done, phase 2 mixed states with a findings badge, phase 3 pending", () => {
    const state = replay(workflowRunningEnvelopes("t-running"));
    const wf = soleWorkflow(state);

    expect(wf.status).toBe("running");
    // Stays linked after resolution so transcript-slots keeps suppressing
    // the standalone resolved permission block (no stray "Allowed" row).
    expect(wf.approvalRequestId).toBe("req-workflow-running");

    const discover = phase(wf, "Discover route files");
    expect(discover.agents).toHaveLength(1);
    expect(discover.agents[0].status).toBe("completed");

    const audit = phase(wf, "Audit each file for missing auth");
    const byId = Object.fromEntries(audit.agents.map((a) => [a.id, a]));
    expect(byId["audit-auth"].status).toBe("completed");
    expect(byId["audit-billing"].status).toBe("completed");
    expect(byId["audit-webhooks"].status).toBe("completed");
    expect(byId["audit-orders"].status).toBe("running");
    expect(byId["audit-reports"].status).toBe("running");
    expect(byId["audit-admin"].status).toBe("pending");
    expect(byId["audit-search"].status).toBe("pending");

    expect(subagentFindingBadge(byId["audit-auth"])).toEqual({ label: "clean", tone: "green" });
    expect(subagentFindingBadge(byId["audit-billing"])).toEqual({
      label: "2 issues",
      tone: "red",
    });
    expect(subagentFindingBadge(byId["audit-webhooks"])).toEqual({
      label: "1 issue",
      tone: "red",
    });
    // billing.ts carries a short read/grep sub-transcript.
    expect(byId["audit-billing"].items.some((i) => i.kind === "tool_call")).toBe(true);

    const verify = phase(wf, "Adversarially verify findings");
    expect(verify.agents).toHaveLength(0);
  });

  it("complete thread: roll-up figures + a trailing assistant report", () => {
    const state = replay(workflowCompleteEnvelopes("t-complete"));
    const wf = soleWorkflow(state);

    expect(wf.status).toBe("completed");
    expect(wf.agentCount).toBe(44);
    expect(wf.totalTokens).toBe(2_900_000);
    expect(wf.durationMs).toBe(291_000);
    expect(wf.resultText).toContain("3 endpoints");

    const report = [...state.messages]
      .reverse()
      .find((m) => m.kind === "assistant_message");
    expect(report).toBeDefined();
    if (report?.kind === "assistant_message") {
      expect(report.text).toContain("3 endpoints");
      expect(report.text).toContain("routes/billing.ts");
      expect(report.text).toContain("routes/webhooks.ts");
    }
  });
});
