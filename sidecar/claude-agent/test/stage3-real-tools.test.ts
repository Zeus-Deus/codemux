// Stage 3 bug repro — simulate the full chain from Rust-shaped
// `mcpTools` payload → `buildCodemuxMcpServer` → SDK config → assert
// the registered tools actually surface on the McpServer instance.
//
// If this test passes but the live agent doesn't see the tools, the
// bug is downstream (in SDK ↔ CLI wire). If this test fails, the bug
// is in our facade.

import { describe, expect, test } from "bun:test";

import {
  buildCodemuxMcpServer,
  setMcpBridgeWriter,
  type RegisteredMcpTool,
} from "../src/mcp-bridge.ts";

// Reset the writer between tests.
setMcpBridgeWriter(() => {});

/** Five real Codemux tool entries, copied verbatim from
 *  `mcp_server.rs::register_tools` shapes. If buildCodemuxMcpServer
 *  fails to register any of these, the live agent path also breaks. */
const REAL_CODEMUX_TOOLS: RegisteredMcpTool[] = [
  {
    name: "browser_navigate",
    prefixedName: "mcp__codemux__browser_navigate",
    description: "Navigate the browser pane to a URL",
    inputSchema: {
      type: "object",
      properties: {
        url: { type: "string", description: "The URL to navigate to" },
      },
      required: ["url"],
    },
    serverId: "codemux-self",
  },
  {
    name: "browser_snapshot",
    prefixedName: "mcp__codemux__browser_snapshot",
    description: "Get DOM elements with selectors",
    inputSchema: { type: "object", properties: {} },
    serverId: "codemux-self",
  },
  {
    name: "browser_click_at",
    prefixedName: "mcp__codemux__browser_click_at",
    description: "Click at coordinates",
    inputSchema: {
      type: "object",
      properties: {
        x: { type: "number" },
        y: { type: "number" },
        button: { enum: ["left", "right", "double"] },
      },
      required: ["x", "y"],
    },
    serverId: "codemux-self",
  },
  {
    name: "git_diff",
    prefixedName: "mcp__codemux__git_diff",
    description: "Run git diff",
    inputSchema: {
      type: "object",
      properties: {
        file: { type: "string" },
      },
    },
    serverId: "codemux-self",
  },
  {
    name: "notify",
    prefixedName: "mcp__codemux__notify",
    description: "Send a notification",
    inputSchema: {
      type: "object",
      properties: {
        message: { type: "string" },
        level: { enum: ["info", "attention"] },
      },
      required: ["message"],
    },
    serverId: "codemux-self",
  },
];

describe("buildCodemuxMcpServer with real Codemux tool shapes", () => {
  test("returns a config without throwing", () => {
    const config = buildCodemuxMcpServer(REAL_CODEMUX_TOOLS);
    expect(config.type).toBe("sdk");
    expect(config.name).toBe("codemux");
    expect(config.instance).toBeDefined();
  });

  test("instance has the right tool count registered", () => {
    const config = buildCodemuxMcpServer(REAL_CODEMUX_TOOLS);
    // The McpServer instance internally tracks registered tools. We
    // can't enumerate them via the public API, but a non-zero
    // registration is observable via the toolCount property if it
    // exists, or by inspecting the underlying server.
    const inst = config.instance as unknown as {
      _registeredTools?: Record<string, unknown>;
      server?: { _registeredTools?: Record<string, unknown> };
    };
    const tools = inst._registeredTools ?? inst.server?._registeredTools;
    expect(tools).toBeDefined();
    expect(Object.keys(tools!).length).toBe(REAL_CODEMUX_TOOLS.length);
    expect(tools!["mcp__codemux__browser_navigate"]).toBeDefined();
    expect(tools!["mcp__codemux__notify"]).toBeDefined();
  });

  test("registered tool names match prefixedName field exactly", () => {
    const config = buildCodemuxMcpServer(REAL_CODEMUX_TOOLS);
    const inst = config.instance as unknown as {
      _registeredTools?: Record<string, unknown>;
      server?: { _registeredTools?: Record<string, unknown> };
    };
    const tools = inst._registeredTools ?? inst.server?._registeredTools;
    const names = Object.keys(tools!).sort();
    const expected = REAL_CODEMUX_TOOLS.map((t) => t.prefixedName).sort();
    expect(names).toEqual(expected);
  });

  test("empty tool list is harmless", () => {
    const config = buildCodemuxMcpServer([]);
    expect(config.type).toBe("sdk");
    expect(config.instance).toBeDefined();
  });
});
