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
 * - `"rate limit event"` → a notice only when
 *   `rate_limit_info.status === "rejected"` (the provider actually
 *   stopped the run); an informational rate-limit tick is null.
 * - `"assistant error: <reason>"` → `"Provider error: <reason>"` (the
 *   SDK's enumerated assistant errors, e.g. rate_limit / overloaded).
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
    return "Provider error: " + message.slice(ASSISTANT_ERROR_PREFIX.length);
  }
  return null;
}
