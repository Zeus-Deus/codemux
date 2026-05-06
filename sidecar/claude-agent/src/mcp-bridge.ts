// In-process MCP facade for the Claude Agent SDK.
//
// Stage 3 of Step 9 — the sidecar exposes a SINGLE virtual MCP server
// to the SDK (`Options.mcpServers["codemux"]` with `type: "sdk"`).
// All tool definitions for every user-installed MCP that Codemux is
// running funnel through this one server, with `mcp__<server>__<tool>`
// names matching the prefix Rust assigns.
//
// Tool calls fan back out via `sendUpstreamRequest("mcp-tool-call",
// ...)`, which the Claude session's incoming-requests task on the
// Rust side dispatches through the registry to the right MCP child.
//
// Module-level (not session-level) because:
//   * the SDK builds `Options.mcpServers` once per session, but the
//     pending-promise routing for outbound RPC is a process-wide
//     concern (responses arrive on stdin, not per-session).
//   * tests can inject a fake `writeLine` and call
//     `buildCodemuxMcpServer` directly without mounting a session.

import {
  createSdkMcpServer,
  type McpSdkServerConfigWithInstance,
  type SdkMcpToolDefinition,
} from "@anthropic-ai/claude-agent-sdk";
import { z } from "zod";

import { logger } from "./logger.ts";
import { sendUpstreamRequest } from "./upstream-rpc.ts";

/** Method name on the Rust side that handles tool dispatch. Mirrored
 *  in `src-tauri/src/agent_provider/claude/session.rs::METHOD_MCP_TOOL_CALL`. */
export const METHOD_MCP_TOOL_CALL = "mcp-tool-call";

/** Wire shape of one tool registration. Matches Rust's
 *  `commands/mcp.rs` serialization of `McpTool` (camelCase fields). */
export interface RegisteredMcpTool {
  /** Unprefixed tool name as the upstream MCP server returned it. */
  name: string;
  /** `mcp__<server>__<tool>` — agent-facing identifier. The SDK
   *  registers tools by this name; the model sees this exactly. */
  prefixedName: string;
  description: string | null;
  /** JSON-Schema `inputSchema` from the upstream `tools/list`. We
   *  convert to Zod for the SDK's typed handler hooks. */
  inputSchema: unknown;
  serverId: string;
}

/** Tool result shape MCP servers return from `tools/call`. Forwarded
 *  to the SDK callback verbatim — the SDK already understands this
 *  shape because it's the MCP spec. */
interface McpToolResult {
  content: Array<{ type: string; text?: string; [k: string]: unknown }>;
  isError?: boolean;
  [k: string]: unknown;
}

/** Process-wide writer hook. `main.ts` injects a real `process.stdout`
 *  writer; tests inject a capture buffer. */
let writeLine: (line: string) => void = () => {
  throw new Error("MCP bridge writer not initialised");
};

export function setMcpBridgeWriter(fn: (line: string) => void): void {
  writeLine = fn;
}

/**
 * Build the in-process MCP server config the Claude SDK will consume
 * via `Options.mcpServers["codemux"]`. Each tool is registered on the
 * server with a Zod schema derived from its JSON Schema; the handler
 * RPCs back to Rust to do the actual dispatch.
 *
 * Tools is the snapshot at session-start time. Stage 4 polish can
 * call `query.setMcpServers` mid-session to rebuild this when MCPs
 * come up after the chat has already started.
 */
export function buildCodemuxMcpServer(
  tools: RegisteredMcpTool[],
): McpSdkServerConfigWithInstance {
  const sdkTools: Array<SdkMcpToolDefinition> = tools.map((t) =>
    toolToSdkDefinition(t),
  );

  return createSdkMcpServer({
    name: "codemux",
    version: "0.1.0",
    tools: sdkTools,
  });
}

function toolToSdkDefinition(
  tool: RegisteredMcpTool,
): SdkMcpToolDefinition {
  const inputSchema = jsonSchemaToZodShape(tool.inputSchema);
  // Cast through `unknown`: the SDK's `SdkMcpToolDefinition<Schema>`
  // infers the handler's `args` from `Schema`, but our schema is
  // built dynamically so `Schema` resolves to the empty raw shape and
  // the handler ends up typed `(args: { readonly [x: string]: never })`.
  // The runtime contract is fine — the SDK validates with the schema
  // we passed and forwards the parsed object — we just need to widen
  // the type so the assignment compiles.
  const handler = (async (
    args: Record<string, unknown>,
  ): Promise<unknown> => {
    try {
      const result = await sendUpstreamRequest(
        METHOD_MCP_TOOL_CALL,
        { name: tool.prefixedName, arguments: args ?? {} },
        writeLine,
      );
      return normalizeToolResult(result);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      logger.warn("mcp tool call failed", {
        tool: tool.prefixedName,
        err: message,
      });
      return {
        content: [
          { type: "text" as const, text: `Tool call failed: ${message}` },
        ],
        isError: true,
      };
    }
  }) as unknown as SdkMcpToolDefinition["handler"];
  return {
    name: tool.prefixedName,
    description: tool.description ?? `${tool.name} (via ${tool.serverId})`,
    inputSchema,
    handler,
  };
}

/** Turn whatever the upstream returned into the canonical MCP tool
 *  result shape. The Rust dispatcher already produces this shape for
 *  successful calls; we tolerate stragglers (raw strings / non-objects)
 *  defensively. */
function normalizeToolResult(raw: unknown): McpToolResult {
  if (typeof raw === "object" && raw !== null && !Array.isArray(raw)) {
    const obj = raw as Record<string, unknown>;
    if (Array.isArray(obj["content"])) {
      return obj as unknown as McpToolResult;
    }
  }
  return {
    content: [{ type: "text", text: typeof raw === "string" ? raw : JSON.stringify(raw) }],
  };
}

/** Convert a JSON Schema object into a Zod raw shape `{key: zod, ...}`
 *  the SDK's `SdkMcpToolDefinition.inputSchema` accepts. We handle
 *  the shapes Anthropic-published MCP servers actually return:
 *    - `{ type: "object", properties: {...}, required: [...] }`
 *  Top-level non-object schemas fall back to `{}` (unknown input
 *  shape; the model is free to send arbitrary JSON).
 *
 *  This conversion is intentionally lossy: we preserve types,
 *  required-ness, descriptions, and enums; everything else becomes
 *  `z.unknown()`. The MCP server itself validates its own inputs
 *  upstream, so a too-permissive sidecar schema only costs us
 *  Zod-level validation — not safety. */
export function jsonSchemaToZodShape(
  schema: unknown,
): Record<string, z.ZodTypeAny> {
  if (
    typeof schema !== "object" ||
    schema === null ||
    Array.isArray(schema)
  ) {
    return {};
  }
  const obj = schema as Record<string, unknown>;
  if (obj["type"] !== "object") {
    return {};
  }
  const properties = obj["properties"];
  if (
    typeof properties !== "object" ||
    properties === null ||
    Array.isArray(properties)
  ) {
    return {};
  }
  const requiredArr = Array.isArray(obj["required"])
    ? (obj["required"] as unknown[]).filter(
        (v): v is string => typeof v === "string",
      )
    : [];
  const required = new Set(requiredArr);

  const shape: Record<string, z.ZodTypeAny> = {};
  for (const [key, value] of Object.entries(properties)) {
    const child = jsonSchemaPropertyToZod(value);
    shape[key] = required.has(key) ? child : child.optional();
  }
  return shape;
}

function jsonSchemaPropertyToZod(prop: unknown): z.ZodTypeAny {
  if (typeof prop !== "object" || prop === null || Array.isArray(prop)) {
    return z.unknown();
  }
  const obj = prop as Record<string, unknown>;
  const description =
    typeof obj["description"] === "string"
      ? (obj["description"] as string)
      : undefined;

  // `enum` short-circuits the type check. Pure-string enums get a
  // typed Zod enum; mixed-type enums fall through to z.unknown().
  if (Array.isArray(obj["enum"])) {
    const values = obj["enum"] as unknown[];
    if (values.length > 0 && values.every((v) => typeof v === "string")) {
      const result = z.enum([
        ...(values as string[]),
      ] as [string, ...string[]]);
      return description ? result.describe(description) : result;
    }
  }

  let base: z.ZodTypeAny;
  switch (obj["type"]) {
    case "string":
      base = z.string();
      break;
    case "number":
    case "integer":
      base = z.number();
      break;
    case "boolean":
      base = z.boolean();
      break;
    case "null":
      base = z.null();
      break;
    case "array":
      base = z.array(jsonSchemaPropertyToZod(obj["items"]));
      break;
    case "object": {
      // Nested objects: recurse into properties. For unrestricted
      // objects with no properties declared, fall back to
      // `z.record(z.unknown())` so the model can pass anything.
      const nested = jsonSchemaToZodShape(obj);
      if (Object.keys(nested).length === 0) {
        base = z.record(z.string(), z.unknown());
      } else {
        base = z.object(nested);
      }
      break;
    }
    default:
      base = z.unknown();
  }
  return description ? base.describe(description) : base;
}

// Test seam: re-export the writer for tests that want to assert on
// the wire payload directly.
export function _getWriterForTests(): (line: string) => void {
  return writeLine;
}
