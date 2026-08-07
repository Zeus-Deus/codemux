import { describe, expect, it } from "vitest";

import { resolveOrbState } from "@/lib/orb-state";

import { subagentOrbActivity, turnOrbActivity } from "./orb-activity";
import type { ChatViewItem, SubagentView, ToolCallItem } from "./types";

let seq = 0;

function toolCall(overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  seq += 1;
  return {
    kind: "tool_call",
    id: `tc-${seq}`,
    seq,
    tool_use_id: `tu-${seq}`,
    tool_name: "Read",
    input: { file_path: "/f" },
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function view(overrides: Partial<SubagentView> = {}): SubagentView {
  return { id: "s1", status: "running", items: [], toneIndex: 0, ...overrides };
}

/** The pair is what surfaces actually render, so assert through both. */
const stateOfTurn = (messages: ChatViewItem[]) =>
  resolveOrbState(turnOrbActivity(messages));
const stateOfSubagent = (v: SubagentView) => resolveOrbState(subagentOrbActivity(v));

describe("turnOrbActivity", () => {
  it("describes the running tool", () => {
    expect(
      stateOfTurn([toolCall({ tool_name: "Grep", status: "running" })]),
    ).toBe("searching");
    expect(
      stateOfTurn([toolCall({ tool_name: "Write", status: "running" })]),
    ).toBe("composing");
  });

  it("reads a running shell's command", () => {
    expect(
      stateOfTurn([
        toolCall({
          tool_name: "Bash",
          input: { command: "git push origin HEAD" },
          status: "running",
        }),
      ]),
    ).toBe("connecting");
  });

  it("goes neutral once the last tool has finished", () => {
    // Between tool calls the agent is composing its reply; leaving the
    // finished tool's animation up would claim something that stopped.
    expect(
      stateOfTurn([toolCall({ tool_name: "Grep", status: "done" })]),
    ).toBe("working");
  });

  it("reads a running tool after an errored one as a retry", () => {
    expect(
      stateOfTurn([
        toolCall({ tool_name: "Bash", status: "error" }),
        toolCall({ tool_name: "Bash", status: "running" }),
      ]),
    ).toBe("solving");
  });

  it("does not call it a retry when the earlier call succeeded", () => {
    expect(
      stateOfTurn([
        toolCall({ tool_name: "Read", status: "done" }),
        toolCall({ tool_name: "Grep", status: "running" }),
      ]),
    ).toBe("searching");
  });

  it("ignores non-tool items between the tool calls", () => {
    const prose = {
      kind: "assistant_message",
      id: "m1",
      seq: 99,
    } as unknown as ChatViewItem;
    expect(
      stateOfTurn([toolCall({ tool_name: "Glob", status: "running" }), prose]),
    ).toBe("searching");
  });

  it("is neutral on an empty transcript", () => {
    expect(stateOfTurn([])).toBe("working");
  });
});

describe("subagentOrbActivity", () => {
  it("treats a pending subagent as queued", () => {
    expect(stateOfSubagent(view({ status: "pending" }))).toBe("breathing");
  });

  it("describes a running subagent's current tool", () => {
    expect(
      stateOfSubagent(
        view({ items: [toolCall({ tool_name: "Edit", status: "running" })] }),
      ),
    ).toBe("composing");
  });

  it("reads a retry after a failed tool call", () => {
    expect(
      stateOfSubagent(
        view({
          items: [
            toolCall({ tool_name: "Bash", status: "error" }),
            toolCall({ tool_name: "Bash", status: "running" }),
          ],
        }),
      ),
    ).toBe("solving");
  });

  it("is neutral for a settled subagent", () => {
    // A finished row shows a flat check rather than an orb, but the
    // resolver must not claim activity if it is ever asked.
    expect(
      stateOfSubagent(
        view({
          status: "completed",
          items: [toolCall({ tool_name: "Grep", status: "running" })],
        }),
      ),
    ).toBe("working");
  });
});
