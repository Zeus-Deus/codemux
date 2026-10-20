/**
 * `?nativeTools=1`: a seeded Codex turn whose tool calls use the app-server
 * item envelopes the backend forwards verbatim (`commandExecution`,
 * `fileChange`, `mcpToolCall`, `dynamicToolCall`). Synthetic data only.
 */
export function codexNativeToolEnvelopes(threadId: string, turnId: string): object[] {
  const out: object[] = [];
  const item = (body: object) =>
    out.push({ type: "item_completed", thread_id: threadId, turn_id: turnId, item: body });
  const call = (id: string, toolName: string, input: object, content: unknown) => {
    item({ kind: "tool_use", tool_name: toolName, tool_use_id: id, input });
    item({ kind: "tool_result", tool_use_id: id, content, is_error: false });
  };
  const command = (id: string, script: string, output: string) =>
    call(
      id,
      "commandExecution",
      {
        type: "commandExecution",
        id,
        command: `/usr/bin/bash -lc '${script}'`,
        commandActions: [{ type: "unknown", command: script }],
        cwd: "/demo/parser",
        aggregatedOutput: null,
        exitCode: null,
        durationMs: null,
        status: "inProgress",
      },
      output,
    );

  out.push({
    type: "user_message",
    thread_id: threadId,
    client_nonce: "seed-nonce-native-tools",
    text: "Make parseRange treat the end bound as inclusive and cover it with a test",
  });
  command(
    "exec-native-search",
    `rg -n "parseRange" src`,
    "src/range.ts:12:export function parseRange(text: string): Range {\nsrc/cli.ts:48:  const range = parseRange(args.lines);",
  );
  const changes = [
    {
      path: "/demo/parser/src/range.ts",
      kind: { type: "update", move_path: null },
      diff: "@@ -14,3 +14,3 @@\n   const [start, end] = text.split(\"-\").map(Number);\n-  return { start, end: end - 1 };\n+  return { start, end };\n }\n",
    },
    {
      path: "/demo/parser/src/range.test.ts",
      kind: { type: "add" },
      diff: "import { parseRange } from \"./range\";\n\nit(\"keeps the end bound\", () => {\n  expect(parseRange(\"3-7\")).toEqual({ start: 3, end: 7 });\n});\n",
    },
  ];
  const fileChange = { type: "fileChange", id: "call_native_patch", changes, status: "inProgress" };
  call("call_native_patch", "fileChange", fileChange, { ...fileChange, status: "completed" });
  command("exec-native-test", "npm test -- src/range.test.ts", "✓ src/range.test.ts (1 test) 4ms\n\nTest Files  1 passed (1)\n     Tests  1 passed (1)");
  const docs = {
    type: "mcpToolCall",
    id: "call_native_docs",
    server: "docs",
    tool: "search",
    arguments: { query: "inclusive range semantics" },
    result: null,
    error: null,
    durationMs: null,
    status: "inProgress",
  };
  call("call_native_docs", "mcpToolCall", docs, {
    ...docs,
    status: "completed",
    durationMs: 210,
    result: {
      content: [{ type: "text", text: "guides/ranges.md: Ranges written as a-b include both bounds." }],
      structuredContent: null,
    },
  });
  const browser = {
    type: "dynamicToolCall",
    id: "exec-native-browser",
    namespace: null,
    tool: "codemux_mcp__codemux__browser_navigate",
    arguments: { url: "http://localhost:1420/ranges" },
    contentItems: null,
    success: null,
    durationMs: null,
    status: "inProgress",
  };
  call("exec-native-browser", "dynamicToolCall", browser, {
    ...browser,
    status: "completed",
    success: true,
    durationMs: 380,
    contentItems: [{ type: "inputText", text: "Navigated to http://localhost:1420/ranges (title: Range preview)" }],
  });
  item({
    kind: "assistant_text",
    text: "parseRange now keeps the end bound, so `3-7` covers lines 3 through 7. The new test passes.",
  });
  out.push({ type: "turn_completed", thread_id: threadId, turn_id: turnId, status: { kind: "success" }, usage: null });
  return out;
}
