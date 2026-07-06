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
 * completed, Verify running) from the persisted envelopes.
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
    expect(verify.status).toBe("running");
    // Its `npm run verify` call has no result → still running in the peek.
    expect(
      verify.items.some(
        (i) => i.kind === "tool_call" && i.status === "running",
      ),
    ).toBe(true);

    expect(countRunningSubagents(state.messages)).toBe(1);
  });
});
