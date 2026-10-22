/**
 * Classifier that decides which `runtime_warning` events deserve a
 * user-facing inline transcript notice (a `RuntimeNoticeItem`) versus
 * staying console-only SDK debug noise.
 *
 * Kept pure + separately unit-tested: the reducer's `runtime_warning`
 * case calls this and only appends a row when it returns a non-null
 * message. Deliberately conservative — the vast majority of
 * `runtime_warning`s are SDK lifecycle chatter (`stream_event …`) that
 * must never spam the transcript.
 */

/** Prefix the SDK uses for its enumerated assistant errors (rate_limit,
 *  overloaded, …). The remainder is a short human-readable reason. */
const ASSISTANT_ERROR_PREFIX = "assistant error: ";

/** The assistant error a usage-limit stop reports. */
const RATE_LIMIT_REASON = "rate_limit";

/** The assistant error a signed-out Claude CLI reports. */
const SIGNED_OUT_WARNING = ASSISTANT_ERROR_PREFIX + "authentication_failed";

/** The provider whose signed-out CLI a `runtime_warning` reports, if any.
 *  Only Claude's SDK names this error; other providers fail at session
 *  start, where the health banner already covers them. */
export function signedOutProviderFromWarning(message: string): "claude" | null {
  return message === SIGNED_OUT_WARNING ? "claude" : null;
}

/** Contract prefix the Claude adapter stamps on the warning it emits when
 *  the sidecar couldn't resume a stale session and transparently rebuilt a
 *  fresh one. The remainder is the ready-to-render notice text. */
const RESUME_FALLBACK_PREFIX = "resume-fallback: ";

/** Codex could not apply a saved premium tier and selected Standard. */
export const SERVICE_TIER_FALLBACK_PREFIX = "service-tier-fallback: ";

/** Claude adapter: an idle chat was compacted on a plugin's request.
 *  Informational, not a warning. */
export const IDLE_COMPACTION_PREFIX = "idle-compaction: ";

/** Whether a notice row reports an idle compaction (rendered muted). */
export function isIdleCompactionWarning(message: string): boolean {
  return message.startsWith(IDLE_COMPACTION_PREFIX);
}

/** Explicit restriction emitted by the pinned Hermes adapter on session start. */
const HERMES_USAGE_LIMIT_RECOVERY_PREFIX = "hermes-usage-limit-recovery: ";

/** A chat started without Codemux's collaboration tools (OpenCode 1). */
const COLLABORATION_UNAVAILABLE_PREFIX = "collaboration-unavailable: ";

/** Defensive nested access: `originalPayload.rate_limit_info.status`. */
function readRateLimitStatus(originalPayload: unknown): string | null {
  if (!originalPayload || typeof originalPayload !== "object") return null;
  const info = (originalPayload as { rate_limit_info?: unknown })
    .rate_limit_info;
  if (!info || typeof info !== "object") return null;
  const status = (info as { status?: unknown }).status;
  return typeof status === "string" ? status : null;
}

/**
 * Map a `runtime_warning` to a user-facing notice string, or `null` when
 * it should stay console-only.
 *
 * - `"rate limit event"` (legacy persisted rows only; current adapters emit
 *   `usage_limit_reached` instead) → a notice only when
 *   `rate_limit_info.status === "rejected"` (the provider actually
 *   stopped the run); an informational rate-limit tick is null.
 * - `"assistant error: authentication_failed"` → a plain "isn't signed
 *   in" sentence (the row carries a Sign in action; see
 *   `signedOutProviderFromWarning`).
 * - `"assistant error: <reason>"` → `"Provider error: <reason>"` (the
 *   SDK's enumerated assistant errors, e.g. overloaded). `rate_limit` is
 *   null: a usage-limit stop arrives as `usage_limit_reached`, which owns
 *   the transcript record and the resume affordance.
 * - `"resume-fallback: <text>"` → `<text>` (stale-session recovery: the
 *   remainder is already user-ready copy explaining the fresh session).
 * - "service-tier-fallback: <text>" → `<text>` (saved premium tier
 *   could not be applied; the session uses Standard).
 * - `"hermes-usage-limit-recovery: <text>"` → `<text>` (the adapter's
 *   explicit compatibility restriction, persisted for reopened chats).
 * - `"collaboration-unavailable: <text>"` → `<text>` (the chat runs
 *   without Codemux's tools, and why).
 * - `"idle-compaction: <text>"` → `<text>` (an idle Claude chat was
 *   summarized on a plugin's request; rendered as info).
 * - anything else → null (SDK debug noise).
 */
export function runtimeNoticeFromWarning(
  message: string,
  originalPayload: unknown,
): string | null {
  if (message === "rate limit event") {
    return readRateLimitStatus(originalPayload) === "rejected"
      ? "Usage limit reached — the provider stopped the run."
      : null;
  }
  if (message === SIGNED_OUT_WARNING) return "Claude Code isn't signed in.";
  if (message.startsWith(ASSISTANT_ERROR_PREFIX)) {
    const reason = message.slice(ASSISTANT_ERROR_PREFIX.length);
    if (reason === RATE_LIMIT_REASON) return null;
    return "Provider error: " + reason;
  }
  if (message.startsWith(SERVICE_TIER_FALLBACK_PREFIX)) {
    return message.slice(SERVICE_TIER_FALLBACK_PREFIX.length);
  }
  if (message.startsWith(RESUME_FALLBACK_PREFIX)) {
    return message.slice(RESUME_FALLBACK_PREFIX.length);
  }
  if (message.startsWith(HERMES_USAGE_LIMIT_RECOVERY_PREFIX)) {
    return message.slice(HERMES_USAGE_LIMIT_RECOVERY_PREFIX.length);
  }
  if (message.startsWith(COLLABORATION_UNAVAILABLE_PREFIX)) {
    return message.slice(COLLABORATION_UNAVAILABLE_PREFIX.length);
  }
  if (isIdleCompactionWarning(message)) {
    return message.slice(IDLE_COMPACTION_PREFIX.length);
  }
  return null;
}
