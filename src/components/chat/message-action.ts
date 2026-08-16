/**
 * Shared styling for the actions in a message's footer strip — Copy on every
 * settled message, Revert on turns that carry a checkpoint.
 *
 * They sit side by side under the same message, so they have to reveal as one
 * unit: any difference in when they fade in reads as a glitch rather than as
 * two independent controls. Centralising the rule is what keeps a third action
 * from drifting out of step later.
 *
 * The contract:
 *  - Hidden by default; revealed by the owning message's `group` on hover or
 *    focus-within, so the whole message is the hover target rather than a
 *    10px chip you have to hunt for.
 *  - `pointer-events-none` while hidden, so the strip — which is laid out
 *    unconditionally, to keep hover from reflowing the transcript — never
 *    becomes an invisible click target.
 *  - Always shown on a coarse pointer, which has no hover at all: gating on it
 *    would make these unreachable in the remote web client on a phone.
 *
 * A control that is mid-operation or mid-confirmation (Reverting…, Copied)
 * pins itself visible on top of this; see the call sites.
 */
export const MESSAGE_ACTION_CLASS = [
  "flex items-center gap-0.5 rounded text-[10px] text-muted-foreground",
  "pointer-events-none opacity-0 transition-opacity",
  "hover:text-foreground",
  "group-hover:pointer-events-auto group-hover:opacity-100",
  "group-focus-within:pointer-events-auto group-focus-within:opacity-100",
  "focus-visible:pointer-events-auto focus-visible:opacity-100",
  "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
  "pointer-coarse:pointer-events-auto pointer-coarse:opacity-100",
].join(" ");

/** Gap between the strip's actions, and the strip's offset from the message. */
export const MESSAGE_ACTION_ROW_CLASS = "flex items-center gap-2.5";
