// ClaudeSession — one live SDK `query()` per chat thread.
//
// Streaming-input mode always: the `prompt` handed to the SDK is an
// `AsyncPromptQueue<SDKUserMessage>` that user turns push into. This
// shape is what unlocks `interrupt`, `setModel`, and
// `setPermissionMode` on the returned `Query`.
//
// SDK messages arrive via the returned `Query`'s async iterator. Each
// message is forwarded to the RPC layer as an opaque JSON payload —
// the Rust side owns classification.
//
// Three special-case exceptions to the pass-through rule:
//
//   * `ExitPlanMode` tool uses surface as a `plan-proposed`
//     notification (in addition to the raw `sdk-message`), and the
//     permission callback denies the tool with a stop-now message.
//   * `AskUserQuestion` tool uses surface as a `user-input-requested`
//     notification (in addition to the raw `sdk-message`).
//   * `permission-request` events fired from the permission bridge
//     the first time a tool is gated.

import {
  query as defaultQuery,
  type CanUseTool,
  type Options,
  type PermissionMode,
  type Query,
  type SDKMessage,
  type SDKUserMessage,
} from "@anthropic-ai/claude-agent-sdk";

import { AsyncPromptQueue } from "./async-queue.ts";
import {
  makeCanUseTool,
  type ApprovalDecision,
  type PendingApprovals,
  type PermissionsEmitter,
} from "./permissions.ts";
import { logger } from "./logger.ts";

/** User-facing event channel. The RPC dispatcher implements this by
 *  writing a JSON-RPC notification line to stdout. */
export interface EventEmitter extends PermissionsEmitter {
  notification(method: string, params: unknown): void;
}

/** Parameters for `start-session`. Mirrors the subset of SDK
 *  `Options` a production integration actually sets; everything else
 *  is intentionally left unset (see the research report §14). */
export interface SessionStartInput {
  /** Runtime-owned thread identifier (not the SDK's session uuid). */
  threadId: string;
  /** Absolute working directory for the session. Required. */
  cwd: string;
  /** Optional initial model id — opaque string passed to the SDK. */
  model?: string;
  /** Reasoning-effort hint. Stored as a raw string and cast through
   *  `unknown` into the SDK's `EffortLevel` so that callers can pass
   *  values like `"xhigh"` / `"max"` even if a future SDK release
   *  tightens the union. */
  effort?: string;
  /** Initial permission mode. */
  permissionMode?: PermissionMode;
  /** Must be `true` when `permissionMode === "bypassPermissions"`. */
  allowDangerouslySkipPermissions?: boolean;
  /** Extra directories (beyond `cwd`) the agent is allowed to touch. */
  additionalDirectories?: string[];
  /** Flag-layer settings, forwarded opaquely. */
  settings?: Record<string, unknown>;
  /** SDK session uuid to resume from. */
  resume?: string;
  /** Explicit uuid for a brand-new session. */
  sessionId?: string;
  /** Absolute path to the user's local `claude` binary. Required. */
  pathToClaudeCodeExecutable: string;
  /** Raw CLI flags the user added in settings. `null` value means
   *  the flag is a boolean. */
  extraArgs?: Record<string, string | null>;
}

/** Parameters for `send-turn`. Images are omitted for now — the UI
 *  does not yet need them. */
export interface SendTurnInput {
  /** User text. */
  text: string;
  /** Per-turn model override (falls back to the session default). */
  modelOverride?: string;
}

// ---------------------------------------------------------------------------
// Dependency-injection seam for tests. Real RPCs use the SDK's `query`;
// tests swap in a `FakeQuery` factory.
// ---------------------------------------------------------------------------

/** Signature of the SDK's `query` function, pinned for DI. */
export type QueryFactory = (args: {
  prompt: string | AsyncIterable<SDKUserMessage>;
  options?: Options;
}) => Query;

let queryFactory: QueryFactory = defaultQuery;

/** Swap the factory so tests can hand in a fake `Query`. */
export function setQueryFactoryForTests(factory: QueryFactory): void {
  queryFactory = factory;
}

/** Restore the default factory. Call from test teardown. */
export function resetQueryFactoryForTests(): void {
  queryFactory = defaultQuery;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Heuristic: is this thrown value an abort / interrupt rather than a
 *  genuine error? Matches the pattern the research report §9c saw in
 *  production — no `import { AbortError }`, just string checks. */
function isAbortLikeError(err: unknown): boolean {
  if (err === null || err === undefined) return false;
  const message = err instanceof Error ? err.message : String(err);
  const name = err instanceof Error ? err.name : "";
  if (name === "AbortError") return true;
  const m = message.toLowerCase();
  return (
    m.includes("all fibers interrupted without error") ||
    m.includes("request was aborted") ||
    m.includes("interrupted by user") ||
    m.includes("aborted")
  );
}

/** Build the full SDK `Options` object from our public `SessionStartInput`.
 *  Every field is set conditionally except the two hardcoded
 *  infrastructure flags (`settingSources`, `includePartialMessages`) and
 *  the two always-required ones (`pathToClaudeCodeExecutable`, `env`). */
function buildQueryOptions(
  input: SessionStartInput,
  canUseTool: CanUseTool,
): Options {
  const opts: Partial<Options> = {
    // Always.
    cwd: input.cwd,
    pathToClaudeCodeExecutable: input.pathToClaudeCodeExecutable,
    settingSources: ["user", "project", "local"],
    includePartialMessages: true,
    canUseTool,
    // `env: process.env` pass-through matches the research finding.
    // The SDK itself is responsible for reading the user's Claude
    // credentials — the sidecar never peeks.
    env: process.env as Record<string, string | undefined>,
  };

  if (input.model !== undefined) opts.model = input.model;
  if (input.effort !== undefined) {
    // `EffortLevel` in 0.2.114 accepts "xhigh" and "max", but older
    // and newer versions have historically tightened/loosened this
    // union. Cast through `unknown` so the literal string is passed
    // straight to the CLI regardless of the published type.
    opts.effort = input.effort as unknown as NonNullable<Options["effort"]>;
  }
  if (input.permissionMode !== undefined) opts.permissionMode = input.permissionMode;
  if (input.permissionMode === "bypassPermissions") {
    opts.allowDangerouslySkipPermissions =
      input.allowDangerouslySkipPermissions ?? true;
  }
  if (input.settings && Object.keys(input.settings).length > 0) {
    opts.settings = input.settings as unknown as NonNullable<Options["settings"]>;
  }
  if (input.resume !== undefined) opts.resume = input.resume;
  if (input.sessionId !== undefined) opts.sessionId = input.sessionId;
  if (input.additionalDirectories && input.additionalDirectories.length > 0) {
    opts.additionalDirectories = [...input.additionalDirectories];
  }
  if (input.extraArgs && Object.keys(input.extraArgs).length > 0) {
    opts.extraArgs = { ...input.extraArgs };
  }

  return opts as Options;
}

// ---------------------------------------------------------------------------
// ClaudeSession
// ---------------------------------------------------------------------------

/** A single live SDK session bound to one runtime thread. */
export class ClaudeSession {
  readonly threadId: string;
  private readonly promptQueue: AsyncPromptQueue<SDKUserMessage>;
  private readonly query: Query;
  private readonly emitter: EventEmitter;
  private readonly pendingApprovals: PendingApprovals;
  private closed = false;
  private iterationTask: Promise<void>;

  constructor(input: SessionStartInput, emit: EventEmitter) {
    this.threadId = input.threadId;
    this.emitter = emit;
    this.promptQueue = new AsyncPromptQueue<SDKUserMessage>();
    this.pendingApprovals = new Map();

    const canUseTool = makeCanUseTool(
      input.threadId,
      emit,
      this.pendingApprovals,
    );
    const options = buildQueryOptions(input, canUseTool);

    this.query = queryFactory({
      prompt: this.promptQueue,
      options,
    });

    // Announce the session to subscribers up front.
    emit.notification("session-configured", {
      threadId: this.threadId,
      pathToClaudeCodeExecutable: input.pathToClaudeCodeExecutable,
      cwd: input.cwd,
    });

    this.iterationTask = this.consumeMessages();
  }

  /** Drive the SDK's async iterator, forwarding every message to the
   *  RPC layer. Unknown / not-yet-enumerated variants are forwarded
   *  raw; classification lives on the Rust side. */
  private async consumeMessages(): Promise<void> {
    try {
      for await (const message of this.query) {
        if (this.closed) break;
        this.emitter.notification("sdk-message", {
          threadId: this.threadId,
          message,
        });
        this.emitSpecialCases(message);
      }
      if (!this.closed) {
        this.emitter.notification("session-ended", {
          threadId: this.threadId,
          reason: "iteration-complete",
        });
      }
    } catch (err) {
      if (isAbortLikeError(err)) {
        this.emitter.notification("session-ended", {
          threadId: this.threadId,
          reason: "interrupted",
        });
      } else {
        this.emitter.notification("session-error", {
          threadId: this.threadId,
          error: {
            message: err instanceof Error ? err.message : String(err),
            stack: err instanceof Error ? err.stack ?? null : null,
          },
        });
      }
    }
  }

  /** Emit side-channel notifications for the two tool uses that
   *  production integrations treat specially (research §5f). The raw
   *  `sdk-message` is still sent alongside so consumers never lose
   *  context. */
  private emitSpecialCases(message: SDKMessage): void {
    if (message.type !== "assistant") return;
    const content = (message as unknown as {
      message?: { content?: unknown[] };
    }).message?.content;
    if (!Array.isArray(content)) return;
    for (const block of content) {
      if (
        typeof block !== "object" ||
        block === null ||
        (block as { type?: string }).type !== "tool_use"
      ) {
        continue;
      }
      const toolUse = block as {
        type: string;
        name?: string;
        id?: string;
        input?: unknown;
      };
      if (toolUse.name === "AskUserQuestion") {
        this.emitter.notification("user-input-requested", {
          threadId: this.threadId,
          toolUseId: toolUse.id ?? null,
          input: toolUse.input ?? null,
        });
      } else if (toolUse.name === "ExitPlanMode") {
        this.emitter.notification("plan-proposed", {
          threadId: this.threadId,
          toolUseId: toolUse.id ?? null,
          plan:
            (toolUse.input as { plan?: unknown } | undefined)?.plan ?? null,
          input: toolUse.input ?? null,
        });
      }
    }
  }

  /** Enqueue a user turn on the prompt queue. Rejects if the session
   *  has already been closed. */
  async sendTurn(input: SendTurnInput): Promise<void> {
    if (this.closed) {
      throw new Error("session is closed");
    }
    // If a per-turn model override was supplied, apply it before the
    // turn is dispatched. The SDK applies `setModel` to subsequent
    // messages, not retroactively, which matches what we want.
    if (input.modelOverride !== undefined) {
      try {
        await this.query.setModel(input.modelOverride);
      } catch (err) {
        logger.warn("setModel on turn-override failed", {
          threadId: this.threadId,
          err: err instanceof Error ? err.message : String(err),
        });
      }
    }
    const msg: SDKUserMessage = {
      type: "user",
      message: {
        role: "user",
        content: input.text,
      },
      parent_tool_use_id: null,
    };
    this.promptQueue.push(msg);
  }

  /** Halt the current turn. The SDK emits no final `result` for an
   *  interrupted turn; our message-iteration loop surfaces a
   *  `session-ended` notification with `reason: "interrupted"`. */
  async interrupt(): Promise<void> {
    if (this.closed) return;
    await this.query.interrupt();
  }

  /** Swap the session's default model. `undefined` reverts to the
   *  SDK default. */
  async setModel(model: string | undefined): Promise<void> {
    if (this.closed) {
      throw new Error("session is closed");
    }
    await this.query.setModel(model);
  }

  /** Change the permission mode on the live session. */
  async setPermissionMode(mode: PermissionMode): Promise<void> {
    if (this.closed) {
      throw new Error("session is closed");
    }
    await this.query.setPermissionMode(mode);
  }

  /** Resolve a pending approval request with the user's decision.
   *  Throws if `requestId` is not currently awaiting a decision. */
  async respondToRequest(
    requestId: string,
    decision: ApprovalDecision,
  ): Promise<void> {
    const resolver = this.pendingApprovals.get(requestId);
    if (!resolver) {
      throw new Error(`request ${requestId} not found or already resolved`);
    }
    this.pendingApprovals.delete(requestId);
    resolver(decision);
  }

  /** Hook for the AskUserQuestion flow. Treats `answers` as an
   *  allow-with-updatedInput approval — the eventual full UX will
   *  differentiate but this is the shape the SDK already expects. */
  async respondToUserInput(
    requestId: string,
    answers: unknown[],
  ): Promise<void> {
    await this.respondToRequest(requestId, {
      behavior: "allow",
      updatedInput: { answers },
    });
  }

  /** Fetch the SDK's cached init response. Typed as `unknown` on the
   *  way out so the Rust side can classify. */
  async initializationResult(): Promise<unknown> {
    if (this.closed) {
      throw new Error("session is closed");
    }
    return this.query.initializationResult();
  }

  /** Close the session: stop the iteration loop, close the prompt
   *  queue, tear down the SDK subprocess, reject pending approvals. */
  async close(): Promise<void> {
    if (this.closed) return;
    this.closed = true;

    // Reject any pending tool approvals so they don't dangle.
    for (const [, resolver] of this.pendingApprovals) {
      resolver({
        behavior: "deny",
        message: "session closed",
        interrupt: true,
      });
    }
    this.pendingApprovals.clear();

    // Stop accepting new user turns. The iterator may still yield
    // buffered messages; that's fine.
    this.promptQueue.close();

    // `close()` on the SDK's Query is sync and tears down the
    // subprocess. After this, the iterator will terminate.
    try {
      this.query.close();
    } catch (err) {
      logger.warn("query.close threw", {
        threadId: this.threadId,
        err: err instanceof Error ? err.message : String(err),
      });
    }

    // Wait for iterationTask so `session-ended` is emitted before we
    // return. Don't propagate its error — already surfaced via
    // session-error.
    try {
      await this.iterationTask;
    } catch {
      // swallowed — surfaced via session-error
    }
  }

  /** For tests: returns true after `close()`. */
  isClosed(): boolean {
    return this.closed;
  }
}
