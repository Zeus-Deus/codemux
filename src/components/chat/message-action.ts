/**
 * Goes on the message root that owns a footer strip — the hover/focus target
 * `MESSAGE_ACTION_CLASS` reveals off. Named so it can't be confused with the
 * unnamed `group`s inside a message's own content.
 */
export const MESSAGE_GROUP_CLASS = "group/message";

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
 *  - Hidden by default; revealed by the owning message's `group/message` on
 *    hover or focus-within, so the whole message is the hover target rather
 *    than a 10px chip you have to hunt for. The group is *named*: a message
 *    wraps content that runs its own unnamed `group` (inline images zoom on
 *    hover), and an unnamed `group-hover:` fires from any `.group` ancestor,
 *    so hovering the message would otherwise trigger those too.
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
  "group-hover/message:pointer-events-auto group-hover/message:opacity-100",
  "group-focus-within/message:pointer-events-auto group-focus-within/message:opacity-100",
  "focus-visible:pointer-events-auto focus-visible:opacity-100",
  "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
  "pointer-coarse:pointer-events-auto pointer-coarse:opacity-100",
].join(" ");

/** Gap between the strip's actions, and the strip's offset from the message. */
export const MESSAGE_ACTION_ROW_CLASS = "flex items-center gap-2.5";
