// Entry point for the claude-agent sidecar.
//
// * Reads newline-delimited JSON-RPC 2.0 messages from stdin.
// * Dispatches requests against the method registry built in
//   `methods/index.ts`.
// * Writes responses and notifications to stdout.
// * All logging goes to stderr — stdout is reserved for the protocol
//   channel.
// * Exits 0 on stdin EOF, SIGTERM, or SIGINT; exits 1 on unhandled
//   exceptions or rejections.

import {
  formatMessage,
  isRequest,
  parseLine,
  RPC_INTERNAL_ERROR,
  RPC_INVALID_PARAMS,
  RPC_METHOD_NOT_FOUND,
  RPC_PARSE_ERROR,
  type JsonRpcId,
  type JsonRpcOutgoing,
} from "./rpc.ts";
import { logger } from "./logger.ts";
import {
  buildMethods,
  InvalidParamsError,
  type MethodHandler,
} from "./methods/index.ts";
import type { EventEmitter } from "./session.ts";

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

function writeError(id: JsonRpcId, err: unknown): void {
  if (err instanceof InvalidParamsError) {
    writeMessage({
      jsonrpc: "2.0",
      id,
      error: {
        code: RPC_INVALID_PARAMS,
        message: err.message,
      },
    });
    return;
  }
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

/** Event emitter passed into the session and permissions subsystems.
 *  Each call turns into a JSON-RPC notification on stdout. */
const emitter: EventEmitter = {
  notification(method, params) {
    writeMessage({
      jsonrpc: "2.0",
      method,
      params,
    });
  },
};

const methods: Record<string, MethodHandler> = buildMethods(emitter);

async function handleLine(raw: string): Promise<void> {
  let msg;
  try {
    msg = parseLine(raw);
  } catch (err) {
    writeParseError(raw, err);
    return;
  }
  if (msg === null) return;

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
      writeError(msg.id, err);
    }
    return;
  }

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
