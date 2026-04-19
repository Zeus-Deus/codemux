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

/**
 * Build a `CanUseTool` callback suitable for the SDK's `Options.canUseTool`.
 *
 * @param threadId  - runtime-owned thread id, included on every event
 * @param emit      - event emitter that forwards notifications to stdout
 * @param pending   - the session's map of parked resolvers
 */
export function makeCanUseTool(
  threadId: string,
  emit: PermissionsEmitter,
  pending: PendingApprovals,
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
      // a deny. Abort handler already removed the pending entry.
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
