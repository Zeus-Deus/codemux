import type { OrbActivity } from "@/lib/orb-state";

import { isRunning } from "./subagents";
import type { ChatViewItem, SubagentView, ToolCallItem } from "./types";

/**
 * Adapters that read the live "what is happening" signal out of a
 * transcript and hand it to the shared mapper in `src/lib/orb-state.ts`.
 *
 * Nothing new is plumbed through the backend for this: the reducer already
 * stamps every tool call with its name, input, and status, which is exactly
 * the signal the orb needs. Keeping the derivation here (rather than in the
 * components) means the transcript's turn marker, the subagent rows, and
 * the docked composer strip all read the same tail the same way.
 */

/** `ToolCallItem.input` is `unknown` on the wire; narrow it for the mapper. */
function toolInputRecord(input: unknown): Record<string, unknown> | null {
  return input !== null && typeof input === "object" && !Array.isArray(input)
    ? (input as Record<string, unknown>)
    : null;
}

/** Most recent tool call in a list of transcript items, newest first. */
function lastToolCalls(
  items: readonly ChatViewItem[],
  limit: number,
): ToolCallItem[] {
  const out: ToolCallItem[] = [];
  for (let i = items.length - 1; i >= 0 && out.length < limit; i--) {
    const item = items[i];
    if (item.kind === "tool_call") out.push(item);
  }
  return out;
}

/**
 * Whether the agent looks like it is having another go after a failure.
 *
 * There is no retry counter anywhere in the event model, so this is
 * inferred from the shape the transcript actually takes: a tool is running
 * now, and the tool call immediately before it errored. That is what a
 * retry looks like from the outside, and it is the same evidence a reader
 * would use scrolling the thread.
 */
function looksLikeRetry(recent: ToolCallItem[]): boolean {
  const [current, previous] = recent;
  if (!current || current.status !== "running") return false;
  return previous?.status === "error";
}

/**
 * Orb signal for a thread's in-flight turn, from either the whole
 * transcript or one Activity block's steps — both are just item lists whose
 * tail describes the present moment.
 *
 * Two surfaces call this, and they are mutually exclusive by construction:
 * the Activity block's working header owns the turn while a tool is
 * running, and `shouldShowThinkingIndicator` only lets the transcript-tail
 * marker appear once the tail is no longer a running tool. So the thread
 * shows exactly one orb at a time.
 *
 * Neither is reachable while an approval is pending — a parked turn renders
 * the approval UI — so there is no `awaitingUser` case to handle here.
 */
export function turnOrbActivity(messages: readonly ChatViewItem[]): OrbActivity {
  const recent = lastToolCalls(messages, 2);
  const current = recent[0];
  if (looksLikeRetry(recent)) return { retrying: true };
  // Only a *running* tool describes the present moment. Once it finishes,
  // the agent is composing its reply, and the neutral working orb is more
  // honest than leaving the last tool's animation up.
  if (current?.status === "running") {
    return {
      toolName: current.tool_name,
      toolInput: toolInputRecord(current.input),
    };
  }
  return {};
}

/**
 * Orb signal for one subagent row.
 *
 * `pending` is the only "accepted but not started" signal a subagent has,
 * which is exactly the breathing (queued) case.
 */
export function subagentOrbActivity(view: SubagentView): OrbActivity {
  if (view.status === "pending") return { queued: true };
  if (!isRunning(view)) return {};
  const recent = lastToolCalls(view.items, 2);
  const current = recent[0];
  if (looksLikeRetry(recent)) return { retrying: true };
  if (current?.status === "running") {
    return {
      toolName: current.tool_name,
      toolInput: toolInputRecord(current.input),
    };
  }
  return {};
}
