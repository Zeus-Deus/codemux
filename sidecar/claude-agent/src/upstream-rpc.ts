// Outbound JSON-RPC requests from the sidecar back to Codemux.
//
// Stage 3 — the SDK's MCP tool callbacks need to round-trip through
// Codemux's Rust runtime so the user-installed MCP children spawn
// once and serve every Claude session. To do that the sidecar issues
// `mcp-tool-call` requests on its stdout; Rust's
// `JsonRpcChild::incoming_requests` channel routes them to the Claude
// session's request handler, which calls the registry's
// `dispatch_tool_call` and writes a response back on its stdin.
//
// This module owns:
//   * a monotonic outbound id allocator (negative ids so they can't
//     collide with the inbound ids `JsonRpcChild` allocates from 1↑),
//   * a pending-promise map keyed by id,
//   * `sendRequest()` — write a request line, await the typed result.
//
// `main.ts` calls `dispatchResponse()` whenever it sees a parsed
// response shape on stdin. If no callback is awaiting that id the
// response is ignored with a warning.

import type { JsonRpcResponse } from "./rpc.ts";
import { logger } from "./logger.ts";

/** How long an outbound request may wait before we time it out. The
 *  registry's own per-tools/call timeout (`DEFAULT_REQUEST_TIMEOUT` in
 *  `src-tauri/src/mcp/runtime.rs`) is 30 s, plus the MCP child's own
 *  budget; budget here is a touch more generous so the inner timeout
 *  always fires first and we get a tidy error. */
export const OUTBOUND_REQUEST_TIMEOUT_MS = 60_000;

/** Standard JSON-RPC error code for our timeout case. */
const RPC_INTERNAL_ERROR = -32603;

interface PendingResolver {
  resolve: (value: unknown) => void;
  reject: (err: Error) => void;
  timer: ReturnType<typeof setTimeout>;
  method: string;
}

/** Outbound id allocator. Negative numbers stay clearly distinct from
 *  the positive ids the Rust client allocates for its own requests so
 *  the two streams can never collide on the wire. */
let nextOutboundId = -1;
const pending = new Map<number, PendingResolver>();

/** Test seam: reset state between cases. */
export function _resetUpstreamRpcForTests(): void {
  for (const { reject, timer } of pending.values()) {
    clearTimeout(timer);
    reject(new Error("test reset"));
  }
  pending.clear();
  nextOutboundId = -1;
}

/** Issue an outbound JSON-RPC request, awaiting the matching response.
 *  `writeLine` is injected so tests can capture the wire payload. */
export async function sendUpstreamRequest(
  method: string,
  params: unknown,
  writeLine: (line: string) => void,
  timeoutMs: number = OUTBOUND_REQUEST_TIMEOUT_MS,
): Promise<unknown> {
  const id = nextOutboundId--;
  return await new Promise<unknown>((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(
        new Error(`upstream RPC '${method}' timed out after ${timeoutMs}ms`),
      );
    }, timeoutMs);
    pending.set(id, { resolve, reject, timer, method });
    // Wire shape MUST include `id` so the Rust client's reader routes
    // this as a server-initiated request, not a notification.
    try {
      writeLine(
        JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n",
      );
    } catch (err) {
      clearTimeout(timer);
      pending.delete(id);
      reject(err instanceof Error ? err : new Error(String(err)));
    }
  });
}

/** Route a parsed response to whichever caller is awaiting `id`. If
 *  no caller is awaiting (e.g. duplicate response, late timer fire)
 *  the response is dropped with a warning. */
export function dispatchResponse(msg: JsonRpcResponse): void {
  if (typeof msg.id !== "number") {
    logger.warn("upstream response with non-numeric id, ignoring", {
      id: msg.id,
    });
    return;
  }
  const slot = pending.get(msg.id);
  if (!slot) {
    logger.warn("upstream response for unknown id, ignoring", { id: msg.id });
    return;
  }
  pending.delete(msg.id);
  clearTimeout(slot.timer);
  if ("error" in msg) {
    slot.reject(
      new Error(
        `upstream RPC '${slot.method}' rejected: ${msg.error.message} (code ${msg.error.code})`,
      ),
    );
    return;
  }
  if ("result" in msg) {
    slot.resolve(msg.result);
    return;
  }
  // Defensive: a response with neither shape shouldn't be possible
  // (parseLine guards against it) but we'd rather fail the awaiting
  // caller than silently leak the slot.
  slot.reject(
    new Error(
      `upstream RPC '${slot.method}' returned malformed response (no result/error)`,
    ),
  );
}

/** Test seam: peek the pending map size. */
export function _pendingCountForTests(): number {
  return pending.size;
}

export { RPC_INTERNAL_ERROR };
