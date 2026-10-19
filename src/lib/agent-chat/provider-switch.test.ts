import { describe, expect, it } from "vitest";

import { replayPayloads } from "./hydrate";
import {
  endpointLabel,
  handoffLabel,
  modelLabel,
  switchHint,
  switchRefusal,
} from "./provider-switch";

describe("provider switch", () => {
  it("replays a persisted switch into a divider item after the turn it follows", () => {
    const state = replayPayloads([
      JSON.stringify({ type: "user_message", thread_id: "t1", text: "Fix the parser" }),
      JSON.stringify({
        type: "provider_switched",
        thread_id: "t1",
        from_provider: "claude",
        from_model: "claude-opus-4-8",
        to_provider: "codex",
        to_model: "gpt-5.5",
        handoff_kind: "direct",
        truncated: true,
      }),
    ]);
    const last = state.messages[state.messages.length - 1];
    expect(last).toMatchObject({
      kind: "provider_switch",
      fromProvider: "claude",
      fromModel: "claude-opus-4-8",
      toProvider: "codex",
      toModel: "gpt-5.5",
      handoffKind: "direct",
      truncated: true,
    });
  });

  it("labels what was carried and who it moved between", () => {
    expect(handoffLabel("summary", false)).toBe("summary carried over");
    expect(handoffLabel("direct", false)).toBe("context carried over");
    expect(handoffLabel("direct", true)).toBe("recent context carried over");
    expect(handoffLabel("none", false)).toBe("nothing to carry yet");
    expect(handoffLabel("failed", false)).toBe("context couldn't be carried over");
    expect(endpointLabel("codex", "GPT-5.5")).toBe("Codex · GPT-5.5");
    expect(endpointLabel("opencode", null)).toBe("OpenCode");
    expect(modelLabel("claude-opus-4-8", null)).toBe("Claude Opus 4.8");
    expect(modelLabel("default", null)).toBeNull();
    expect(modelLabel("gpt-5.5", "GPT-5.5 Codex")).toBe("GPT-5.5 Codex");
    expect(switchHint("codex")).toBe(
      "Continues this chat in Codex. The conversation so far carries over.",
    );
  });

  it("tells a refusal that changed nothing from a part-way failure", () => {
    expect(
      switchRefusal(new Error("provider_switch_refused: Wait for the current turn")),
    ).toBe("Wait for the current turn");
    expect(switchRefusal("provider_switch_refused: busy")).toBe("busy");
    expect(switchRefusal(new Error("spawn failed"))).toBeNull();
  });
});
