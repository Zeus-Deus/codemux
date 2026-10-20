import { describe, expect, it } from "vitest";

import { nativeToolCall, presentationToolName, toolPresentation } from "./native-tool-call";
import type { ToolCallItem } from "./types";

function call(tool_name: string, input: unknown, result_content: unknown = null): ToolCallItem {
  return {
    kind: "tool_call",
    id: `tc-${tool_name}`,
    seq: 0,
    tool_use_id: "exec-1",
    tool_name,
    input,
    status: result_content == null ? "running" : "done",
    result_content,
    approval_request_id: null,
  };
}

const commandEnvelope = (command: unknown) => ({
  type: "commandExecution",
  id: "exec-1",
  command,
  cwd: "/repo",
  aggregatedOutput: null,
  exitCode: null,
  status: "inProgress",
});

describe("nativeToolCall: commandExecution", () => {
  it("unwraps the login-shell wrapper Codex reports", () => {
    const item = call("commandExecution", commandEnvelope("/usr/bin/bash -lc 'git status --short'"), "M a.ts\n");
    expect(nativeToolCall(item)).toEqual({
      kind: "command",
      command: "git status --short",
      output: "M a.ts\n",
      exitCode: null,
    });
    expect(toolPresentation(item)).toEqual({ name: "Bash", input: { command: "git status --short" } });
  });

  it("decodes quoting inside the wrapped script", () => {
    const single = call("commandExecution", commandEnvelope(`/usr/bin/bash -lc 'echo '"'"'hi'"'"' && rg -n "x" src'`));
    expect(nativeToolCall(single)).toMatchObject({ command: `echo 'hi' && rg -n "x" src` });
    const double = call("commandExecution", commandEnvelope(`/bin/zsh -c "printf \\"%s\\" \\$HOME"`));
    expect(nativeToolCall(double)).toMatchObject({ command: `printf "%s" $HOME` });
  });

  it("keeps a command it cannot decode as one word verbatim", () => {
    const raw = "/usr/bin/bash -lc 'unterminated";
    expect(nativeToolCall(call("commandExecution", commandEnvelope(raw)))).toMatchObject({ command: raw });
    expect(nativeToolCall(call("commandExecution", commandEnvelope("cargo test")))).toMatchObject({
      command: "cargo test",
    });
    // Operators and expansions outside quotes make more than one word.
    const unquoted = "/bin/bash -c echo;true";
    expect(nativeToolCall(call("commandExecution", commandEnvelope(unquoted)))).toMatchObject({ command: unquoted });
  });

  it("accepts an argv array", () => {
    const item = call("commandExecution", commandEnvelope(["bash", "-lc", "ls -la"]));
    expect(nativeToolCall(item)).toMatchObject({ command: "ls -la" });
  });

  it("reads output and exit code from a settled envelope result", () => {
    const item = call(
      "commandExecution",
      commandEnvelope("/usr/bin/bash -lc 'npm test'"),
      { ...commandEnvelope("/usr/bin/bash -lc 'npm test'"), aggregatedOutput: "1 failed\n", exitCode: 1, status: "failed" },
    );
    expect(nativeToolCall(item)).toMatchObject({ command: "npm test", output: "1 failed\n", exitCode: 1 });
  });
});

describe("nativeToolCall: fileChange", () => {
  it("normalises each change with line counts", () => {
    const item = call("fileChange", {
      type: "fileChange",
      id: "call-1",
      status: "completed",
      changes: [
        { path: "/repo/src/a.ts", kind: { type: "update", move_path: null }, diff: "@@ -1,2 +1,2 @@\n-old\n+new\n same\n" },
        { path: "/repo/src/b.ts", kind: { type: "add" }, diff: "one\ntwo\n" },
        { path: "/repo/src/c.ts", kind: { type: "delete" }, diff: "gone\n" },
        { kind: { type: "add" }, diff: "no path" },
      ],
    });
    const native = nativeToolCall(item);
    expect(native?.kind).toBe("file_change");
    if (native?.kind !== "file_change") return;
    expect(native.changes.map(({ path, kind, added, removed }) => ({ path, kind, added, removed }))).toEqual([
      { path: "/repo/src/a.ts", kind: "update", added: 1, removed: 1 },
      { path: "/repo/src/b.ts", kind: "add", added: 2, removed: 0 },
      { path: "/repo/src/c.ts", kind: "delete", added: 0, removed: 1 },
    ]);
    expect(toolPresentation(item)).toEqual({ name: "Edit", input: { file_path: "/repo/src/a.ts" } });
  });

  it("presents an all-new-files change as Write", () => {
    const item = call("fileChange", {
      type: "fileChange",
      changes: [{ path: "/repo/new.md", kind: { type: "add" }, diff: "# hi\n" }],
    });
    expect(presentationToolName(item)).toBe("Write");
  });
});

describe("nativeToolCall: MCP and dynamic tools", () => {
  it("names an MCP call by server and tool, with its arguments and result blocks", () => {
    const input = {
      type: "mcpToolCall",
      id: "call_abc",
      server: "docs",
      tool: "search",
      arguments: { query: "retry policy" },
      result: null,
      error: null,
      status: "inProgress",
    };
    const settled = {
      ...input,
      status: "completed",
      result: { content: [{ type: "text", text: "3 matches" }], structuredContent: null },
    };
    expect(nativeToolCall(call("mcpToolCall", input))).toEqual({
      kind: "tool",
      name: "mcp__docs__search",
      args: { query: "retry policy" },
      output: null,
      error: null,
    });
    expect(nativeToolCall(call("mcpToolCall", input, settled))).toMatchObject({
      output: [{ type: "text", text: "3 matches" }],
    });
    expect(
      nativeToolCall(
        call("mcpToolCall", input, { ...settled, result: { content: [], structuredContent: { hits: 3 } } }),
      ),
    ).toMatchObject({ output: [JSON.stringify({ hits: 3 }, null, 2)] });
    expect(
      nativeToolCall(call("mcpToolCall", input, { ...settled, result: null, error: { message: "server gone" } })),
    ).toMatchObject({ output: null, error: "server gone" });
  });

  it("keeps structured MCP results next to their summary, unless a block already echoes them", () => {
    const input = { type: "mcpToolCall", id: "c", server: "docs", tool: "search", arguments: {} };
    const summary = { type: "text", text: "3 matches" };
    const structured = { hits: [{ file: "guide.md" }] };
    const result = (content: unknown[]) => ({
      ...input,
      status: "completed",
      result: { content, structuredContent: structured },
    });
    expect(nativeToolCall(call("mcpToolCall", input, result([summary])))).toMatchObject({
      output: [summary, JSON.stringify(structured, null, 2)],
    });
    const echo = { type: "text", text: JSON.stringify(structured, null, 2) };
    expect(nativeToolCall(call("mcpToolCall", input, result([echo])))).toMatchObject({ output: [echo] });
    // Structured data shaped like a text block is still shown in full.
    const lookalike = { ...input, status: "completed", result: { content: [], structuredContent: { type: "text", text: "Summary", payload: 1 } } };
    expect(nativeToolCall(call("mcpToolCall", input, lookalike))).toMatchObject({
      output: [JSON.stringify({ type: "text", text: "Summary", payload: 1 }, null, 2)],
    });
  });

  it("uses a dynamic call's own tool name and parses string arguments", () => {
    const input = {
      type: "dynamicToolCall",
      id: "exec-9",
      tool: "codemux_mcp__codemux__browser_navigate",
      arguments: '{"url":"http://localhost:1420"}',
      contentItems: null,
      success: null,
      status: "inProgress",
    };
    const item = call("dynamicToolCall", input, {
      ...input,
      status: "completed",
      success: true,
      contentItems: [{ type: "inputText", text: "Navigated" }],
    });
    expect(nativeToolCall(item)).toEqual({
      kind: "tool",
      name: "codemux_mcp__codemux__browser_navigate",
      args: { url: "http://localhost:1420" },
      output: [{ type: "inputText", text: "Navigated" }],
      error: null,
    });
    expect(toolPresentation(item)).toEqual({
      name: "codemux_mcp__codemux__browser_navigate",
      input: { url: "http://localhost:1420" },
    });
  });

  it("leaves other calls and malformed envelopes alone", () => {
    const lookalike = call("commandExecution", { command: "status" }, [{ type: "text", text: "ok" }]);
    expect(nativeToolCall(lookalike)).toBeNull();
    expect(presentationToolName(lookalike)).toBe("commandExecution");
    const bash = call("Bash", { command: "ls" });
    expect(nativeToolCall(bash)).toBeNull();
    expect(toolPresentation(bash)).toEqual({ name: "Bash", input: { command: "ls" } });
    const noTool = call("dynamicToolCall", { type: "dynamicToolCall", id: "x" });
    expect(nativeToolCall(noTool)).toBeNull();
    expect(presentationToolName(noTool)).toBe("dynamicToolCall");
  });
});
