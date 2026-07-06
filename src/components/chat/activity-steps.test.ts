import { describe, it, expect } from "vitest";

import type { ReasoningItem, ToolCallItem } from "@/lib/agent-chat/types";

import type { ActivityStep } from "./transcript-slots";
import {
  deriveActivityDurationMs,
  deriveActivitySummary,
  deriveWorkingCounter,
  formatActivityDuration,
  stepMeta,
  toStepView,
} from "./activity-steps";

function tool(name: string, overrides: Partial<ToolCallItem> = {}): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${Math.random()}`,
    seq: 0,
    tool_use_id: "tu",
    tool_name: name,
    input: {},
    status: "done",
    result_content: null,
    approval_request_id: null,
    ...overrides,
  };
}

function think(overrides: Partial<ReasoningItem> = {}): ReasoningItem {
  return {
    kind: "reasoning",
    id: `re-${Math.random()}`,
    seq: 0,
    turn_id: "t1",
    text: "hmm",
    streaming: false,
    ...overrides,
  };
}

describe("deriveActivitySummary", () => {
  it("read-heavy → Explored the codebase", () => {
    expect(
      deriveActivitySummary([tool("Read"), tool("Grep"), think()]),
    ).toBe("Explored the codebase");
  });
  it("edit-only → Edited files", () => {
    expect(deriveActivitySummary([tool("Edit"), tool("Write")])).toBe("Edited files");
  });
  it("command-only → Ran commands", () => {
    expect(deriveActivitySummary([tool("Bash"), tool("Bash")])).toBe("Ran commands");
  });
  it("reads + edits → Explored and edited files", () => {
    expect(deriveActivitySummary([tool("Read"), tool("Edit")])).toBe(
      "Explored and edited files",
    );
  });
});

describe("deriveWorkingCounter", () => {
  it("counts done / running / failed steps", () => {
    expect(
      deriveWorkingCounter([
        tool("Read"),
        tool("Bash", { status: "running" }),
        tool("Grep", { status: "error" }),
        think({ streaming: true }),
      ]),
    ).toBe("1 done · 2 running · 1 failed");
  });
});

describe("deriveActivityDurationMs / formatActivityDuration", () => {
  it("spans earliest start to latest completion", () => {
    const ms = deriveActivityDurationMs([
      tool("Read", { started_at: 1_000, completed_at: 5_000 }),
      tool("Bash", { started_at: 6_000, completed_at: 73_000 }),
    ]);
    expect(ms).toBe(72_000);
    expect(formatActivityDuration(72_000)).toBe("1m 12s");
    expect(formatActivityDuration(8_000)).toBe("8s");
  });

  it("omits a sub-second span (hydrated transcripts collapse to one instant)", () => {
    expect(
      deriveActivityDurationMs([
        tool("Read", { started_at: 1_000, completed_at: 1_000 }),
        tool("Read", { started_at: 1_000, completed_at: 1_000 }),
      ]),
    ).toBeNull();
  });

  it("returns null when nothing is timestamped", () => {
    expect(deriveActivityDurationMs([tool("Read"), tool("Read")])).toBeNull();
  });
});

describe("stepMeta / toStepView", () => {
  it("shows +added −removed for an edit", () => {
    const edit = tool("Edit", {
      input: { file_path: "/a", old_string: "one\ntwo", new_string: "one\ntwo\nthree" },
    });
    expect(stepMeta(edit)).toBe("+1 −0");
  });
  it("labels a running step and an errored step", () => {
    expect(stepMeta(tool("Bash", { status: "running" }))).toBe("running");
    expect(stepMeta(tool("Bash", { status: "error" }))).toBe("failed");
  });
  it("maps tool + reasoning to a short verb", () => {
    expect(toStepView(tool("Bash", { input: { command: "ls" } })).verb).toBe("run");
    expect(toStepView(think({ text: "first line\nsecond" })).verb).toBe("think");
    expect(toStepView(think({ text: "first line\nsecond" })).summary).toBe("first line");
  });
});

describe("ActivityStep type import is exercised", () => {
  it("compiles against the ActivityStep union", () => {
    const steps: ActivityStep[] = [tool("Read"), think()];
    expect(steps).toHaveLength(2);
  });
});
