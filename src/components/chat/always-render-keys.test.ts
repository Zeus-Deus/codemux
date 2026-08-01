import { describe, expect, it } from "vitest";

import type {
  ChatViewItem,
  PermissionRequestItem,
  ToolCallItem,
  UserMessageItem,
  WorkflowRunItem,
} from "@/lib/agent-chat/types";

import { deriveAlwaysRenderKeys } from "./always-render-keys";
import type { TranscriptSlot } from "./transcript-slots";

let seq = 0;

function slot(item: ChatViewItem): TranscriptSlot {
  return {
    key: item.id,
    messageId: item.id,
    scrollAnchor: item.kind === "user_message",
    side: item.kind === "user_message" ? "user" : "assistant",
    showAvatar: false,
    turnStart: false,
    body: { kind: "item", item },
  };
}

function activitySlot(key: string, items: ToolCallItem[]): TranscriptSlot {
  return {
    key,
    messageId: items[0]?.id ?? key,
    scrollAnchor: false,
    side: "assistant",
    showAvatar: false,
    turnStart: false,
    body: { kind: "activity", items, working: false },
  };
}

function userMessage(
  id: string,
  queued?: UserMessageItem["queued"],
): UserMessageItem {
  return { kind: "user_message", id, seq: seq++, text: `turn ${id}`, queued };
}

function permissionRequest(
  id: string,
  resolution: PermissionRequestItem["resolution"],
  toolUseId: string | null = null,
): PermissionRequestItem {
  return {
    kind: "permission_request",
    id,
    seq: seq++,
    request_id: `req-${id}`,
    turn_id: null,
    request_kind: "tool_use",
    payload: null,
    tool_use_id: toolUseId,
    resolution,
  };
}

function toolCall(id: string, approvalRequestId: string | null): ToolCallItem {
  return {
    kind: "tool_call",
    id,
    seq: seq++,
    tool_use_id: `tu-${id}`,
    tool_name: "Bash",
    input: {},
    status: "running",
    result_content: null,
    approval_request_id: approvalRequestId,
  };
}

function workflowRun(
  id: string,
  approvalRequestId: string | null,
): WorkflowRunItem {
  return {
    kind: "workflow_run",
    id,
    seq: seq++,
    workflowId: `wf-${id}`,
    status: "running",
    name: "deploy",
    description: null,
    script: null,
    plannedPhases: [],
    phases: [],
    resultText: null,
    totalTokens: null,
    agentCount: null,
    startedAt: 0,
    durationMs: null,
    approvalRequestId,
  };
}

function indexRequests(
  requests: PermissionRequestItem[],
): Map<string, PermissionRequestItem> {
  return new Map(requests.map((r) => [r.request_id, r]));
}

describe("deriveAlwaysRenderKeys", () => {
  it("keeps queued user turns mounted", () => {
    const queued = userMessage("u2", { queuedId: "q1" });
    const slots = [slot(userMessage("u1")), slot(queued)];

    expect(deriveAlwaysRenderKeys(slots, new Map())).toEqual(["u2"]);
  });

  it("keeps pending and responding permission requests mounted", () => {
    const pending = permissionRequest("p1", { state: "pending" });
    const responding = permissionRequest("p2", {
      state: "responding",
      decision: { decision: "allow" },
    });
    const resolved = permissionRequest("p3", {
      state: "resolved",
      decision: { decision: "allow" },
    });
    const failed = permissionRequest("p4", {
      state: "failed",
      reason: "stale_provider_callback",
      message: "gone",
    });
    const slots = [pending, responding, resolved, failed].map(slot);

    expect(
      deriveAlwaysRenderKeys(
        slots,
        indexRequests([pending, responding, resolved, failed]),
      ),
    ).toEqual(["p1", "p2"]);
  });

  it("keeps tool calls and workflow runs with live approvals mounted", () => {
    const gating = permissionRequest("p1", { state: "pending" }, "tu-t1");
    const settled = permissionRequest("p2", {
      state: "resolved",
      decision: { decision: "deny", message: "no" },
    });
    const gatedTool = toolCall("t1", gating.request_id);
    const settledTool = toolCall("t2", settled.request_id);
    const ungatedTool = toolCall("t3", null);
    const gatedWorkflow = workflowRun("w1", gating.request_id);
    const ungatedWorkflow = workflowRun("w2", null);

    const slots = [
      slot(gatedTool),
      slot(settledTool),
      slot(ungatedTool),
      slot(gatedWorkflow),
      slot(ungatedWorkflow),
    ];

    expect(
      deriveAlwaysRenderKeys(slots, indexRequests([gating, settled])),
    ).toEqual(["t1", "w1"]);
  });

  it("ignores approval ids with no matching request", () => {
    const slots = [slot(toolCall("t1", "req-missing"))];

    expect(deriveAlwaysRenderKeys(slots, new Map())).toEqual([]);
  });

  it("never pins folded activity slots", () => {
    const slots = [
      activitySlot("run:t1", [toolCall("t1", null), toolCall("t2", null)]),
    ];

    expect(deriveAlwaysRenderKeys(slots, new Map())).toEqual([]);
  });

  it("returns nothing for an ordinary settled transcript", () => {
    const resolved = permissionRequest("p1", {
      state: "resolved",
      decision: { decision: "allow" },
    });
    const slots = [
      slot(userMessage("u1")),
      slot(resolved),
      slot(toolCall("t1", resolved.request_id)),
    ];

    expect(deriveAlwaysRenderKeys(slots, indexRequests([resolved]))).toEqual(
      [],
    );
  });

  it("returns keys in transcript order and stays small on long threads", () => {
    const slots: TranscriptSlot[] = [];
    for (let i = 0; i < 500; i += 1) {
      slots.push(slot(userMessage(`u${i}`)));
    }
    const queuedTail = userMessage("u-queued", { queuedId: "q1" });
    const pending = permissionRequest("p-live", { state: "pending" });
    slots.splice(10, 0, slot(pending));
    slots.push(slot(queuedTail));

    const keys = deriveAlwaysRenderKeys(slots, indexRequests([pending]));

    expect(keys).toEqual(["p-live", "u-queued"]);
  });
});
