/**
 * The GitHub budget gate: stop asking when the host has said no.
 *
 * GitHub meters the GraphQL API — the one `gh pr list` spends from — at
 * five thousand points an hour. Every request past that is refused for
 * the rest of the hour, and the refusal is *cheaper for the host than
 * for us*: it arrives fast, it looks exactly like a normal failure, and
 * the ordinary reaction to a failure is to try again. A page that fans
 * out across twenty repositories and retries each one therefore spends
 * its entire recovery window making the wall it just hit taller.
 *
 * So the gate. The first refusal pauses every pull-request query, at
 * once and globally — one gate for the page, the sidebar badge and the
 * toast watcher together, because they all draw from one budget and a
 * gate any of them could route around is not a gate. It then asks the
 * host when the budget refills and waits exactly that long.
 *
 * Three properties this file exists to guarantee:
 *
 * - **Pause first, ask second.** The pause is set to a default the
 *   instant a refusal is seen, before the call that refines it. The
 *   refining call is itself a request, and a gate that opened a window
 *   for one more request per failure would have nothing to say for
 *   itself.
 * - **Nothing on screen goes away.** The gate only stops *fetching*.
 *   The rows already loaded stay loaded, and the strip above them says
 *   what happened and when it will lift — the same "stale and labelled
 *   beats blank" rule the rest of the page is built on.
 * - **It lifts by itself.** A timer fires at the reset, the queries
 *   re-enable, and the page refreshes without the user having done
 *   anything. Retry is offered, not required.
 */

import { useEffect, useSyncExternalStore } from "react";

import { githubRateLimit } from "@/tauri/commands";

/**
 * Never pause for less than this, even if the host says the budget has
 * already refilled.
 *
 * A reset in the past means the clocks disagree or the reply was stale,
 * and resuming instantly on that reading is how a gate turns into a
 * retry loop with extra steps.
 */
export const MIN_PAUSE_MS = 60_000;

/**
 * Never pause for longer than this. GitHub's window is an hour, so an
 * hour is the longest honest answer; anything beyond it is a misread
 * reply, and the cost of testing that reading is one request.
 */
export const MAX_PAUSE_MS = 60 * 60_000;

/**
 * What we wait when the host won't say. Long enough to be a real
 * back-off, short enough that a transient refusal doesn't cost the rest
 * of the hour.
 */
export const DEFAULT_PAUSE_MS = 5 * 60_000;

/**
 * A refusal for spending too much, as opposed to any other failure.
 *
 * Matched on wording because that is what the host gives us: `gh` exits
 * non-zero with the API's own sentence, and the two GitHub uses are
 * "API rate limit exceeded" and "API rate limit already exceeded". The
 * secondary-limit wording ("exceeded a secondary rate limit") is caught
 * too — it is the same instruction to back off, on a shorter clock.
 *
 * Narrow by intent rather than by construction: this is a substring
 * match on prose, so it recognises the refusals the host actually sends
 * and nothing that merely fails. What it cannot do is tell a primary
 * budget from a secondary block — both say "rate limit" — which is why
 * `noteRateLimited` re-reads the remaining budget before deciding how
 * long to wait rather than trusting this to have classified it.
 *
 * An unreachable repository must stay a footer line rather than pausing
 * every other repository with it, and it does: "could not resolve host"
 * says nothing about limits.
 */
export function isRateLimitError(error: unknown): boolean {
  return /\b(rate limit|ratelimit)\b/i.test(String(error ?? ""));
}

/**
 * Whether this repository draws on the budget the gate is holding.
 *
 * The gate is GitHub's hourly GraphQL budget. A GitLab root is metered
 * by an entirely different host and must keep polling while GitHub is
 * refusing — one over-budget GitHub account silently freezing the badge,
 * the toasts and the page for a GitLab repository would be a new bug
 * traded for the old one.
 *
 * `null` is GitHub, per the wire's back-compat rule (`resolveProvider`):
 * an unnamed host is the one the app started with, and flattening it to
 * "some other host" here would exempt most repositories from the gate.
 */
export function budgetApplies(providerKind: string | null | undefined): boolean {
  return (providerKind ?? "github") === "github";
}

/**
 * When to resume, given what the host said about the refill.
 *
 * `resetEpochSec` of zero or less means it said nothing, which is the
 * default's case rather than an instruction to resume now.
 */
export function pauseUntil(resetEpochSec: number, now: number): number {
  const raw = resetEpochSec > 0 ? resetEpochSec * 1000 : now + DEFAULT_PAUSE_MS;
  return Math.min(Math.max(raw, now + MIN_PAUSE_MS), now + MAX_PAUSE_MS);
}

// ── The gate itself ──────────────────────────────────────────────────
//
// Module scope, for the same reason the snapshot write guard is: these
// are three components sharing one query cache and, underneath it, one
// account's budget. A per-component gate would let the badge keep
// spending while the page waits.

let pausedUntil = 0;
const listeners = new Set<() => void>();

function publish(value: number): void {
  if (value === pausedUntil) return;
  pausedUntil = value;
  for (const listener of listeners) listener();
}

export function subscribeRateLimit(listener: () => void): () => void {
  return subscribe(listener);
}

/** Test seam: raise the gate without a refusal to raise it from. */
export function publishForTest(until: number): void {
  publish(until);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** Epoch ms the gate lifts at, or 0 when nothing is paused. */
export function getPausedUntil(): number {
  return pausedUntil;
}

/** Lift the gate — what Retry means, and what the timer does. */
export function clearRateLimitPause(): void {
  publish(0);
}

/** Test seam: a fresh gate without reloading the module.
 *
 *  Goes through `publish` rather than assigning: a subscriber that is
 *  still mounted would otherwise keep the old snapshot and never
 *  re-render, which is the one way to produce the permanently-stuck gate
 *  this file is written to avoid. */
export function _resetRateLimitGate(): void {
  publish(0);
}

/**
 * Record a refusal and work out how long to wait.
 *
 * `path` is the repository whose call was refused, and is only there to
 * give `gh` a working directory that resolves the right host — an
 * enterprise root and a github.com root have separate budgets.
 *
 * Idempotent while a pause is in force: many roots fail in the same
 * cycle and each will call this, but only the first one pays for the
 * lookup.
 */
export async function noteRateLimited(path: string, now = Date.now()): Promise<void> {
  if (pausedUntil > now) return;
  // Pause on the default *first*. Everything below this line is a
  // request, and until the gate is up the polls it is meant to stop are
  // still firing.
  publish(now + DEFAULT_PAUSE_MS);
  try {
    const limit = await githubRateLimit(path);
    // `now` again, not the one captured above: the lookup is a shell-out
    // with a ten-second budget of its own, and the floor below is only a
    // floor if it is measured from when it is applied.
    const settled = Date.now();
    // The reset the host reports is the *primary* hourly window's, and
    // it reports it whether or not that window is the one that refused
    // us. A secondary limit — the short block GitHub applies for asking
    // too fast, which `isRateLimitError` deliberately also catches — can
    // arrive with three-quarters of the hourly budget still unspent, and
    // waiting for an hourly rollover that was never the problem would
    // turn a sixty-second block into fifty-nine minutes of dark page.
    // Headroom remaining means it was not the hourly budget: wait the
    // minimum and find out.
    publish(
      limit.graphql_remaining > 0
        ? settled + MIN_PAUSE_MS
        : pauseUntil(limit.graphql_reset, settled),
    );
  } catch {
    // The host wouldn't say. The default stands, which is the whole
    // reason it is set before the asking rather than after it.
  }
}

/**
 * The gate, as a React value: epoch ms to resume at, or 0 when open.
 *
 * Also owns the timer that lifts it, so the queries re-enable and the
 * page refreshes on its own the moment the budget refills.
 */
export function useRateLimitPause(): number {
  const until = useSyncExternalStore(subscribe, getPausedUntil, getPausedUntil);

  useEffect(() => {
    if (until === 0) return;
    const delay = until - Date.now();
    if (delay <= 0) {
      clearRateLimitPause();
      return;
    }
    const timer = setTimeout(clearRateLimitPause, delay);
    return () => clearTimeout(timer);
  }, [until]);

  // A pause whose moment has passed is already open, even if the timer
  // above has not run yet — so a render never gates on a stale value.
  return until > Date.now() ? until : 0;
}
