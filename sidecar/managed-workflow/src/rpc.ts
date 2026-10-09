import { createInterface } from "node:readline";
import type { Json } from "./contract";
import { object } from "./contract";

const MAX_LINE_BYTES = 2 * 1024 * 1024;
const MAX_PENDING = 64;
const REQUEST_TIMEOUT_MS = 120_000;

/** Host responses stay readable while an asynchronous provider request is running. */
export class Rpc {
  private nextId = 0;
  private closed = false;
  private readonly pending = new Map<string, { resolve: (value: Json) => void; reject: (error: Error) => void; timer: ReturnType<typeof setTimeout> }>();

  constructor(private readonly write: (line: string) => void) {}

  private send(message: unknown): void {
    if (this.closed) throw new Error("Managed host transport is closed");
    const line = JSON.stringify(message);
    if (Buffer.byteLength(line) > MAX_LINE_BYTES) throw new Error("Managed RPC message exceeds size limit");
    this.write(line + "\n");
  }

  notify(method: string, params: Json): void { this.send({ jsonrpc: "2.0", method, params }); }

  request(method: string, params: Json): Promise<Json> {
    if (this.closed || this.pending.size >= MAX_PENDING) return Promise.reject(new Error("Managed host transport unavailable"));
    const id = `managed-${++this.nextId}`;
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.pending.delete(id);
        reject(new Error("Managed host tool response timed out"));
      }, REQUEST_TIMEOUT_MS);
      this.pending.set(id, { resolve, reject, timer });
      try { this.send({ jsonrpc: "2.0", id, method, params }); }
      catch (error) {
        clearTimeout(timer); this.pending.delete(id);
        reject(error instanceof Error ? error : new Error("Managed RPC send failed"));
      }
    });
  }

  async accept(line: string, dispatch: (method: string, params: unknown) => Promise<Json>): Promise<void> {
    if (Buffer.byteLength(line) > MAX_LINE_BYTES) throw new Error("Managed RPC input exceeds size limit");
    const message: unknown = JSON.parse(line);
    if (!object(message) || message.jsonrpc !== "2.0") throw new Error("Invalid managed RPC envelope");
    if (typeof message.method !== "string") {
      if (typeof message.id !== "string") throw new Error("Invalid managed RPC response id");
      const pending = this.pending.get(message.id);
      if (!pending) throw new Error("Unexpected managed RPC response");
      this.pending.delete(message.id); clearTimeout(pending.timer);
      if (object(message.error) && typeof message.error.message === "string") {
        pending.reject(new Error(message.error.message));
      } else if ("result" in message && !("error" in message)) {
        pending.resolve(message.result as Json);
      } else {
        pending.reject(new Error("Malformed managed RPC response"));
        throw new Error("Malformed managed RPC response");
      }
      return;
    }
    if (!(typeof message.id === "number" || typeof message.id === "string")) {
      throw new Error("Managed RPC requires request identifiers");
    }
    try {
      this.send({ jsonrpc: "2.0", id: message.id, result: await dispatch(message.method, message.params) });
    } catch (error) {
      this.send({ jsonrpc: "2.0", id: message.id, error: { code: -32000, message: error instanceof Error ? error.message : "Managed request failed" } });
    }
  }

  close(): void {
    this.closed = true;
    for (const pending of this.pending.values()) {
      clearTimeout(pending.timer); pending.reject(new Error("Managed host transport is closed"));
    }
    this.pending.clear();
  }
}

export async function serve(rpc: Rpc, dispatch: (method: string, params: unknown) => Promise<Json>, dispose: () => Promise<void>): Promise<void> {
  const lines = createInterface({ input: process.stdin, crlfDelay: Infinity });
  let fatal = false;
  const tasks = new Set<Promise<void>>();
  for await (const line of lines) {
    if (fatal) break;
    const task = rpc.accept(line, dispatch).catch(() => {
      fatal = true; rpc.close(); lines.close(); process.exitCode = 1;
    });
    tasks.add(task);
    void task.finally(() => tasks.delete(task));
  }
  rpc.close();
  await dispose();
  await Promise.allSettled(tasks);
}
