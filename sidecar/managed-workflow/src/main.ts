import { Agent, Cursor, JsonlLocalAgentStore } from "@cursor/sdk/bundled";
import { CursorRuntime, CURSOR_ADAPTER_VERSION } from "./cursor";
import { OpenCodeRuntime, OPENCODE_ADAPTER_VERSION, OPENCODE_NATIVE_VERSION } from "./opencode";
import { object, parseInitialize } from "./contract";
import type { Json, Runtime } from "./contract";
import { Rpc, serve } from "./rpc";
import { probeCursorNatives } from "./cursor-probe";

// stdout is protocol-only, including during SDK initialization.
for (const name of ["log", "info", "debug", "warn"] as const) {
  console[name] = (...values: unknown[]) => console.error(...values);
}

if (process.argv.includes("--check-native")) {
  await probeCursorNatives();
} else if (process.argv.includes("--check")) {
  process.stdout.write(JSON.stringify({ protocolVersion: 1, cursorAdapterVersion: CURSOR_ADAPTER_VERSION,
    openCodeAdapterVersion: OPENCODE_ADAPTER_VERSION, openCodeNativeVersion: OPENCODE_NATIVE_VERSION }) + "\n");
} else {
  const rpc = new Rpc((line) => process.stdout.write(line));
  let runtime: Runtime | undefined;
  let initializing = false;
  let closed = false;
  async function dispose(): Promise<void> {
    if (closed) return;
    closed = true;
    // The Rust parent independently kills and verifies the owned process group.
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await Promise.race([runtime?.dispose(), new Promise<void>((resolve) => { timer = setTimeout(resolve, 5_000); })]);
    } finally { if (timer) clearTimeout(timer); }
  }
  async function dispatch(method: string, params: unknown): Promise<Json> {
    if (closed) throw new Error("Managed attempt is closed");
    switch (method) {
      case "initialize": {
        if (runtime || initializing) throw new Error("Managed attempt cannot be initialized twice");
        initializing = true;
        const input = parseInitialize(params);
        if (input.provider === "opencode") runtime = new OpenCodeRuntime(rpc);
        else if (input.provider === "cursor") runtime = new CursorRuntime({
          create: (options) => Agent.create(options),
          authStatus: () => Cursor.auth.status(),
          createStore: (path) => new JsonlLocalAgentStore(path),
        }, rpc);
        else throw new Error("Unsupported managed workflow provider");
        return await runtime.initialize(input) as unknown as Json;
      }
      case "startTurn": {
        if (!runtime || !object(params) || typeof params.turnId !== "string" || typeof params.prompt !== "string") {
          throw new Error("Invalid managed turn request");
        }
        await runtime.startTurn({ turnId: params.turnId, prompt: params.prompt });
        return {};
      }
      case "cancel": if (!runtime) throw new Error("Managed attempt is uninitialized"); await runtime.cancel(); return {};
      case "shutdown": await dispose(); return {};
      default: throw new Error("Unknown managed workflow method");
    }
  }
  await serve(rpc, dispatch, dispose);
}
