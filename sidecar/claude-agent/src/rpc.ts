// JSON-RPC 2.0 envelope types plus small parse/format helpers.
//
// The sidecar talks over stdin/stdout using newline-delimited JSON,
// matching what `JsonRpcChild` on the Rust side expects. Every line is
// exactly one JSON object followed by `\n`.

export type JsonRpcId = number | string | null;

/** Incoming message with an `id` — the client wants a response. */
export interface JsonRpcRequest {
  jsonrpc: "2.0";
  id: JsonRpcId;
  method: string;
  params?: unknown;
}

/** Incoming message without an `id` — fire-and-forget. */
export interface JsonRpcNotification {
  jsonrpc: "2.0";
  method: string;
  params?: unknown;
}

export interface JsonRpcSuccessResponse {
  jsonrpc: "2.0";
  id: JsonRpcId;
  result: unknown;
}

export interface JsonRpcErrorResponse {
  jsonrpc: "2.0";
  id: JsonRpcId;
  error: JsonRpcError;
}

export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

export type JsonRpcResponse = JsonRpcSuccessResponse | JsonRpcErrorResponse;
export type JsonRpcIncoming = JsonRpcRequest | JsonRpcNotification;
export type JsonRpcOutgoing = JsonRpcResponse | JsonRpcNotification;

// Standard JSON-RPC 2.0 error codes (spec: https://www.jsonrpc.org/specification).
export const RPC_PARSE_ERROR = -32700;
export const RPC_INVALID_REQUEST = -32600;
export const RPC_METHOD_NOT_FOUND = -32601;
export const RPC_INVALID_PARAMS = -32602;
export const RPC_INTERNAL_ERROR = -32603;

/**
 * Discriminate a parsed incoming message: `true` means the sender is
 * expecting a response.
 */
export function isRequest(msg: JsonRpcIncoming): msg is JsonRpcRequest {
  return "id" in msg && (msg as { id?: unknown }).id !== undefined;
}

/** Stage 3: a parsed line is either an incoming request/notification
 *  (the sidecar's pre-existing role as RPC server) OR a response to a
 *  request the sidecar issued upstream (the sidecar's new role as RPC
 *  client for tool callbacks). The dispatcher in `main.ts` picks the
 *  right path based on whether `method` is present. */
export type ParsedLine =
  | { kind: "incoming"; msg: JsonRpcIncoming }
  | { kind: "response"; msg: JsonRpcResponse };

/**
 * Parse a single framed JSON-RPC line. Returns `null` for an empty /
 * whitespace-only line (which callers should simply skip). Throws a
 * plain `Error` for malformed JSON. Distinguishes incoming
 * (request/notification) from response shapes for the bidirectional
 * tool-call flow Stage 3 relies on.
 */
export function parseLine(line: string): ParsedLine | null {
  const trimmed = line.trim();
  if (trimmed === "") return null;

  const parsed: unknown = JSON.parse(trimmed);
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed)) {
    throw new Error("message is not a JSON object");
  }
  const obj = parsed as Record<string, unknown>;
  if (obj["jsonrpc"] !== "2.0") {
    throw new Error("invalid or missing jsonrpc version");
  }

  // Response shape: id present, method absent, has either `result` or
  // `error`. These flow into the upstream-rpc pending map.
  if (
    typeof obj["method"] !== "string" &&
    "id" in obj &&
    ("result" in obj || "error" in obj)
  ) {
    return { kind: "response", msg: obj as unknown as JsonRpcResponse };
  }

  // Otherwise it must be a request or notification.
  if (typeof obj["method"] !== "string") {
    throw new Error("missing method");
  }
  return { kind: "incoming", msg: obj as unknown as JsonRpcIncoming };
}

/**
 * Serialize an outgoing message with the trailing `\n` required by the
 * framing used on the Rust side.
 */
export function formatMessage(msg: JsonRpcOutgoing): string {
  return JSON.stringify(msg) + "\n";
}
