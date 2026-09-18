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

/** Contract prefix the Claude adapter stamps on the warning it emits when
 *  the sidecar couldn't resume a stale session and transparently rebuilt a
 *  fresh one. The remainder is the ready-to-render notice text. */
const RESUME_FALLBACK_PREFIX = "resume-fallback: ";

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
 * - `"assistant error: <reason>"` → `"Provider error: <reason>"` (the
 *   SDK's enumerated assistant errors, e.g. overloaded). `rate_limit` is
 *   null: a usage-limit stop arrives as `usage_limit_reached`, which owns
 *   the transcript record and the resume affordance.
 * - `"resume-fallback: <text>"` → `<text>` (stale-session recovery: the
 *   remainder is already user-ready copy explaining the fresh session).
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
  if (message.startsWith(ASSISTANT_ERROR_PREFIX)) {
    const reason = message.slice(ASSISTANT_ERROR_PREFIX.length);
    if (reason === RATE_LIMIT_REASON) return null;
    return "Provider error: " + reason;
  }
  if (message.startsWith(RESUME_FALLBACK_PREFIX)) {
    return message.slice(RESUME_FALLBACK_PREFIX.length);
  }
  return null;
}
