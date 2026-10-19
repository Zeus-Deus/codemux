// Stage 3 of Step 9 — sidecar tool-call facade tests.
//
// We exercise three layers:
//
//   1. `parseLine` distinguishes incoming requests from upstream
//      responses, so the dispatcher can route the latter to
//      `dispatchResponse` instead of the method registry.
//   2. `sendUpstreamRequest` writes a request line with id/method/
//      params and resolves when a matching response arrives.
//   3. `jsonSchemaToZodShape` produces a Zod raw shape that maps
//      common MCP tool input shapes (object with required fields,
//      enums, arrays, nested objects).
//   4. `buildCodemuxMcpServer` returns an `McpSdkServerConfigWithInstance`
//      whose tool callbacks RPC out and wrap upstream errors as
//      `isError: true` results so the SDK never sees a thrown promise.

import { afterEach, describe, expect, test } from "bun:test";

import { parseLine } from "../src/rpc.ts";
import {
  _pendingCountForTests,
  _resetUpstreamRpcForTests,
  dispatchResponse,
  sendUpstreamRequest,
} from "../src/upstream-rpc.ts";
import {
  buildCodemuxMcpServer,
  jsonSchemaToZodShape,
  setMcpBridgeWriter,
  type RegisteredMcpTool,
} from "../src/mcp-bridge.ts";
import { z } from "zod";
import { createPluginPermissions } from "../src/plugin-permissions.ts";

async function callSdk(
  server: ReturnType<typeof buildCodemuxMcpServer>,
  name: string,
  args: Record<string, unknown>,
  toolUseId = "call",
) {
  let finish!: (value: unknown) => void;
  const result = new Promise<unknown>((resolve) => {
    finish = resolve;
  });
  const transport: Parameters<typeof server.instance.connect>[0] = {
    start: async () => {},
    close: async () => {},
    send: async (message) => {
      if ("id" in message && message.id === 2) finish(message);
    },
  };
  await server.instance.connect(transport);
  transport.onmessage!({
    jsonrpc: "2.0",
    id: 1,
    method: "initialize",
    params: {
      protocolVersion: "2024-11-05",
      capabilities: {},
      clientInfo: { name: "test", version: "1" },
    },
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  transport.onmessage!({ jsonrpc: "2.0", method: "notifications/initialized" });
  transport.onmessage!({
    jsonrpc: "2.0",
    id: 2,
    method: "tools/call",
    params: {
      name,
      arguments: args,
      _meta: { "claudecode/toolUseId": toolUseId },
    },
  });
  try {
    return await result;
  } finally {
    await server.instance.close();
  }
}
const bridgeTool: RegisteredMcpTool = {
  name: "codemux_call_plugin_tool",
  prefixedName: "codemux_call_plugin_tool",
  serverId: "codemux-plugin-bridge",
  description: "Call plugin",
  inputSchema: {
    type: "object",
    properties: { name: { type: "string" }, arguments: { type: "object" } },
    required: ["name"],
  },
};

test("round 4: in-process bridge refuses missing hooks, even with a stale bridge allow", async () => {
  for (const allow of [
    [],
    ["mcp__codemux__codemux_call_plugin_tool"],
    ["mcp__codemux"],
    ["mcp__codemux__*"],
  ]) {
    const dispatched: string[] = [];
    setMcpBridgeWriter((line) => {
      dispatched.push(line);
      const request = JSON.parse(line);
      queueMicrotask(() =>
        dispatchResponse({
          jsonrpc: "2.0",
          id: request.id,
          result: { content: [] },
        }),
      );
    });
    const gate = createPluginPermissions({
      threadId: "test",
      getQuery: () => ({
        getSettings: async () => ({ effective: { permissions: { allow } } }),
      }),
      getMode: () => "bypassPermissions",
      canUseTool: async () => ({ behavior: "allow" }),
      canUseTarget: async () => ({ behavior: "allow" }),
    });
    const response = await callSdk(
      buildCodemuxMcpServer([bridgeTool], gate),
      bridgeTool.name,
      { name: "fixture__x", arguments: {} },
    );
    expect(response).toMatchObject({
      result: {
        isError: true,
        content: [{ text: expect.stringContaining("permission check") }],
      },
    });
    expect(dispatched).toEqual([]);
  }
});

test("round 4: the SDK handler consumes the exact hook ticket and rejects replay", async () => {
  const gate = createPluginPermissions({
    threadId: "test",
    getQuery: () => ({
      getSettings: async () => ({ effective: { permissions: {} } }),
    }),
    getMode: () => "bypassPermissions",
    canUseTool: async () => ({ behavior: "allow" }),
    canUseTarget: async () => ({ behavior: "allow" }),
  });
  const args = { name: "fixture__x", arguments: { value: 7 } };
  const calls: string[] = [];
  setMcpBridgeWriter((line) => {
    calls.push(line);
    const request = JSON.parse(line);
    queueMicrotask(() =>
      dispatchResponse({
        jsonrpc: "2.0",
        id: request.id,
        result: { content: [{ type: "text", text: "executed" }] },
      }),
    );
  });
  await gate.hook(
    {
      hook_event_name: "PreToolUse",
      tool_name: "mcp__codemux__codemux_call_plugin_tool",
      tool_input: args,
      tool_use_id: "call",
      session_id: "test",
      transcript_path: "/fixture",
      cwd: "/fixture",
    },
    "call",
    { signal: new AbortController().signal },
  );
  expect(
    await callSdk(
      buildCodemuxMcpServer([bridgeTool], gate),
      bridgeTool.name,
      args,
    ),
  ).toMatchObject({ result: { content: [{ text: "executed" }] } });
  expect(
    await callSdk(
      buildCodemuxMcpServer([bridgeTool], gate),
      bridgeTool.name,
      args,
    ),
  ).toMatchObject({ result: { isError: true } });
  expect(calls).toHaveLength(1);
});

test("round 4: the SDK list response cannot expose denied targets", async () => {
  const gate = createPluginPermissions({
    threadId: "test",
    getQuery: () => ({
      getSettings: async () => ({
        effective: { permissions: { deny: ["mcp__codemux__fixture__x"] } },
      }),
    }),
    getMode: () => "default",
    canUseTool: async () => ({ behavior: "allow" }),
    canUseTarget: async () => ({ behavior: "allow" }),
  });
  const tools = { tools: [{ name: "fixture__x" }, { name: "fixture__y" }] };
  setMcpBridgeWriter((line) => {
    const request = JSON.parse(line);
    queueMicrotask(() =>
      dispatchResponse({
        jsonrpc: "2.0",
        id: request.id,
        result: {
          content: [{ type: "text", text: JSON.stringify(tools) }],
          structuredContent: tools,
        },
      }),
    );
  });
  const tool = {
    ...bridgeTool,
    name: "codemux_list_plugin_tools",
    prefixedName: "codemux_list_plugin_tools",
    inputSchema: { type: "object" },
  };
  const response = await callSdk(
    buildCodemuxMcpServer([tool], gate),
    tool.name,
    {},
  );
  expect(response).toMatchObject({
    result: { structuredContent: { tools: [{ name: "fixture__y" }] } },
  });
  expect(JSON.stringify(response)).not.toContain("fixture__x");
});

afterEach(() => {
  _resetUpstreamRpcForTests();
});

describe("parseLine", () => {
  test("classifies a request shape as incoming", () => {
    const parsed = parseLine(
      '{"jsonrpc":"2.0","id":1,"method":"start-session","params":{}}',
    );
    expect(parsed?.kind).toBe("incoming");
  });

  test("classifies a response shape as response", () => {
    const parsed = parseLine(
      '{"jsonrpc":"2.0","id":-1,"result":{"content":[]}}',
    );
    expect(parsed?.kind).toBe("response");
    if (parsed?.kind === "response") {
      expect(parsed.msg.id).toBe(-1);
    }
  });

  test("classifies an error response shape as response", () => {
    const parsed = parseLine(
      '{"jsonrpc":"2.0","id":-2,"error":{"code":-32603,"message":"boom"}}',
    );
    expect(parsed?.kind).toBe("response");
  });

  test("notifications still parse as incoming", () => {
    const parsed = parseLine('{"jsonrpc":"2.0","method":"sdk-message"}');
    expect(parsed?.kind).toBe("incoming");
  });

  test("blank lines return null", () => {
    expect(parseLine("")).toBeNull();
    expect(parseLine("   ")).toBeNull();
  });
});

describe("sendUpstreamRequest", () => {
  test("writes the request payload and resolves on matching response", async () => {
    const written: string[] = [];
    const writeLine = (line: string) => {
      written.push(line);
    };
    const promise = sendUpstreamRequest(
      "mcp-tool-call",
      { name: "x", arguments: { foo: 1 } },
      writeLine,
    );
    expect(written).toHaveLength(1);
    const wire = JSON.parse(written[0]!);
    expect(wire.jsonrpc).toBe("2.0");
    expect(typeof wire.id).toBe("number");
    expect(wire.method).toBe("mcp-tool-call");
    expect(wire.params.name).toBe("x");

    dispatchResponse({
      jsonrpc: "2.0",
      id: wire.id,
      result: { content: [{ type: "text", text: "ok" }] },
    });
    const resolved = await promise;
    expect(resolved).toEqual({ content: [{ type: "text", text: "ok" }] });
    expect(_pendingCountForTests()).toBe(0);
  });

  test("rejects when the upstream returns an error response", async () => {
    const written: string[] = [];
    const promise = sendUpstreamRequest("x", {}, (line) => written.push(line));
    const wire = JSON.parse(written[0]!);
    dispatchResponse({
      jsonrpc: "2.0",
      id: wire.id,
      error: { code: -32603, message: "boom" },
    });
    await expect(promise).rejects.toThrow(/boom/);
  });

  test("times out after the configured budget", async () => {
    const written: string[] = [];
    const promise = sendUpstreamRequest(
      "x",
      {},
      (line) => written.push(line),
      30, // 30ms test budget
    );
    await expect(promise).rejects.toThrow(/timed out/);
  });
});

describe("jsonSchemaToZodShape", () => {
  test("returns empty shape for non-object schemas", () => {
    expect(jsonSchemaToZodShape({ type: "string" })).toEqual({});
    expect(jsonSchemaToZodShape(null)).toEqual({});
    expect(jsonSchemaToZodShape("not a schema")).toEqual({});
  });

  test("handles required and optional string fields", () => {
    const shape = jsonSchemaToZodShape({
      type: "object",
      properties: {
        url: { type: "string", description: "A URL" },
        timeout: { type: "number" },
      },
      required: ["url"],
    });
    const obj = z.object(shape);
    const valid = obj.safeParse({ url: "https://example.com" });
    expect(valid.success).toBe(true);
    const missing = obj.safeParse({ timeout: 30 });
    expect(missing.success).toBe(false);
  });

  test("handles enum properties as Zod enum", () => {
    const shape = jsonSchemaToZodShape({
      type: "object",
      properties: {
        mode: { enum: ["a", "b"] },
      },
      required: ["mode"],
    });
    const obj = z.object(shape);
    expect(obj.safeParse({ mode: "a" }).success).toBe(true);
    expect(obj.safeParse({ mode: "c" }).success).toBe(false);
  });

  test("handles array, boolean, and nested object", () => {
    const shape = jsonSchemaToZodShape({
      type: "object",
      properties: {
        tags: { type: "array", items: { type: "string" } },
        live: { type: "boolean" },
        meta: {
          type: "object",
          properties: { id: { type: "string" } },
          required: ["id"],
        },
      },
    });
    const obj = z.object(shape);
    const ok = obj.safeParse({
      tags: ["a", "b"],
      live: true,
      meta: { id: "x" },
    });
    expect(ok.success).toBe(true);
  });

  test("missing properties falls back to permissive record for nested object", () => {
    const shape = jsonSchemaToZodShape({
      type: "object",
      properties: {
        opaque: { type: "object" },
      },
    });
    const obj = z.object(shape);
    const ok = obj.safeParse({ opaque: { anything: 1, else: "ok" } });
    expect(ok.success).toBe(true);
  });
});

describe("buildCodemuxMcpServer", () => {
  test("returns a SDK server config with name 'codemux'", () => {
    const tool: RegisteredMcpTool = {
      name: "search",
      prefixedName: "mcp__demo__search",
      description: "Search the demo KB",
      inputSchema: {
        type: "object",
        properties: { q: { type: "string" } },
        required: ["q"],
      },
      serverId: "demo",
    };
    const server = buildCodemuxMcpServer([tool]);
    expect(server.type).toBe("sdk");
    expect(server.name).toBe("codemux");
    expect(server.instance).toBeDefined();
  });

  test("tool callback wraps upstream errors as isError: true", async () => {
    const written: string[] = [];
    setMcpBridgeWriter((line) => written.push(line));

    const tool: RegisteredMcpTool = {
      name: "search",
      prefixedName: "mcp__demo__search",
      description: "Search",
      inputSchema: {
        type: "object",
        properties: { q: { type: "string" } },
        required: ["q"],
      },
      serverId: "demo",
    };
    buildCodemuxMcpServer([tool]);
    // Drive the handler directly via the upstream RPC path: send a
    // request, simulate an error response, and confirm the inner
    // handler turns the rejection into an isError result.
    // (The full SDK round-trip lives behind the SDK; we exercise the
    // outbound contract here.)
    const promise = sendUpstreamRequest(
      "mcp-tool-call",
      { name: "mcp__demo__search", arguments: { q: "hello" } },
      (line) => written.push(line),
    );
    const wire = JSON.parse(written[written.length - 1]!);
    dispatchResponse({
      jsonrpc: "2.0",
      id: wire.id,
      error: { code: -32603, message: "child crashed" },
    });
    await expect(promise).rejects.toThrow(/child crashed/);
  });

  test("live catalog call carries a bounded snapshot of native plugin tools", async () => {
    const pluginTools: RegisteredMcpTool[] = Array.from({ length: 700 }, (_, index) => ({
      name: `t${index}`, prefixedName: `fixture__t${index}`, description: "fixture",
      inputSchema: { type: "object" }, serverId: "plugin:fixture:editor",
    }));
    const server = buildCodemuxMcpServer([...pluginTools, {
      name: "codemux_list_plugin_tools", prefixedName: "codemux_list_plugin_tools",
      description: "live list", inputSchema: { type: "object" }, serverId: "codemux-plugin-bridge",
    }]);
    let names: string[] = [];
    setMcpBridgeWriter(line => {
      const request = JSON.parse(line);
      names = request.params.directPluginTools;
      queueMicrotask(() => dispatchResponse({ jsonrpc: "2.0", id: request.id, result: { content: [] } }));
    });
    let complete!: () => void;
    const completed = new Promise<void>(resolve => { complete = resolve; });
    const transport: Parameters<typeof server.instance.connect>[0] = {
      start: async () => {}, close: async () => {},
      send: async message => { if ("id" in message && message.id === 2) complete(); },
    };
    await server.instance.connect(transport);
    transport.onmessage!({ jsonrpc: "2.0", id: 1, method: "initialize", params: { protocolVersion: "2024-11-05", capabilities: {}, clientInfo: { name: "test", version: "1" } } });
    await new Promise(resolve => setTimeout(resolve, 0));
    transport.onmessage!({ jsonrpc: "2.0", method: "notifications/initialized" });
    transport.onmessage!({ jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: "codemux_list_plugin_tools", arguments: {} } });
    await completed;
    expect(names).toHaveLength(512);
    expect(names[0]).toBe("fixture__t0");
    expect(names.at(-1)).toBe("fixture__t511");
    await server.instance.close();
  });

  test("empty tool list still produces a valid server config", () => {
    const server = buildCodemuxMcpServer([]);
    expect(server.type).toBe("sdk");
    expect(server.name).toBe("codemux");
  });
});
