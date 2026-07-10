// FakeQuery — test double implementing the SDK's `Query` contract.
//
// Supports:
//   * Script-driven emission of `SDKMessage` values by calling
//     `emit(msg)`. The iterator `yield`s each emitted message.
//   * The 4 control methods production integrations actually call —
//     `interrupt`, `setModel`, `setPermissionMode`, `close` — plus
//     `initializationResult` for the probe path.
//   * All other `Query` methods are implemented as no-ops that return
//     a plausible shape. Attempting to call one that isn't mocked
//     throws so the test notices.

import type { Options, SDKMessage, SDKUserMessage, Query,
  PermissionMode } from "@anthropic-ai/claude-agent-sdk";

type EmittedItem =
  | { kind: "message"; value: SDKMessage }
  | { kind: "end" }
  | { kind: "error"; error: Error };

/** A controllable fake implementing enough of `Query` to drive the
 *  session. */
export class FakeQuery implements AsyncIterable<SDKMessage> {
  readonly capturedPrompts: SDKUserMessage[] = [];
  readonly capturedOptions: Options | undefined;

  public setModelCalls: Array<string | undefined> = [];
  public setPermissionModeCalls: PermissionMode[] = [];
  public interruptCalls = 0;
  public closed = false;
  /** When set, `interrupt()` throws this from the iterator, emulating
   *  the real SDK aborting the query when interrupted. Leave null to
   *  emulate a soft interrupt where the iterator keeps running. */
  public interruptError: Error | null = null;

  public initResult: unknown = {
    commands: [],
    agents: [],
    output_style: "default",
    available_output_styles: ["default"],
    models: [],
    account: {},
  };

  private queue: EmittedItem[] = [];
  private resolveNext:
    | {
        resolve: (v: IteratorResult<SDKMessage>) => void;
        reject: (err: unknown) => void;
      }
    | null = null;

  constructor(args: {
    prompt: string | AsyncIterable<SDKUserMessage>;
    options?: Options;
  }) {
    this.capturedOptions = args.options;
    if (typeof args.prompt !== "string") {
      const iterable = args.prompt;
      // Consume the prompt iterable in the background so sendTurn
      // pushes actually reach our capture buffer.
      void (async () => {
        for await (const msg of iterable) {
          this.capturedPrompts.push(msg);
        }
      })();
    }
  }

  /** Script an `SDKMessage` into the iterator. */
  emit(msg: SDKMessage): void {
    if (this.resolveNext) {
      const r = this.resolveNext;
      this.resolveNext = null;
      r.resolve({ value: msg, done: false });
      return;
    }
    this.queue.push({ kind: "message", value: msg });
  }

  /** Signal end-of-stream — the iterator terminates cleanly. */
  endStream(): void {
    if (this.resolveNext) {
      const r = this.resolveNext;
      this.resolveNext = null;
      r.resolve({ value: undefined as unknown as SDKMessage, done: true });
      return;
    }
    this.queue.push({ kind: "end" });
  }

  /** Throw from inside the iterator. */
  errorStream(error: Error): void {
    if (this.resolveNext) {
      const r = this.resolveNext;
      this.resolveNext = null;
      r.reject(error);
      return;
    }
    this.queue.push({ kind: "error", error });
  }

  [Symbol.asyncIterator](): AsyncIterator<SDKMessage> {
    return {
      next: (): Promise<IteratorResult<SDKMessage>> => {
        if (this.queue.length > 0) {
          const item = this.queue.shift();
          if (!item) {
            // Unreachable — length check above — but keeps TS happy.
            return Promise.resolve({
              value: undefined as unknown as SDKMessage,
              done: true,
            });
          }
          if (item.kind === "message") {
            return Promise.resolve({ value: item.value, done: false });
          }
          if (item.kind === "end") {
            return Promise.resolve({
              value: undefined as unknown as SDKMessage,
              done: true,
            });
          }
          return Promise.reject(item.error);
        }
        return new Promise<IteratorResult<SDKMessage>>((resolve, reject) => {
          this.resolveNext = { resolve, reject };
        });
      },
      return: (): Promise<IteratorResult<SDKMessage>> => {
        this.closed = true;
        return Promise.resolve({
          value: undefined as unknown as SDKMessage,
          done: true,
        });
      },
    };
  }

  // ---------- Control methods the session calls -----------------------

  async interrupt(): Promise<void> {
    this.interruptCalls += 1;
    if (this.interruptError) {
      this.errorStream(this.interruptError);
    }
  }

  async setModel(model?: string): Promise<void> {
    this.setModelCalls.push(model);
  }

  async setPermissionMode(mode: PermissionMode): Promise<void> {
    this.setPermissionModeCalls.push(mode);
  }

  async initializationResult(): Promise<unknown> {
    return this.initResult;
  }

  close(): void {
    this.closed = true;
    this.endStream();
  }

  // ---------- Shims for the rest of the Query surface ------------------
  // Session integration uses none of these. A test that triggers one
  // unexpectedly gets a loud failure.

  async setMaxThinkingTokens(): Promise<void> {
    throw new Error("FakeQuery.setMaxThinkingTokens not implemented");
  }
  async applyFlagSettings(): Promise<void> {
    throw new Error("FakeQuery.applyFlagSettings not implemented");
  }
  async supportedCommands(): Promise<[]> {
    return [];
  }
  async supportedModels(): Promise<[]> {
    return [];
  }
  async supportedAgents(): Promise<[]> {
    return [];
  }
  async mcpServerStatus(): Promise<[]> {
    return [];
  }
  async getContextUsage(): Promise<Record<string, never>> {
    return {};
  }
  async reloadPlugins(): Promise<Record<string, never>> {
    return {};
  }
  async accountInfo(): Promise<Record<string, never>> {
    return {};
  }
  async rewindFiles(): Promise<Record<string, never>> {
    return {};
  }
  async seedReadState(): Promise<void> {
    /* noop */
  }
  async reconnectMcpServer(): Promise<void> {
    /* noop */
  }
  async toggleMcpServer(): Promise<void> {
    /* noop */
  }
  async setMcpServers(): Promise<Record<string, never>> {
    return {};
  }
  async streamInput(): Promise<void> {
    /* noop */
  }
  async stopTask(): Promise<void> {
    /* noop */
  }

  // AsyncGenerator requires throw/return — our iterator wraps them.
  async throw(err?: unknown): Promise<IteratorResult<SDKMessage>> {
    this.closed = true;
    throw err ?? new Error("FakeQuery.throw");
  }

  async [Symbol.asyncDispose](): Promise<void> {
    this.closed = true;
  }
}

/** Cast helper that lets callers substitute a `FakeQuery` for a real
 *  SDK `Query` without wrestling with structural-vs-nominal type
 *  mismatch. Safe because sessions only touch the async-iterable
 *  surface + 4 control methods, all of which `FakeQuery` implements. */
export function asQuery(fake: FakeQuery): Query {
  return fake as unknown as Query;
}
