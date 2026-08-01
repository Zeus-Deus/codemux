// @vitest-environment jsdom
import { beforeAll, describe, expect, it } from "vitest";

import { LAZY_TOOL_RESULT_KEY } from "@/lib/agent-chat/lazy-tool-result";
import { replayPayloads } from "@/lib/agent-chat/hydrate";
import type { AgentChatMessageRow } from "@/tauri/commands";

import { STRESS_THREAD_PREFIX } from "./stress-fixture";

/**
 * The dev mock has to serve the SAME shapes the Rust read path does, or
 * `npm run dev` exercises a fiction. This drives the mock's own invoke
 * surface: a stress transcript whose tool results are big enough to be
 * stubbed, read by cursor, with the full body still fetchable by row id.
 *
 * Fixture spec: few turns, large payload budget — the named presets
 * spread their budget thin enough that each individual result lands
 * under the threshold.
 */
type Invoke = (cmd: string, args?: Record<string, unknown>) => Promise<unknown>;

const THREAD = `${STRESS_THREAD_PREFIX}3`;
let invoke: Invoke;

beforeAll(async () => {
  localStorage.setItem(
    "codemux:fixture",
    JSON.stringify({ chatEvents: 200, payloadMb: 5 }),
  );
  await import("./tauri-mock");
  invoke = (
    window as unknown as { __TAURI_INTERNALS__: { invoke: Invoke } }
  ).__TAURI_INTERNALS__.invoke;
});

describe("dev mock cursor reads", () => {
  it("serves rows with ascending ids and honours the cursor", async () => {
    const all = (await invoke("agent_chat_list_messages_after", {
      threadId: THREAD,
      afterId: null,
    })) as AgentChatMessageRow[];
    expect(all.length).toBeGreaterThan(0);
    expect(all.every((r, i) => i === 0 || r.id > all[i - 1].id)).toBe(true);

    const tail = (await invoke("agent_chat_list_messages_after", {
      threadId: THREAD,
      afterId: all[all.length - 2].id,
    })) as AgentChatMessageRow[];
    expect(tail).toHaveLength(1);
    expect(tail[0].id).toBe(all[all.length - 1].id);

    const head = await invoke("agent_chat_thread_head_id", { threadId: THREAD });
    expect(head).toBe(all[all.length - 1].id);
    expect(
      await invoke("agent_chat_thread_head_id", { threadId: "nope" }),
    ).toBeNull();
  });

  it("stubs oversized tool results and serves the full body by row id", async () => {
    const rows = (await invoke("agent_chat_list_messages_after", {
      threadId: THREAD,
      afterId: null,
    })) as AgentChatMessageRow[];

    const stubbed = rows.filter((r) => r.payload.includes(LAZY_TOOL_RESULT_KEY));
    expect(stubbed.length).toBeGreaterThan(0);

    const stub = JSON.parse(stubbed[0].payload).item.content[
      LAZY_TOOL_RESULT_KEY
    ] as { row_id: number; bytes: number; preview: string; line_count: number };
    expect(stub.row_id).toBe(stubbed[0].id);
    expect(stub.bytes).toBeGreaterThan(32 * 1024);
    // The shipped row is a small fraction of the body it stands in for.
    expect(stubbed[0].payload.length).toBeLessThan(stub.bytes / 4);

    const full = (await invoke("agent_chat_get_tool_result", {
      rowId: stub.row_id,
    })) as string;
    const content = JSON.parse(full).item.content as string;
    expect(content.length).toBeGreaterThan(32 * 1024);
    expect(content.startsWith(stub.preview)).toBe(true);
    expect(content.split("\n")).toHaveLength(stub.line_count);
  });

  it("replays a stubbed transcript without losing any rows", async () => {
    // The stub travels through the reducer as ordinary content, so a
    // shaped transcript rebuilds the same item count as an unshaped one.
    const rows = (await invoke("agent_chat_list_messages_after", {
      threadId: THREAD,
      afterId: null,
    })) as AgentChatMessageRow[];
    const shaped = replayPayloads(rows.map((r) => r.payload));
    const raw = replayPayloads(
      (await invoke("agent_chat_list_messages", {
        threadId: THREAD,
      })) as string[],
    );
    expect(shaped.messages.map((m) => m.kind)).toEqual(
      raw.messages.map((m) => m.kind),
    );
  });
});

describe("dev mock stub preview parity with the Rust shaper", () => {
  const KEY = LAZY_TOOL_RESULT_KEY;

  function toolResult(content: unknown): string {
    return JSON.stringify({
      type: "item_completed",
      thread_id: "t",
      turn_id: "turn-1",
      item: {
        kind: "tool_result",
        tool_use_id: "tu-1",
        content,
        is_error: false,
      },
    });
  }

  function stubOf(payload: string) {
    return JSON.parse(payload).item?.content?.[KEY] as
      | { preview: string; line_count: number; bytes: number }
      | undefined;
  }

  it("flattens a block array by joining text fields, not by dumping JSON", async () => {
    const { mockShapePayload } = await import("./tauri-mock");
    const line = "captured output line\n";
    const body = line.repeat(3_000);
    const shaped = mockShapePayload(
      5,
      toolResult([
        { type: "text", text: body },
        { type: "text", text: "tail" },
      ]),
    );
    const stub = stubOf(shaped);
    expect(stub).toBeDefined();
    // `stringify_tool_content` joins entries with "\n" and takes their
    // `text` verbatim — a JSON dump would start with "[" and escape.
    expect(stub!.preview.startsWith(line)).toBe(true);
    expect(stub!.line_count).toBe(`${body}\ntail`.split("\n").length);
  });

  it("truncates the preview on a UTF-8 byte boundary", async () => {
    const { mockShapePayload } = await import("./tauri-mock");
    // Multi-byte throughout: a UTF-16 slice would cut in a different
    // place than Rust's byte-bounded truncation, and could split a char.
    const shaped = mockShapePayload(6, toolResult("é".repeat(40_000)));
    const stub = stubOf(shaped);
    expect(stub).toBeDefined();
    expect(stub!.preview).not.toContain("�");
    const previewBytes = new TextEncoder().encode(stub!.preview).length;
    expect(previewBytes).toBeLessThanOrEqual(2 * 1024);
    // Right up against the cap, not arbitrarily short.
    expect(previewBytes).toBeGreaterThan(2 * 1024 - 4);
  });

  it("does not stub a body carrying a renderable image block", async () => {
    const { mockShapePayload } = await import("./tauri-mock");
    const data = "A".repeat(40_000);
    const payload = toolResult([
      { type: "text", text: "screenshot:" },
      {
        type: "image",
        source: { type: "base64", media_type: "image/png", data },
      },
    ]);
    expect(mockShapePayload(7, payload)).toBe(payload);

    // …but a base64 PDF is not an image, and must be stubbed.
    const pdf = toolResult([
      {
        type: "image",
        source: { type: "base64", media_type: "application/pdf", data },
      },
    ]);
    expect(stubOf(mockShapePayload(8, pdf))).toBeDefined();
  });
});
