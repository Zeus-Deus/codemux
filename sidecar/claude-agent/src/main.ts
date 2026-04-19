// Entry point for the claude-agent sidecar.
//
// * Reads newline-delimited JSON-RPC 2.0 messages from stdin.
// * Dispatches requests to a fixed registry (currently only `ping`).
// * Writes responses to stdout, logs everything else to stderr.
// * Exits 0 on stdin EOF, SIGTERM, or SIGINT; exits 1 on unhandled
//   exceptions or rejections.
//
// This file deliberately lives at the edge — no business logic — so
// that future methods just register themselves in `methods` without
// risking protocol drift.

import {
  formatMessage,
  isRequest,
  parseLine,
  RPC_INTERNAL_ERROR,
  RPC_METHOD_NOT_FOUND,
  RPC_PARSE_ERROR,
  type JsonRpcId,
  type JsonRpcOutgoing,
} from "./rpc.ts";
import { logger } from "./logger.ts";
import { ping } from "./methods/ping.ts";

type MethodHandler = (params: unknown) => Promise<unknown>;

/** Registry of implemented methods. Keep the registry the single source
 *  of truth — `main.ts` should not contain any method-specific logic. */
const methods: Record<string, MethodHandler> = {
  ping: (params) => ping((params ?? {}) as Record<string, unknown>),
};

function writeMessage(msg: JsonRpcOutgoing): void {
  process.stdout.write(formatMessage(msg));
}

function writeParseError(line: string, err: unknown): void {
  writeMessage({
    jsonrpc: "2.0",
    id: null,
    error: {
      code: RPC_PARSE_ERROR,
      message: err instanceof Error ? err.message : String(err),
      data: { line },
    },
  });
}

function writeMethodNotFound(id: JsonRpcId, method: string): void {
  writeMessage({
    jsonrpc: "2.0",
    id,
    error: {
      code: RPC_METHOD_NOT_FOUND,
      message: `unknown method: ${method}`,
    },
  });
}

function writeInternalError(id: JsonRpcId, err: unknown): void {
  const e = err instanceof Error ? err : new Error(String(err));
  writeMessage({
    jsonrpc: "2.0",
    id,
    error: {
      code: RPC_INTERNAL_ERROR,
      message: e.message,
      data: { stack: e.stack ?? null },
    },
  });
}

/** Dispatch one framed line. Any failure produces a well-formed
 *  JSON-RPC error response rather than throwing. */
async function handleLine(raw: string): Promise<void> {
  let msg;
  try {
    msg = parseLine(raw);
  } catch (err) {
    writeParseError(raw, err);
    return;
  }
  if (msg === null) return; // blank line, silently skip

  // `isRequest` narrows `msg` to a type carrying `id`; capturing the
  // narrowed value keeps subsequent branches type-safe.
  if (isRequest(msg)) {
    const handler = methods[msg.method];
    if (!handler) {
      writeMethodNotFound(msg.id, msg.method);
      return;
    }
    try {
      const result = await handler(msg.params);
      writeMessage({ jsonrpc: "2.0", id: msg.id, result });
    } catch (err) {
      writeInternalError(msg.id, err);
    }
    return;
  }

  // Notification path: execute but never respond.
  const handler = methods[msg.method];
  if (!handler) {
    logger.warn("notification for unknown method", { method: msg.method });
    return;
  }
  try {
    await handler(msg.params);
  } catch (err) {
    logger.error("notification handler threw", {
      method: msg.method,
      err: err instanceof Error ? err.message : String(err),
    });
  }
}

/** Drive stdin as a byte stream, split on `\n`, and dispatch each
 *  line. Tolerates partial lines across chunk boundaries. */
async function runStdinLoop(): Promise<void> {
  const decoder = new TextDecoder();
  let buffer = "";
  for await (const chunk of process.stdin as AsyncIterable<Uint8Array>) {
    buffer += decoder.decode(chunk, { stream: true });
    while (true) {
      const nl = buffer.indexOf("\n");
      if (nl === -1) break;
      const line = buffer.slice(0, nl);
      buffer = buffer.slice(nl + 1);
      await handleLine(line);
    }
  }
  // Flush any decoder state and handle a trailing no-newline line.
  buffer += decoder.decode();
  if (buffer.trim() !== "") {
    await handleLine(buffer);
  }
}

async function main(): Promise<void> {
  logger.info("sidecar started", { pid: process.pid });

  process.on("uncaughtException", (err: Error) => {
    logger.error("uncaught exception", {
      err: err.message,
      stack: err.stack ?? null,
    });
    process.exit(1);
  });
  process.on("unhandledRejection", (reason: unknown) => {
    logger.error("unhandled rejection", {
      reason: reason instanceof Error ? reason.message : String(reason),
    });
    process.exit(1);
  });
  process.on("SIGTERM", () => {
    logger.info("received SIGTERM, exiting");
    process.exit(0);
  });
  process.on("SIGINT", () => {
    logger.info("received SIGINT, exiting");
    process.exit(0);
  });

  await runStdinLoop();
  logger.info("stdin closed, exiting");
  process.exit(0);
}

main().catch((err: unknown) => {
  logger.error("main threw", {
    err: err instanceof Error ? err.message : String(err),
  });
  process.exit(1);
});
