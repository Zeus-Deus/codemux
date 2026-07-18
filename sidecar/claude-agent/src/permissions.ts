// canUseTool bridge — converts the SDK's synchronous per-call
// permission callback into an async RPC round-trip back to the Rust
// side.
//
// When the SDK decides to call a tool, it invokes
// `canUseTool(toolName, toolInput, {signal, ...})` and awaits a
// `PermissionResult`. We generate a `requestId`, emit a
// `request-opened` notification, and park the callback on a promise.
// The Rust side later responds via the `respond-to-request` RPC,
// which resolves the promise.
//
// `ExitPlanMode` is denied immediately with a stop-now message (the
// research report §5f confirmed this is how production integrations
// handle it). A separate `plan-proposed` notification surfaces the
// proposed plan content so the UI can display it.

import { randomUUID } from "node:crypto";

import type {
  CanUseTool,
  PermissionResult,
  PermissionUpdate,
} from "@anthropic-ai/claude-agent-sdk";

import { logger } from "./logger.ts";

/** Decisions the Rust side can hand back. Modelled on the SDK's
 *  `PermissionResult` but shaped for JSON transport. */
export type ApprovalDecision =
  | {
      behavior: "allow";
      /** Optional edited tool input to apply instead of the original. */
      updatedInput?: unknown;
      /** Optional "always allow" permission updates. */
      updatedPermissions?: unknown;
    }
  | {
      behavior: "deny";
      /** Reason shown to the model. */
      message: string;
      /** If true, also halt the current turn. */
      interrupt?: boolean;
    };

/** Resolver function stored while a single request is awaiting its
 *  user decision. */
export type PendingApprovalResolver = (decision: ApprovalDecision) => void;

/** Map from request id to resolver. Owned by a `ClaudeSession`. */
export type PendingApprovals = Map<string, PendingApprovalResolver>;

/** Slim notification-only surface the permission bridge needs. The
 *  full event-emitter interface extends this in `session.ts`. */
export interface PermissionsEmitter {
  notification(method: string, params: unknown): void;
}

/** Coarse classification of a tool invocation, matching the
 *  request-kinds the Rust side already understands from the Codex
 *  adapter (`command` / `file-change` / `file-read` / `other`). */
export function classifyToolKind(
  toolName: string,
  _toolInput: Record<string, unknown>,
): string {
  // Specialized surfaces the UI renders as structured forms, not
  // generic allow/deny prompts. AskUserQuestion is the SDK's
  // interactive-clarification tool — the runtime ships a dedicated
  // form for it, so we tag the request kind up front rather than
  // leaving it to downstream fuzzy matching.
  if (toolName === "AskUserQuestion") return "user-input";

  const n = toolName.toLowerCase();
  if (n === "bash" || n.includes("shell") || n.includes("command")) {
    return "command";
  }
  if (n === "read" || n === "fileread" || n.includes("read")) {
    return "file-read";
  }
  if (
    n === "edit" ||
    n === "write" ||
    n === "filechange" ||
    n.includes("edit") ||
    n.includes("write")
  ) {
    return "file-change";
  }
  return "other";
}

/** Extra context the bridge consults to detect a CLI bypass downgrade.
 *  Supplied by `ClaudeSession`; optional so standalone/legacy callers
 *  keep the plain prompt-every-call behavior. */
export interface CanUseToolContext {
  /** Current INTENDED permission mode for the session, kept current by
   *  `setPermissionMode`. When this reads `"bypassPermissions"` the
   *  user has explicitly chosen Full access. */
  getPermissionMode: () => string | undefined;
  /** Best-effort one-shot restore of real bypass on the live query,
   *  invoked when a downgrade is detected. */
  onBypassDowngradeDetected?: () => void;
}

/**
 * Build a `CanUseTool` callback suitable for the SDK's `Options.canUseTool`.
 *
 * @param threadId  - runtime-owned thread id, included on every event
 * @param emit      - event emitter that forwards notifications to stdout
 * @param pending   - the session's map of parked resolvers
 * @param context   - optional hooks for bypass-downgrade detection
 */
export function makeCanUseTool(
  threadId: string,
  emit: PermissionsEmitter,
  pending: PendingApprovals,
  context?: CanUseToolContext,
): CanUseTool {
  return async (
    toolName,
    toolInput,
    callbackOptions,
  ): Promise<PermissionResult> => {
    const requestId = randomUUID();

    // ExitPlanMode is denied immediately; the proposed plan is
    // surfaced out-of-band via `plan-proposed`. User acceptance of a
    // plan starts a fresh turn — there is no "allow" path here.
    if (toolName === "ExitPlanMode") {
      emit.notification("plan-proposed", {
        threadId,
        requestId,
        toolUseId: callbackOptions.toolUseID,
        plan:
          (toolInput as { plan?: unknown } | undefined)?.plan ?? null,
        input: toolInput,
      });
      return {
        behavior: "deny",
        message:
          "Plan captured. Stop here and wait for the user's decision.",
        interrupt: true,
      } satisfies PermissionResult;
    }

    // ── Bypass-downgrade guard ──────────────────────────────────────
    // When the user picks Full access the session launches with
    // `permissionMode: bypassPermissions`, under which the CLI
    // short-circuits permission evaluation and NEVER calls this
    // callback. So if we are invoked while the session's intended mode
    // is still `bypassPermissions`, the launch was silently downgraded:
    // the CLI re-checks its danger-mode consent state at process boot,
    // and when that read transiently fails or looks absent — which
    // happens when concurrent sessions are rewriting the shared
    // settings/consent files at the same moment — it drops
    // bypassPermissions and boots in `default` mode, warning only on
    // stderr. The user still sees "Full access" in the composer, so a
    // permission prompt here would violate their explicit choice.
    //
    // Honor that choice: auto-allow the call with its original input
    // (no `request-opened`, no parked pending entry) and ask the
    // session to attempt a one-shot live restore of real bypass.
    //
    // `AskUserQuestion` is exempt: it is a clarification form, not a
    // permission gate. Auto-allowing it would hand the model an
    // unanswered question, so it must keep flowing through the
    // interactive prompt path even under intended bypass.
    if (
      toolName !== "AskUserQuestion" &&
      context?.getPermissionMode() === "bypassPermissions"
    ) {
      context.onBypassDowngradeDetected?.();
      logger.warn(
        "canUseTool fired under intended bypassPermissions — CLI silently downgraded the session to default (consent-read race); auto-allowing to honor Full access",
        { threadId, toolName },
      );
      return {
        behavior: "allow",
        updatedInput: toolInput as Record<string, unknown>,
      } satisfies PermissionResult;
    }

    const decisionPromise = new Promise<ApprovalDecision>(
      (resolve, reject) => {
        pending.set(requestId, resolve);
        const onAbort = (): void => {
          if (!pending.has(requestId)) return;
          pending.delete(requestId);
          reject(new Error("tool approval aborted"));
        };
        callbackOptions.signal.addEventListener("abort", onAbort, {
          once: true,
        });
      },
    );

    emit.notification("request-opened", {
      threadId,
      requestId,
      toolName,
      toolInput,
      toolUseId: callbackOptions.toolUseID,
      kind: classifyToolKind(toolName, toolInput),
      // Passthrough SDK-provided hints — the UI can display them and
      // the adapter can echo any `suggestions` back as
      // `updatedPermissions` on a "always allow" decision.
      title: callbackOptions.title ?? null,
      displayName: callbackOptions.displayName ?? null,
      description: callbackOptions.description ?? null,
      suggestions: (callbackOptions.suggestions ?? []) as PermissionUpdate[],
      agentId: callbackOptions.agentID ?? null,
    });

    let decision: ApprovalDecision;
    try {
      decision = await decisionPromise;
    } catch {
      // Signal aborted before user responded — the SDK treats this as
      // a deny. Abort handler already removed the local pending entry;
      // mirror a `request-resolved` back so the host clears its own
      // pending-approvals map too. Without it the host keeps a stale
      // entry after an interrupt and its dispatch queue wedges forever.
      emit.notification("request-resolved", {
        threadId,
        requestId,
        decision: {
          behavior: "deny",
          message: "Tool request was aborted.",
        },
      });
      return {
        behavior: "deny",
        message: "Tool request was aborted.",
      } satisfies PermissionResult;
    }
    // Clean up the pending entry regardless of who resolved it. The
    // `respondToRequest` RPC path also deletes, but it is cheap and
    // safe to do here so direct resolver callers (tests, or a future
    // caller that doesn't go through the RPC) don't leak entries.
    pending.delete(requestId);

    emit.notification("request-resolved", {
      threadId,
      requestId,
      decision,
    });

    if (decision.behavior === "allow") {
      const result: PermissionResult = {
        behavior: "allow",
        updatedInput: (decision.updatedInput ?? toolInput) as Record<
          string,
          unknown
        >,
      };
      if (decision.updatedPermissions !== undefined) {
        result.updatedPermissions =
          decision.updatedPermissions as PermissionUpdate[];
      }
      return result;
    }

    const denyResult: PermissionResult = {
      behavior: "deny",
      message: decision.message,
    };
    if (decision.interrupt) denyResult.interrupt = true;
    return denyResult;
  };
}
