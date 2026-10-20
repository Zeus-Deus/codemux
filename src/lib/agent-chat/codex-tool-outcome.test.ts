import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";

import type { ProviderRuntimeEvent } from "@/tauri/events";

import { replayPayloads } from "./hydrate";
import { applyEvent, createEmptyThreadState } from "./reducer";

// Synthetic native envelopes based on `codex-cli 0.162.1 app-server generate-ts
// --experimental`; shared with the Rust adapter to pin live/replay parity.
const cases: Array<{
  name: string;
  item: { type: string; id: string; [key: string]: unknown };
  outcome: "success" | "error" | "unconfirmed";
}> = JSON.parse(readFileSync(resolve(process.cwd(),
  "src-tauri/src/agent_provider/codex/fixtures/tool-outcomes.json",
), "utf8"));

const statuses = { success: "done", error: "error", unconfirmed: "unconfirmed" } as const;

describe("Codex tool outcomes", () => {
  for (const fixture of cases) {
    it(`${fixture.name}: live and historical flags follow native evidence`, () => {
      const use: ProviderRuntimeEvent = {
        type: "item_completed", thread_id: "t", turn_id: "turn",
        item: {
          kind: "tool_use", tool_use_id: fixture.item.id,
          tool_name: fixture.item.type, input: { ...fixture.item, status: "inProgress" },
        },
      };
      const result: ProviderRuntimeEvent = {
        type: "item_completed", thread_id: "t", turn_id: "turn",
        item: {
          kind: "tool_result", tool_use_id: fixture.item.id,
          content: fixture.item, is_error: false,
        },
      };
      for (const events of [[use, result], [result, use]]) {
        const live = events.reduce((state, event) => applyEvent(state, event), createEmptyThreadState());
        const replayed = replayPayloads(events.map((event) => JSON.stringify(event)));
        for (const state of [live, replayed]) {
          const tool = state.messages.find((item) => item.kind === "tool_call");
          expect(tool?.status).toBe(statuses[fixture.outcome]);
          expect(tool?.result_content).toEqual(fixture.item);
        }
      }
    });

    it(`${fixture.name}: lazy history retains the bounded native outcome`, () => {
      const content = {
        __codemux_lazy_tool_result: { row_id: 7, bytes: 40_000, preview: "output", line_count: 1, has_images: false },
        __codemux_codex_tool_outcome: { tool_use_id: fixture.item.id, status: statuses[fixture.outcome] },
      };
      const event: ProviderRuntimeEvent = {
        type: "item_completed", thread_id: "t", turn_id: "turn",
        item: { kind: "tool_result", tool_use_id: fixture.item.id, is_error: false, content },
      };
      const live = applyEvent(createEmptyThreadState(), event);
      const replayed = replayPayloads([JSON.stringify(event)]);
      for (const state of [live, replayed]) {
        const tool = state.messages.find((item) => item.kind === "tool_call");
        expect(tool?.status).toBe(statuses[fixture.outcome]);
        expect(tool?.result_content).toEqual(content);
      }
    });
  }

  it("keeps an explicit canonical error authoritative over native and lazy success", () => {
    for (const content of [cases[0].item, {
      __codemux_lazy_tool_result: {},
      __codemux_codex_tool_outcome: { tool_use_id: "tool-0", status: "done" },
    }]) {
      const event: ProviderRuntimeEvent = {
        type: "item_completed", thread_id: "t", turn_id: "turn",
        item: { kind: "tool_result", tool_use_id: "tool-0", is_error: true, content },
      };
      for (const state of [applyEvent(createEmptyThreadState(), event), replayPayloads([JSON.stringify(event)])]) {
        expect(state.messages.find((item) => item.kind === "tool_call")?.status).toBe("error");
      }
    }
  });

  it("normalizes native failures inside a subagent transcript", () => {
    const native = cases.find((fixture) => fixture.name === "dynamic fails")!.item;
    const event: ProviderRuntimeEvent = {
      type: "item_completed", thread_id: "t", turn_id: "turn", subagent_id: "child",
      item: { kind: "tool_result", tool_use_id: native.id, is_error: false, content: native },
    };
    for (const state of [applyEvent(createEmptyThreadState(), event), replayPayloads([JSON.stringify(event)])]) {
      const run = state.messages.find((item) => item.kind === "subagent_run");
      expect(run?.subagents[0].items.find((item) => item.kind === "tool_call")?.status).toBe("error");
    }
  });

  it("uses native parent results to fail a child without turning unknown results into success", () => {
    for (const status of ["failed", "inProgress"]) {
      const events: ProviderRuntimeEvent[] = [{
        type: "subagent_updated", thread_id: "t",
        subagent: { subagent_id: "child", status: "running", parent_item_id: "spawn" },
      }, {
        type: "item_completed", thread_id: "t", turn_id: "turn",
        item: { kind: "tool_result", tool_use_id: "spawn", is_error: false,
          content: { type: "dynamicToolCall", id: "spawn", status } },
      }];
      const live = events.reduce((state, event) => applyEvent(state, event), createEmptyThreadState());
      const replayed = replayPayloads(events.map((event) => JSON.stringify(event)));
      for (const state of [live, replayed]) {
        const run = state.messages.find((item) => item.kind === "subagent_run");
        if (status === "failed") expect(run?.subagents[0].status).toBe("failed");
        else expect(run?.subagents[0].status).not.toBe("completed");
      }
    }
  });

  it("leaves stdout-only history and unrelated structured results on their canonical flag", () => {
    for (const content of ["old command stdout", { success: false, status: "failed" },
      { type: "mcpToolCall", id: "another-call", status: "failed" },
      { __codemux_codex_tool_outcome: { tool_use_id: "tool", status: "error" } },
      { __codemux_lazy_tool_result: {}, __codemux_codex_tool_outcome: { tool_use_id: "other", status: "error" } },
    ]) {
      for (const is_error of [false, true]) {
        const event: ProviderRuntimeEvent = {
          type: "item_completed", thread_id: "t", turn_id: "turn",
          item: { kind: "tool_result", tool_use_id: "tool", content, is_error },
        };
        for (const state of [applyEvent(createEmptyThreadState(), event), replayPayloads([JSON.stringify(event)])]) {
          expect(state.messages.find((item) => item.kind === "tool_call")?.status).toBe(is_error ? "error" : "done");
        }
      }
    }
  });
});
