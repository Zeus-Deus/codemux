import type { ChatViewItem } from "./types";

/**
 * Decide whether to render the transcript-tail "thinking" pulse.
 *
 * The pulse fills the dead time between a user action and the first
 * visible agent response: after submit, after AskUserQuestion answers,
 * after plan accept/reject. Once the agent emits an observable signal
 * (streaming prose, a running tool, a pending approval) the indicator
 * steps back so it never competes with that signal for attention.
 *
 * Tail-based: the caller passes `messages` in append order (seq is
 * strictly increasing, so array order matches seq order) and we
 * inspect the last item. That's all we need — once a new activity
 * lands, its own affordance takes over.
 */
export function shouldShowThinkingIndicator(
  messages: ChatViewItem[],
  streaming: boolean,
): boolean {
  if (!streaming) return false;
  if (messages.length === 0) return true;
  const last = messages[messages.length - 1];
  switch (last.kind) {
    case "assistant_message":
      // A streaming assistant tail normally renders its own caret, so the
      // pulse steps back (`!streaming` → false). But a streaming-but-EMPTY
      // tail renders nothing yet — keep the marker up so the turn never
      // looks finished. Layer 1 drops empty deltas before they become an
      // item; this is defense in depth for providers that still land one.
      if (last.streaming && last.text.length === 0) return true;
      return !last.streaming;
    case "reasoning":
      // A streaming reasoning block renders its own "Thinking…" header, so
      // the tail pulse steps back; once sealed it's dead time again.
      return !last.streaming;
    case "tool_call":
      return last.status !== "running";
    case "permission_request":
      return last.resolution.state !== "pending";
    case "subagent_run":
      // The orchestration card renders its own live spinners while any
      // subagent is working, so the tail pulse steps back; once every
      // subagent finishes it's dead time again (waiting on the
      // orchestrator to resume).
      return last.subagents.every(
        (s) => s.status !== "running" && s.status !== "pending",
      );
    case "workflow_run":
      // The workflow card shows its own spinner/progress bar while the
      // run is live (and the approval card while pending), so the tail
      // pulse steps back until the run reaches a terminal state.
      return (
        last.status !== "running" && last.status !== "pending_approval"
      );
    case "user_message":
    case "turn_ended":
      return true;
  }
}
