import { describe, it, expect, vi, beforeEach } from "vitest";

const mockGithubRateLimit = vi.fn();

vi.mock("@/tauri/commands", () => ({
  githubRateLimit: (...a: unknown[]) => mockGithubRateLimit(...a),
}));

import {
  DEFAULT_PAUSE_MS,
  MAX_PAUSE_MS,
  MIN_PAUSE_MS,
  _resetRateLimitGate,
  clearRateLimitPause,
  getPausedUntil,
  isRateLimitError,
  noteRateLimited,
  pauseUntil,
  publishForTest,
  subscribeRateLimit,
} from "./pr-rate-limit";

const ROOT = "/home/dev/projects/codemux";
const NOW = 1_787_474_000_000;

beforeEach(() => {
  _resetRateLimitGate();
  mockGithubRateLimit.mockReset();
});

describe("isRateLimitError", () => {
  it("recognises every wording the host actually sends", () => {
    // The first of these is verbatim what `gh pr list` printed while the
    // page was locked out; the rest are GitHub's other two phrasings.
    expect(
      isRateLimitError("GraphQL: API rate limit already exceeded for user ID 100132710."),
    ).toBe(true);
    expect(isRateLimitError("HTTP 403: API rate limit exceeded for user ID 1.")).toBe(true);
    expect(isRateLimitError("You have exceeded a secondary rate limit")).toBe(true);
  });

  it("leaves every other failure alone", () => {
    // The narrowness is the point: these are failures retrying *would*
    // fix, and one unreachable repository must never pause the rest.
    expect(isRateLimitError("could not resolve host github.com")).toBe(false);
    expect(isRateLimitError("gh: command not found")).toBe(false);
    expect(isRateLimitError("no pull requests match your search")).toBe(false);
    expect(isRateLimitError(null)).toBe(false);
    expect(isRateLimitError(undefined)).toBe(false);
  });
});

describe("pauseUntil", () => {
  it("waits until the host says the budget refills", () => {
    const reset = Math.floor(NOW / 1000) + 900;
    expect(pauseUntil(reset, NOW)).toBe(reset * 1000);
  });

  it("never resumes instantly on a reset that has already passed", () => {
    // Clock skew, or a reply that sat in a queue. Resuming on this
    // reading is how a gate turns back into a retry loop.
    const past = Math.floor(NOW / 1000) - 300;
    expect(pauseUntil(past, NOW)).toBe(NOW + MIN_PAUSE_MS);
  });

  it("falls back to the default when the host said nothing", () => {
    expect(pauseUntil(0, NOW)).toBe(NOW + DEFAULT_PAUSE_MS);
  });

  it("never waits longer than the host's own window", () => {
    // GitHub's budget is hourly, so a reset a day out is a misread
    // reply — and the cost of testing that reading is one request.
    const absurd = Math.floor(NOW / 1000) + 86_400;
    expect(pauseUntil(absurd, NOW)).toBe(NOW + MAX_PAUSE_MS);
  });
});

describe("noteRateLimited", () => {
  it("raises the gate before it asks how long for", async () => {
    // The asking is itself a request. A gate that stayed open until the
    // answer arrived would leave a window for exactly the polls it
    // exists to stop — one per root, on the cycle that just failed.
    let release!: (value: unknown) => void;
    mockGithubRateLimit.mockReturnValue(
      new Promise((resolve) => {
        release = resolve;
      }),
    );

    const pending = noteRateLimited(ROOT, NOW);
    expect(getPausedUntil()).toBe(NOW + DEFAULT_PAUSE_MS);

    // A real reset, because the refinement below is measured from the
    // clock at the moment it lands rather than from the moment the
    // refusal was seen — the lookup is a shell-out with a budget of its
    // own, and a floor is only a floor if it is measured from where it
    // is applied.
    const reset = Math.floor(Date.now() / 1000) + 600;
    release({ graphql_remaining: 0, graphql_reset: reset });
    await pending;
    expect(getPausedUntil()).toBe(reset * 1000);
  });

  it("waits the minimum, not the hour, when the budget still has headroom", async () => {
    // A secondary limit — the short block GitHub applies for asking too
    // fast — says "rate limit" in the same words as an exhausted hourly
    // budget, but arrives with the hourly budget largely unspent. The
    // reset the host reports is the hourly window's rollover either way,
    // so trusting it here would answer a sixty-second block with most of
    // an hour of dark page.
    const now = Date.now();
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 4_140,
      graphql_reset: Math.floor(now / 1000) + 3_000,
      core_remaining: 5_000,
      core_reset: Math.floor(now / 1000) + 3_000,
    });

    await noteRateLimited(ROOT);

    const paused = getPausedUntil();
    expect(paused).toBeGreaterThanOrEqual(now + MIN_PAUSE_MS);
    expect(paused).toBeLessThan(now + MIN_PAUSE_MS + 5_000);
  });

  it("does wait for the rollover when the hourly budget is the thing that ran out", async () => {
    const now = Date.now();
    const reset = Math.floor(now / 1000) + 1_800;
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: reset,
      core_remaining: 5_000,
      core_reset: reset,
    });

    await noteRateLimited(ROOT);
    expect(getPausedUntil()).toBe(reset * 1000);
  });

  it("asks once however many roots were refused", async () => {
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: Math.floor(NOW / 1000) + 600,
    });

    await noteRateLimited(ROOT, NOW);
    await noteRateLimited("/home/dev/projects/site", NOW);
    await noteRateLimited("/home/dev/projects/docs", NOW);

    expect(mockGithubRateLimit).toHaveBeenCalledTimes(1);
  });

  it("wakes its subscribers when the gate is reset", () => {
    // Assigning the module value without publishing would leave a
    // mounted subscriber holding the old snapshot forever — the one way
    // to produce a gate that never lifts.
    const seen: number[] = [];
    const stop = subscribeRateLimit(() => seen.push(getPausedUntil()));
    publishForTest(Date.now() + 60_000);
    _resetRateLimitGate();
    stop();
    expect(seen[seen.length - 1]).toBe(0);
  });

  it("keeps the default when the host will not say", async () => {
    mockGithubRateLimit.mockRejectedValue("gh exited 1");
    await noteRateLimited(ROOT, NOW);
    expect(getPausedUntil()).toBe(NOW + DEFAULT_PAUSE_MS);
  });

  it("can be asked again once the gate has lifted", async () => {
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: Math.floor(NOW / 1000) + 600,
    });
    await noteRateLimited(ROOT, NOW);
    clearRateLimitPause();
    expect(getPausedUntil()).toBe(0);

    await noteRateLimited(ROOT, NOW);
    expect(mockGithubRateLimit).toHaveBeenCalledTimes(2);
  });
});
