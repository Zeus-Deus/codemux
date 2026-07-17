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
  buildCodemuxMcpServer,
  type RegisteredMcpTool,
} from "./mcp-bridge.ts";
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
  /** Stage 3 — tools from Codemux's MCP runtime registry to register
   *  with the SDK as the in-process `codemux` MCP server. Each
   *  tool's prefixed name (`mcp__<server>__<tool>`) is what the
   *  agent sees and what permission rules key on. */
  mcpTools?: RegisteredMcpTool[];
}

/** Parameters for `send-turn`. */
export interface SendTurnInput {
  /** User text. */
  text: string;
  /** Per-turn model override (falls back to the session default). */
  modelOverride?: string;
  /** Inline image attachments. Bytes arrive base64-encoded so the
   *  JSON-RPC frame stays text-only; this matches what the Anthropic
   *  SDK consumes for `image/base64` content blocks. */
  images?: Array<{ mediaType: string; dataBase64: string }>;
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

/** How long `interrupt()` waits for the SDK query's iterator to exit
 *  before assuming the interrupt was treated as soft. Real SDK aborts
 *  land in single-digit milliseconds; the ceiling only guards against
 *  a hung subprocess. */
let interruptExitTimeoutMs = 2000;

/** Lower the interrupt-exit timeout so tests don't wait on the real
 *  ceiling. Mirrors the `setQueryFactoryForTests` DI seam. */
export function setInterruptExitTimeoutForTests(ms: number): void {
  interruptExitTimeoutMs = ms;
}

/** Restore the default interrupt-exit timeout. Call from test teardown. */
export function resetInterruptExitTimeoutForTests(): void {
  interruptExitTimeoutMs = 2000;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Prepend "Ultrathink:\n" to `text` when `effort === "ultrathink"`,
 *  idempotently. Port of `applyClaudePromptEffortPrefix` from a
 *  reference multi-provider client
 *  (packages/shared/src/model.ts:285). The canonical prepend lives
 *  client-side; this copy exists as a defensive belt-and-braces layer
 *  in case a caller bypasses the client and writes the effort field
 *  directly in `start-session`. Idempotency keeps the double-fire
 *  safe. */
function applyClaudePromptEffortPrefix(text: string, effort: string | undefined): string {
  const trimmed = text.trim();
  if (!trimmed) return trimmed;
  if (effort !== "ultrathink") return trimmed;
  if (trimmed.startsWith("Ultrathink:")) return trimmed;
  return `Ultrathink:\n${trimmed}`;
}

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
    // Ask the SDK to emit human-readable `summary` fields on
    // `task_progress` events so the subagent card's activity line can
    // show "currently doing X" instead of only a tool name. Subagent
    // view (Stage 1). Everything else in the subagent pipeline is
    // Rust-side.
    agentProgressSummaries: true,
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

  // Stage 3 — register Codemux's in-process MCP facade so every
  // user-installed MCP's tools surface to this Claude session via a
  // single virtual server. Tool calls are dispatched back to Rust
  // via the `mcp-tool-call` upstream RPC; see `mcp-bridge.ts`.
  const mcpTools = input.mcpTools ?? [];
  if (mcpTools.length > 0) {
    opts.mcpServers = {
      codemux: buildCodemuxMcpServer(mcpTools),
    };
  }

  return opts as Options;
}

// ---------------------------------------------------------------------------
// ClaudeSession
// ---------------------------------------------------------------------------

/** A single live SDK session bound to one runtime thread. */
export class ClaudeSession {
  readonly threadId: string;
  /** Rebuilt by `ensureLiveQuery` after an interrupt, so mutable. */
  private promptQueue: AsyncPromptQueue<SDKUserMessage>;
  /** Rebuilt by `ensureLiveQuery` after an interrupt, so mutable. */
  private query: Query;
  private readonly emitter: EventEmitter;
  private readonly pendingApprovals: PendingApprovals;
  private closed = false;
  private iterationTask: Promise<void>;
  /** Full start input, retained so `ensureLiveQuery` can rebuild a
   *  resumed query after an interrupt kills the original. Mutable
   *  session settings (`model`, `permissionMode`, `mcpTools`) are
   *  recorded here as they change so a rebuild keeps fidelity. */
  private readonly startInput: SessionStartInput;
  /** The `canUseTool` bridge, retained so a rebuilt query reuses the
   *  same closure over `pendingApprovals` and the emitter. */
  private readonly canUseTool: CanUseTool;
  /** True while `interrupt()` is in flight, so `consumeMessages` knows
   *  an iterator exit is expected and stays silent instead of firing
   *  `session-ended`. */
  private interrupting = false;
  /** True once an interrupt has killed the SDK query. The next
   *  `sendTurn` rebuilds a resumed query via `ensureLiveQuery`. */
  private queryDead = false;
  /** SDK-assigned session id, observed from the first incoming SDK
   *  message that carries `session_id`. Forwarded to the Rust side
   *  as a `sdk-session-id` notification so restarts can resume from
   *  this session. The reference impl uses the same
   *  `context.resumeSessionId = message.session_id` pattern
   *  (ClaudeAdapter.ts:1255). */
  private sdkSessionId: string | null = null;
  /** The most recent user turn, retained so a stale-resume self-heal
   *  can replay it onto a freshly-rebuilt query. Reset per turn is not
   *  needed — it always holds the latest turn. */
  private lastUserMessage: SDKUserMessage | null = null;
  /** Guards the stale-resume recovery to one attempt per user turn.
   *  Reset to false at the start of each `sendTurn` so every new turn
   *  earns a fresh recovery chance; set true once recovery fires so a
   *  retry that fails the same way surfaces normally instead of looping. */
  private resumeFallbackAttempted = false;
  /** True while a stale-resume rebuild is in flight, so the OLD
   *  `consumeMessages` loop unwinds silently (no `session-ended` /
   *  `session-error`) without clobbering the NEW query's state. Mirrors
   *  the `interrupting` handling. */
  private recoveringResume = false;
  /** Session-level effort, captured at start. Used by `sendTurn` to
   *  apply the ultrathink prompt-prepend defensively. The canonical
   *  prepend lives in the frontend; this one fires only when a caller
   *  bypasses it and writes `effort: "ultrathink"` directly. */
  private readonly effort: string | undefined;

  constructor(input: SessionStartInput, emit: EventEmitter) {
    this.threadId = input.threadId;
    this.emitter = emit;
    this.effort = input.effort;
    // Shallow copy so later `setModel`/`setPermissionMode`/`updateMcpTools`
    // recordings don't mutate the caller's object; copy `mcpTools` too
    // since it's the one array field a rebuild reads back.
    this.startInput = { ...input };
    if (input.mcpTools) {
      this.startInput.mcpTools = [...input.mcpTools];
    }
    this.promptQueue = new AsyncPromptQueue<SDKUserMessage>();
    this.pendingApprovals = new Map();

    this.canUseTool = makeCanUseTool(
      input.threadId,
      emit,
      this.pendingApprovals,
    );
    const options = buildQueryOptions(input, this.canUseTool);

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
        // Stale-resume self-heal: the SDK reported that the JSONL for
        // the session we asked to resume is gone from disk. Rather than
        // let the error surface (which permanently wedges the thread),
        // suppress it, rebuild a FRESH session, and replay the pending
        // turn if one exists. The CLI validates `--resume` EAGERLY, so
        // this error can arrive right after session start with no turn
        // in flight yet — recovery must not depend on `lastUserMessage`
        // (the replay inside `recoverFromStaleResume` is conditional).
        // One attempt per turn/eager-start; the error result is never
        // forwarded to the host while a recovery is possible.
        if (
          this.isStaleResumeResult(message) &&
          !this.resumeFallbackAttempted &&
          !this.closed
        ) {
          this.recoverFromStaleResume();
          break;
        }
        this.observeSdkSessionId(message);
        this.emitter.notification("sdk-message", {
          threadId: this.threadId,
          message,
        });
        this.emitSpecialCases(message);
      }
      // A stale-resume recovery rebuilt the query on a fresh iteration
      // task; this OLD loop must unwind without emitting anything and
      // without touching the NEW query's state.
      if (this.recoveringResume) {
        this.recoveringResume = false;
        return;
      }
      // An interrupt makes the SDK iterator exit — sometimes cleanly
      // like this, sometimes via an abort-like throw below. Mark the
      // query dead so the next `sendTurn` rebuilds a resumed query, and
      // stay silent: `interrupt()` fires the `turn-interrupted` event.
      if (this.interrupting && !this.closed) {
        this.queryDead = true;
        return;
      }
      if (!this.closed) {
        this.emitter.notification("session-ended", {
          threadId: this.threadId,
          reason: "iteration-complete",
        });
      }
    } catch (err) {
      // Stale-resume can also surface as a thrown error from the
      // iterator rather than a `result` message. Same self-heal.
      if (
        !this.resumeFallbackAttempted &&
        !this.closed &&
        !this.interrupting &&
        this.isStaleResumeError(err)
      ) {
        this.recoverFromStaleResume();
        this.recoveringResume = false;
        return;
      }
      // Breaking out of the loop after recovery can make the old
      // iterator's teardown reject; stay silent, the NEW loop is live.
      if (this.recoveringResume) {
        this.recoveringResume = false;
        return;
      }
      // Interrupt-driven abort: same silent, mark-dead handling as the
      // clean-exit path above.
      if (this.interrupting && !this.closed && isAbortLikeError(err)) {
        this.queryDead = true;
        return;
      }
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

  /** Watch incoming SDK messages for a `session_id` field. Emit a
   *  one-shot `sdk-session-id` notification the first time one is
   *  observed so the Rust side can store it as a resume cursor. */
  private observeSdkSessionId(message: SDKMessage): void {
    if (this.sdkSessionId !== null) return;
    const maybeId = (message as unknown as { session_id?: unknown }).session_id;
    if (typeof maybeId !== "string" || maybeId.length === 0) return;
    this.sdkSessionId = maybeId;
    this.emitter.notification("sdk-session-id", {
      threadId: this.threadId,
      sessionId: maybeId,
    });
  }

  /** Detect the SDK's "the session I asked to resume no longer exists"
   *  failure: a `result` message whose `subtype` is an error variant
   *  and whose payload names the missing session. The `errors` array of
   *  strings is the primary signal; `JSON.stringify` is a defensive
   *  fallback for SDK versions that bury the detail elsewhere. */
  private isStaleResumeResult(message: SDKMessage): boolean {
    const m = message as unknown as {
      type?: unknown;
      subtype?: unknown;
      errors?: unknown;
    };
    if (m.type !== "result") return false;
    if (typeof m.subtype !== "string" || !m.subtype.startsWith("error")) {
      return false;
    }
    const needle = "No conversation found with session ID";
    if (
      Array.isArray(m.errors) &&
      m.errors.some((e) => typeof e === "string" && e.includes(needle))
    ) {
      return true;
    }
    try {
      return JSON.stringify(message).includes(needle);
    } catch {
      return false;
    }
  }

  /** String-match variant of the stale-resume signal for the throw
   *  path, where the failure arrives as a thrown error. */
  private isStaleResumeError(err: unknown): boolean {
    const message = err instanceof Error ? err.message : String(err);
    return message.includes("No conversation found with session ID");
  }

  /** Self-heal a stale resume: announce the fallback, drop every scrap
   *  of resume state, rebuild a FRESH (non-resumed) query, and replay
   *  the pending user turn so the send transparently retries. Marks the
   *  attempt so a same-turn repeat surfaces normally instead of looping,
   *  and sets `recoveringResume` so the OLD `consumeMessages` loop
   *  unwinds silently. */
  private recoverFromStaleResume(): void {
    this.resumeFallbackAttempted = true;
    this.recoveringResume = true;
    // The resume id that was in effect for the failed attempt — surfaced
    // so the host can drop it from its persisted cursor.
    const staleSessionId =
      this.sdkSessionId ?? this.startInput.resume ?? null;
    this.emitter.notification("resume-fallback", {
      threadId: this.threadId,
      staleSessionId,
    });
    // Clear all resume state so the rebuild starts a brand-new session.
    this.sdkSessionId = null;
    delete this.startInput.resume;
    delete this.startInput.sessionId;
    // Rebuild without any resume, then re-push the user's turn onto the
    // new prompt queue so it retries against the fresh session.
    this.rebuildQuery({ resume: undefined });
    if (this.lastUserMessage !== null) {
      this.promptQueue.push(this.lastUserMessage);
    }
  }

  /** Emit a side-channel notification for `ExitPlanMode`, which
   *  permissions.ts denies before a `request-opened` ever fires —
   *  so without this extra hop the UI would never learn the plan
   *  text. `AskUserQuestion` used to ride the same side-channel but
   *  was removed in Stage 1 once canUseTool's `request-opened`
   *  started carrying `kind: "user-input"` — the two emissions
   *  previously produced duplicate cards with different request ids.
   */
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
      if (toolUse.name === "ExitPlanMode") {
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
    // If a prior interrupt killed the query, transparently rebuild a
    // resumed one before enqueuing this turn.
    this.ensureLiveQuery();
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
    const preparedText = applyClaudePromptEffortPrefix(input.text, this.effort);
    // Stage 6 — when images are attached we build a multi-block
    // content array per Anthropic's vision spec: image blocks come
    // BEFORE the text block ("describe these images" pattern). The
    // text-only fast path keeps the string-content shape so existing
    // unit tests that match on `content === text` stay green.
    const images = input.images ?? [];
    const content =
      images.length === 0
        ? preparedText
        : [
            ...images.map((img) => ({
              type: "image" as const,
              source: {
                type: "base64" as const,
                media_type: img.mediaType,
                data: img.dataBase64,
              },
            })),
            { type: "text" as const, text: preparedText },
          ];
    const msg: SDKUserMessage = {
      type: "user",
      message: {
        role: "user",
        content: content as SDKUserMessage["message"]["content"],
      },
      parent_tool_use_id: null,
    };
    // Retain the turn and grant it one stale-resume recovery attempt
    // before it's dispatched, so a self-heal can replay it verbatim.
    this.lastUserMessage = msg;
    this.resumeFallbackAttempted = false;
    this.promptQueue.push(msg);
  }

  /** Halt the current turn. `query.interrupt()` makes the SDK query's
   *  iterator exit, so the query is marked dead and the next
   *  `sendTurn` rebuilds a resumed query. A `turn-interrupted`
   *  notification (not `session-ended`) tells the host the turn is
   *  over but the session lives. */
  async interrupt(): Promise<void> {
    if (this.closed || this.queryDead) return;
    this.interrupting = true;
    try {
      await this.query.interrupt();
      // Wait for `consumeMessages` to observe the iterator exit (which
      // flips `queryDead`), bounded by a timeout in case the SDK
      // treated the interrupt as soft and kept iterating.
      let timer: ReturnType<typeof setTimeout> | undefined;
      await Promise.race([
        this.iterationTask,
        new Promise<void>((resolve) => {
          timer = setTimeout(resolve, interruptExitTimeoutMs);
        }),
      ]);
      if (timer !== undefined) clearTimeout(timer);
      if (this.queryDead && !this.closed) {
        this.emitter.notification("turn-interrupted", {
          threadId: this.threadId,
        });
      }
      // If the timeout won (iterator still alive), do nothing extra:
      // the `finally` reset restores old behavior for a later exit.
    } finally {
      this.interrupting = false;
    }
  }

  /** Rebuild the SDK query after an interrupt marked it dead. No-op
   *  while the query is live or the session is closed. The rebuilt
   *  query resumes the SDK session so history is preserved; the SDK
   *  issues a fresh session id which `observeSdkSessionId` re-emits so
   *  the host updates its resume cursor. Mirrors the reference impl's
   *  `ensureSessionForThread`. */
  private ensureLiveQuery(): void {
    if (!this.queryDead || this.closed) return;

    const resume = this.sdkSessionId ?? this.startInput.resume;
    // A resumed session is issued a NEW session id by the SDK; clear
    // the observed id so `observeSdkSessionId` re-emits it.
    this.sdkSessionId = null;

    this.rebuildQuery({ resume });
  }

  /** Tear down the current query and stand up a replacement with the
   *  given resume disposition, wiring a fresh prompt queue and iteration
   *  task. Shared by `ensureLiveQuery` (resume-preserving, after an
   *  interrupt) and the stale-resume self-heal (`resume: undefined`, a
   *  fresh session). Callers own resume-cursor bookkeeping
   *  (`sdkSessionId`, `startInput.resume`) before calling. */
  private rebuildQuery(opts: { resume: string | undefined }): void {
    // Best-effort reap of the dead subprocess.
    try {
      this.query.close();
    } catch (err) {
      logger.warn("query.close during rebuild threw", {
        threadId: this.threadId,
        err: err instanceof Error ? err.message : String(err),
      });
    }

    this.queryDead = false;
    this.promptQueue = new AsyncPromptQueue<SDKUserMessage>();
    // Drop the explicit `sessionId` — that uuid named the ORIGINAL
    // fresh session; the rebuild resumes (or starts fresh) instead.
    const rebuildInput: SessionStartInput = { ...this.startInput };
    delete rebuildInput.sessionId;
    if (opts.resume === undefined) {
      delete rebuildInput.resume;
    } else {
      rebuildInput.resume = opts.resume;
    }
    const options = buildQueryOptions(rebuildInput, this.canUseTool);
    this.query = queryFactory({ prompt: this.promptQueue, options });
    this.iterationTask = this.consumeMessages();
  }

  /** Stage 4 — push an updated MCP tool list to the live SDK
   *  session. The Rust runtime calls this whenever a server
   *  transitions Discovered → Running so tools that came up after
   *  `start-session` become visible without forcing a chat restart.
   *
   *  Empty `tools` removes the codemux MCP entirely; the SDK
   *  silently ignores requests for unregistered tool names so this
   *  is the right shape for "user disabled all MCPs". Idempotent. */
  async updateMcpTools(tools: RegisteredMcpTool[]): Promise<void> {
    if (this.closed) return;
    // Record first so a rebuild after an interrupt keeps the latest
    // tool list even if the live query call below throws / is skipped.
    this.startInput.mcpTools = tools;
    if (this.queryDead) return;
    const servers: Record<string, ReturnType<typeof buildCodemuxMcpServer>> =
      tools.length > 0 ? { codemux: buildCodemuxMcpServer(tools) } : {};
    try {
      await this.query.setMcpServers(
        servers as unknown as Parameters<Query["setMcpServers"]>[0],
      );
    } catch (err) {
      logger.warn("setMcpServers failed", {
        threadId: this.threadId,
        toolCount: tools.length,
        err: err instanceof Error ? err.message : String(err),
      });
    }
  }

  /** Swap the session's default model. `undefined` reverts to the
   *  SDK default. */
  async setModel(model: string | undefined): Promise<void> {
    if (this.closed) {
      throw new Error("session is closed");
    }
    // Record first so a rebuild after an interrupt keeps the latest
    // model even if the live query call below throws / is skipped.
    // `undefined` reverts to the SDK default, so drop the recorded key.
    if (model === undefined) {
      delete this.startInput.model;
    } else {
      this.startInput.model = model;
    }
    if (this.queryDead) return;
    await this.query.setModel(model);
  }

  /** Change the permission mode on the live session. */
  async setPermissionMode(mode: PermissionMode): Promise<void> {
    if (this.closed) {
      throw new Error("session is closed");
    }
    this.startInput.permissionMode = mode;
    if (this.queryDead) return;
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
