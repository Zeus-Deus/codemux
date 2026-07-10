import { describe, it, expect } from "vitest";

import { replayPayloads } from "@/lib/agent-chat/hydrate";
import {
  countRunningSubagents,
  findSubagentView,
} from "@/lib/agent-chat/subagents";
import type { SubagentRunItem } from "@/lib/agent-chat/types";

import { subagentTurnEnvelopes } from "./mock-fixtures";

/**
 * Guards the seeded dev fixture against reducer drift: the same
 * hydrate path the mock uses (`agent_chat_list_messages` → JSON strings
 * → `replayPayloads`) must rebuild the design-fixture card (Implement
 * completed, Verify interrupted).
 *
 * The transcript ends with Verify still emitting `running` snapshots and
 * no terminal signal, so the hydrate reconciliation (issue #153) settles
 * it to the view-only `interrupted` state — a persisted transcript never
 * resurrects a perpetual spinner. The LIVE running bar stays demoable via
 * the mock's `streamSubagents()` command (real events, no hydrate
 * reconciliation).
 */
describe("subagentTurnEnvelopes seed", () => {
  it("replays into the two-subagent design card", () => {
    const payloads = subagentTurnEnvelopes("t-mock", "turn-mock").map((e) =>
      JSON.stringify(e),
    );
    const state = replayPayloads(payloads);

    const card = state.messages.find(
      (m): m is SubagentRunItem => m.kind === "subagent_run",
    );
    expect(card).toBeDefined();
    expect(card!.subagents.map((s) => s.id)).toEqual(["impl", "verify"]);

    const impl = findSubagentView(state.messages, "impl")!;
    expect(impl.status).toBe("completed");
    expect(impl.resultText).toContain("6 files changed");
    expect(impl.toolUseCount).toBe(28);
    expect(impl.durationMs).toBe(161000);
    // Child sub-transcript populated (tool calls routed in, not leaked
    // into the parent flow).
    expect(impl.items.some((i) => i.kind === "tool_call")).toBe(true);

    const verify = findSubagentView(state.messages, "verify")!;
    // Ends mid-run with no terminal snapshot → force-settled on hydrate.
    expect(verify.status).toBe("interrupted");
    expect(verify.statusAssumed).toBe(true);
    // The subagent VIEW is interrupted, but its child tool call was never
    // given a result, so the sub-transcript row itself stays "running"
    // (the settlement is on the subagent, not its inner steps).
    expect(
      verify.items.some(
        (i) => i.kind === "tool_call" && i.status === "running",
      ),
    ).toBe(true);

    // No spinner survives hydrate — the docked bar's count is 0.
    expect(countRunningSubagents(state.messages)).toBe(0);
  });
});
