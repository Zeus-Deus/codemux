// Relative-time formatting for "Last synced N minutes ago" labels
// in the Stage 5 sync status display, plus future spots that need
// the same shape.
//
// The buckets escalate from seconds → minutes → hours → days, then
// fall back to absolute date for anything older than a week. Each
// boundary gets its own pluralization (1 minute / 2 minutes) so
// the label never reads "1 minutes ago".
//
// `now` is injected as a parameter rather than always reading
// `Date.now()` so unit tests don't have to monkey-patch the
// global clock.

export function relativeTime(date: Date, now: Date = new Date()): string {
  const diffMs = now.getTime() - date.getTime();

  // Future timestamps (clock skew on a freshly-pulled mtime, or
  // a test that injects a date in the future) should not render
  // as "5 minutes ago" — that's confusing. Treat them as "just
  // now" so the label stays sensible.
  if (diffMs < 0) return "just now";

  const diffSec = Math.floor(diffMs / 1000);
  if (diffSec < 60) return "just now";

  if (diffSec < 3600) {
    const minutes = Math.floor(diffSec / 60);
    return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  }

  if (diffSec < 86_400) {
    const hours = Math.floor(diffSec / 3600);
    return `${hours} hour${hours === 1 ? "" : "s"} ago`;
  }

  if (diffSec < 7 * 86_400) {
    const days = Math.floor(diffSec / 86_400);
    return `${days} day${days === 1 ? "" : "s"} ago`;
  }

  // Older than a week: locale-formatted absolute date. The user's
  // browser locale determines the format; tests that depend on a
  // specific format pass an explicit locale via Date.toLocaleDateString
  // out of band.
  return date.toLocaleDateString();
}
