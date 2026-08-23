import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, renderHook, waitFor, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const mockListPrsOverview = vi.fn();
const mockListPrsOverviewStats = vi.fn();
const mockListPullRequests = vi.fn().mockResolvedValue([]);
const mockGithubRateLimit = vi.fn();

vi.mock("@/tauri/commands", () => ({
  listPrsOverview: (...a: unknown[]) => mockListPrsOverview(...a),
  listPrsOverviewStats: (...a: unknown[]) => mockListPrsOverviewStats(...a),
  listPullRequests: (...a: unknown[]) => mockListPullRequests(...a),
  githubRateLimit: (...a: unknown[]) => mockGithubRateLimit(...a),
}));

const mockWorkspaces: {
  workspace_id: string;
  project_root: string | null;
  cwd: string;
  provider_kind: string | null;
}[] = [];

vi.mock("@/stores/app-store", () => ({
  useAppStore: (selector: (s: Record<string, unknown>) => unknown) =>
    selector({ appState: { workspaces: mockWorkspaces } }),
}));

import {
  usePrOverview,
  mergeStats,
  prOverviewKey,
  prOverviewStatsKey,
  _resetSnapshotWriteGuard,
} from "./pr-overview-query";
import { _resetRateLimitGate, clearRateLimitPause } from "./pr-rate-limit";
import {
  PR_SNAPSHOT_VERSION,
  prSnapshotKey,
  readPrOverviewSnapshot,
} from "./pr-overview-snapshot";
import type { PrRow } from "./pr-overview";

const ROOT = "/home/dev/projects/codemux";
const OTHER = "/home/dev/projects/site";

function item(over: Record<string, unknown> & { number: number }) {
  return {
    title: `pull request ${over.number}`,
    author: "juliusm",
    head_branch: `branch/${over.number}`,
    is_draft: false,
    additions: null,
    deletions: null,
    review_decision: null,
    checks: null,
    review_requested_from: [],
    updated_at: "2026-08-16T10:00:00Z",
    url: `https://github.com/example/codemux/pull/${over.number}`,
    ...over,
  };
}

function row(over: Partial<PrRow> & { number: number }): PrRow {
  return {
    title: `carried ${over.number}`,
    author: "juliusm",
    head_branch: `branch/${over.number}`,
    is_draft: false,
    additions: 5,
    deletions: 1,
    review_decision: null,
    checks: "passing",
    review_requested_from: [],
    updated_at: "2026-08-15T10:00:00Z",
    url: `https://github.com/example/codemux/pull/${over.number}`,
    projectRoot: ROOT,
    repo: "example/codemux",
    providerKind: "github",
    ...over,
  };
}

/** A promise whose settlement the test decides. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  // An unhandled rejection here would fail the run before the query
  // layer has had a chance to catch it.
  promise.catch(() => {});
  return { promise, resolve, reject };
}

function wrapper() {
  const client = new QueryClient({
    defaultOptions: {
      queries: { retryDelay: 0, gcTime: 0 },
    },
  });
  return ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
}

function setRoots(...paths: string[]) {
  mockWorkspaces.length = 0;
  for (const path of paths) {
    mockWorkspaces.push({
      workspace_id: `ws-${path}`,
      project_root: path,
      cwd: path,
      provider_kind: "github",
    });
  }
}

beforeEach(() => {
  localStorage.clear();
  _resetSnapshotWriteGuard();
  _resetRateLimitGate();
  mockGithubRateLimit.mockReset();
  mockListPrsOverview.mockReset();
  mockListPrsOverviewStats.mockReset();
  setRoots(ROOT);
});

afterEach(() => {
  cleanup();
});

// ── The merge, as a function ─────────────────────────────────────────

describe("mergeStats", () => {
  it("fills a blank", () => {
    const merged = mergeStats(row({ number: 1, checks: null, additions: null }), new Map([
      [1, { number: 1, checks: "failing", additions: 12, deletions: 3 }],
    ]));
    expect(merged.checks).toBe("failing");
    expect(merged.additions).toBe(12);
  });

  it("leaves a row alone when the stats say nothing about it", () => {
    const original = row({ number: 1, checks: "passing" });
    // Identity, not just equality: an untouched row must not re-render.
    expect(mergeStats(original, new Map())).toBe(original);
  });

  it("leaves a row alone when there are no stats at all", () => {
    const original = row({ number: 1, checks: "passing" });
    expect(mergeStats(original, null)).toBe(original);
  });

  it("never blanks a value it has nothing to say about", () => {
    const merged = mergeStats(row({ number: 1, additions: 12, deletions: 3 }), new Map([
      [1, { number: 1, checks: "failing", additions: null, deletions: null }],
    ]));
    expect(merged.additions).toBe(12);
    expect(merged.deletions).toBe(3);
    expect(merged.checks).toBe("failing");
  });
});

// ── The two-phase paint ──────────────────────────────────────────────

describe("usePrOverview — fast then stats", () => {
  it("paints rows before the checks land, and merges them in after", async () => {
    mockListPrsOverview.mockResolvedValue({
      viewer: "mock-dev",
      items: [item({ number: 1 }), item({ number: 2 })],
    });
    const stats = deferred<unknown>();
    mockListPrsOverviewStats.mockReturnValue(stats.promise);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    // Phase one: everything but the colour.
    await waitFor(() => expect(result.current.rows).toHaveLength(2));
    expect(result.current.rows.map((r) => r.checks)).toEqual([null, null]);
    expect(result.current.rows[0].title).toBe("pull request 1");

    // Phase two: the colour, and not one row fewer.
    stats.resolve([
      { number: 1, checks: "failing", additions: 12, deletions: 3 },
      { number: 2, checks: "passing", additions: 1, deletions: 1 },
    ]);
    await waitFor(() => expect(result.current.rows[0].checks).toBe("failing"));
    expect(result.current.rows).toHaveLength(2);
    expect(result.current.rows[1].checks).toBe("passing");
    expect(result.current.rows[0].additions).toBe(12);
  });

  it("keeps the rows when the stats call fails", async () => {
    mockListPrsOverview.mockResolvedValue({
      viewer: "mock-dev",
      items: [item({ number: 1 })],
    });
    mockListPrsOverviewStats.mockRejectedValue("gh: could not compute status checks");

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    await waitFor(() => expect(result.current.rows).toHaveLength(1));
    await waitFor(() => expect(mockListPrsOverviewStats).toHaveBeenCalled());

    // Still one row, still no invented colour, and the listing itself is
    // not reported as a failure — the slow half failing is not the list
    // failing.
    expect(result.current.rows).toHaveLength(1);
    expect(result.current.rows[0].checks).toBeNull();
    expect(result.current.failures).toHaveLength(0);
  });

  it("does not ask for stats when the host already answered", async () => {
    // The GitLab shape: the list call carries the pipeline, so there is
    // nothing to go back for.
    mockListPrsOverview.mockResolvedValue({
      viewer: "mock-glab",
      items: [item({ number: 1, checks: "passing" })],
    });

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    await waitFor(() => expect(result.current.rows).toHaveLength(1));
    expect(result.current.rows[0].checks).toBe("passing");
    expect(mockListPrsOverviewStats).not.toHaveBeenCalled();
  });
});

// ── The carried paint ────────────────────────────────────────────────

describe("usePrOverview — carried rows", () => {
  function seedSnapshot(savedAt = Date.now() - 7_200_000) {
    localStorage.setItem(
      prSnapshotKey([ROOT]),
      JSON.stringify({
        version: PR_SNAPSHOT_VERSION,
        savedAt,
        rows: [row({ number: 1 }), row({ number: 2 })],
        viewerByRoot: { [ROOT]: "mock-dev" },
      }),
    );
  }

  it("paints from the snapshot before any query resolves", () => {
    seedSnapshot(1000);
    mockListPrsOverview.mockReturnValue(deferred<unknown>().promise);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    // The first render, synchronously — no await anywhere.
    expect(result.current.rows).toHaveLength(2);
    expect(result.current.rows[0].title).toBe("carried 1");
    expect(result.current.carried).toBe(true);
    expect(result.current.carriedAt).toBe(1000);
    expect(result.current.viewerByRoot.get(ROOT)).toBe("mock-dev");
  });

  it("replaces carried rows wholesale when the host answers", async () => {
    seedSnapshot();
    const fast = deferred<unknown>();
    mockListPrsOverview.mockReturnValue(fast.promise);
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    expect(result.current.rows.map((r) => r.number)).toEqual([1, 2]);

    fast.resolve({ viewer: "mock-dev", items: [item({ number: 9 })] });

    await waitFor(() => expect(result.current.carried).toBe(false));
    // Not merged with the carried set: #1 and #2 are gone because the
    // host says they are.
    expect(result.current.rows.map((r) => r.number)).toEqual([9]);
    expect(result.current.carriedAt).toBeNull();
  });

  it("keeps carrying a root that has not answered while another has", async () => {
    setRoots(ROOT, OTHER);
    localStorage.setItem(
      prSnapshotKey([ROOT, OTHER]),
      JSON.stringify({
        version: PR_SNAPSHOT_VERSION,
        savedAt: 1000,
        rows: [row({ number: 1 }), row({ number: 7, projectRoot: OTHER })],
        viewerByRoot: { [ROOT]: "mock-dev", [OTHER]: "mock-dev" },
      }),
    );
    const slow = deferred<unknown>();
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT
        ? Promise.resolve({ viewer: "mock-dev", items: [item({ number: 5 })] })
        : slow.promise,
    );
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    await waitFor(() =>
      expect(result.current.rows.map((r) => r.number).sort()).toEqual([5, 7]),
    );
    // One root fresh, one still carried — and the page says so.
    expect(result.current.carried).toBe(true);
  });

  it("stops carrying a root once it has failed", async () => {
    // The bug this pins: `carried` means "some root has not answered
    // yet". A root that answers with an error *has* answered, and if it
    // keeps counting as carried it stays carried for the whole session —
    // which silently switches off snapshot writes and every pull-request
    // toast, for every root, until the app is restarted.
    setRoots(ROOT, OTHER);
    localStorage.setItem(
      prSnapshotKey([ROOT, OTHER]),
      JSON.stringify({
        version: PR_SNAPSHOT_VERSION,
        savedAt: 1000,
        rows: [row({ number: 1 }), row({ number: 7, projectRoot: OTHER })],
        viewerByRoot: { [ROOT]: "mock-dev", [OTHER]: "mock-dev" },
      }),
    );
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT
        ? Promise.resolve({ viewer: "mock-dev", items: [item({ number: 5 })] })
        : Promise.reject("could not resolve host"),
    );
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    await waitFor(() => expect(result.current.carried).toBe(false));
    // The dead root's stale rows go with it: it is a footer line now,
    // not a repository we are waiting on.
    expect(result.current.rows.map((r) => r.number)).toEqual([5]);
    expect(result.current.carriedAt).toBeNull();
  });

  it("keeps carrying a root that is merely slow, not failed", async () => {
    // The other half of the same rule — a root still in flight is
    // exactly what the carried paint is for, and must not be dropped
    // just because its neighbour failed.
    setRoots(ROOT, OTHER);
    localStorage.setItem(
      prSnapshotKey([ROOT, OTHER]),
      JSON.stringify({
        version: PR_SNAPSHOT_VERSION,
        savedAt: 1000,
        rows: [row({ number: 7, projectRoot: OTHER })],
        viewerByRoot: { [OTHER]: "mock-dev" },
      }),
    );
    const slow = deferred<unknown>();
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT ? Promise.reject("could not resolve host") : slow.promise,
    );
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    expect(result.current.carried).toBe(true);
    expect(result.current.rows.map((r) => r.number)).toEqual([7]);
  });

  it("does not hydrate from a snapshot taken with a different root set", () => {
    localStorage.setItem(
      prSnapshotKey([ROOT, OTHER]),
      JSON.stringify({
        version: PR_SNAPSHOT_VERSION,
        savedAt: 1000,
        rows: [row({ number: 1 })],
        viewerByRoot: { [ROOT]: "mock-dev" },
      }),
    );
    mockListPrsOverview.mockReturnValue(deferred<unknown>().promise);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    expect(result.current.rows).toHaveLength(0);
    expect(result.current.carried).toBe(false);
  });
});

// ── Writing the snapshot ─────────────────────────────────────────────

describe("usePrOverview — recording a clean refresh", () => {
  it("writes the merged rows once every root has answered", async () => {
    mockListPrsOverview.mockResolvedValue({
      viewer: "mock-dev",
      items: [item({ number: 1 })],
    });
    mockListPrsOverviewStats.mockResolvedValue([
      { number: 1, checks: "failing", additions: 12, deletions: 3 },
    ]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.rows[0]?.checks).toBe("failing"));

    await waitFor(() => expect(readPrOverviewSnapshot([ROOT])).not.toBeNull());
    const snapshot = readPrOverviewSnapshot([ROOT])!;
    // The merged row, not the half of it that arrived first.
    expect(snapshot.rows[0].checks).toBe("failing");
    expect(snapshot.rows[0].additions).toBe(12);
  });

  it("has nothing to write when the only root failed", async () => {
    mockListPrsOverview.mockRejectedValue("could not resolve host");

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.failures).toHaveLength(1));

    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
    expect(result.current.allRootsFailed).toBe(true);
  });

  it("still records the roots that answered when one is unreachable", async () => {
    // One permanently unreachable repository must not cost the user the
    // carried paint for every repository that works.
    setRoots(ROOT, OTHER);
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT
        ? Promise.resolve({ viewer: "mock-dev", items: [item({ number: 1 })] })
        : Promise.reject("could not resolve host"),
    );
    mockListPrsOverviewStats.mockResolvedValue([
      { number: 1, checks: "passing", additions: 2, deletions: 0 },
    ]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.failures).toHaveLength(1));

    await waitFor(() => expect(readPrOverviewSnapshot([ROOT, OTHER])).not.toBeNull());
    const snapshot = readPrOverviewSnapshot([ROOT, OTHER])!;
    expect(snapshot.rows.map((r) => r.number)).toEqual([1]);
    // The failure is not part of what was written.
    expect(snapshot.viewerByRoot).toEqual({ [ROOT]: "mock-dev" });
  });

  it("does not write a carried row back as if it were fresh", async () => {
    setRoots(ROOT, OTHER);
    localStorage.setItem(
      prSnapshotKey([ROOT, OTHER]),
      JSON.stringify({
        version: PR_SNAPSHOT_VERSION,
        savedAt: 1000,
        rows: [row({ number: 1 })],
        viewerByRoot: { [ROOT]: "mock-dev" },
      }),
    );
    const slow = deferred<unknown>();
    mockListPrsOverview.mockImplementation((path: string) =>
      path === OTHER
        ? Promise.resolve({ viewer: "mock-dev", items: [item({ number: 5 })] })
        : slow.promise,
    );
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.rows).toHaveLength(2));

    // One root fresh, one still carried: the old timestamp stands until
    // everything on screen came from the host.
    expect(readPrOverviewSnapshot([ROOT, OTHER])?.savedAt).toBe(1000);
  });

  it("keeps writing the snapshot after a root has permanently failed", async () => {
    // A previously-snapshotted root that errors used to pin `carried`
    // true forever, which blocked every later write — so the snapshot
    // froze at the moment the root died and the contract above ("a
    // failing root deliberately does not block the write") quietly
    // stopped holding.
    setRoots(ROOT, OTHER);
    localStorage.setItem(
      prSnapshotKey([ROOT, OTHER]),
      JSON.stringify({
        version: PR_SNAPSHOT_VERSION,
        savedAt: 1000,
        rows: [row({ number: 1 }), row({ number: 7, projectRoot: OTHER })],
        viewerByRoot: { [ROOT]: "mock-dev", [OTHER]: "mock-dev" },
      }),
    );
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT
        ? Promise.resolve({ viewer: "mock-dev", items: [item({ number: 5 })] })
        : Promise.reject("could not resolve host"),
    );
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.carried).toBe(false));

    await waitFor(() =>
      expect(readPrOverviewSnapshot([ROOT, OTHER])?.savedAt).not.toBe(1000),
    );
    const snapshot = readPrOverviewSnapshot([ROOT, OTHER])!;
    expect(snapshot.rows.map((r) => r.number)).toEqual([5]);
    expect(snapshot.viewerByRoot).toEqual({ [ROOT]: "mock-dev" });
  });

  it("does not write the closed view over the open one", async () => {
    mockListPrsOverview.mockResolvedValue({ viewer: "mock-dev", items: [] });
    mockListPullRequests.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true, "closed"), {
      wrapper: wrapper(),
    });
    await waitFor(() => expect(result.current.updatedAt).not.toBeNull());

    expect(readPrOverviewSnapshot([ROOT])).toBeNull();
  });
});

// ── Failure reporting ────────────────────────────────────────────────

describe("usePrOverview — failure shape", () => {
  it("reports a partial failure without claiming everything failed", async () => {
    setRoots(ROOT, OTHER);
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT
        ? Promise.resolve({ viewer: "mock-dev", items: [item({ number: 1 })] })
        : Promise.reject("could not resolve host"),
    );
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    expect(result.current.allRootsFailed).toBe(false);
    expect(result.current.rows).toHaveLength(1);
  });
});

// ── Cadence ──────────────────────────────────────────────────────────
//
// The fan-out is one `gh` call per repository root, so what it costs is
// the root count times the rate — and the watcher that feeds the sidebar
// badge runs for the whole session whether or not anyone is looking. At
// the page's own cadence that came to more requests per hour than the
// host grants in an hour, and the page then found the budget already
// spent. These two tests are the split that fixed it; they assert the
// interval each mode actually registers rather than waiting for timers.

function clientWrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retryDelay: 0, gcTime: 0 } },
  });
  const Wrapper = ({ children }: { children: ReactNode }) => (
    <QueryClientProvider client={client}>{children}</QueryClientProvider>
  );
  return { client, wrapper: Wrapper };
}

function intervalOf(client: QueryClient, key: readonly unknown[]): unknown {
  const query = client.getQueryCache().find({ queryKey: key });
  return query?.observers[0]?.options.refetchInterval;
}

describe("usePrOverview — cadence", () => {
  it("gives the page the cadence a person notices", async () => {
    mockListPrsOverview.mockResolvedValue({ viewer: "mock-dev", items: [item({ number: 1 })] });
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { client, wrapper } = clientWrapper();
    const { result } = renderHook(() => usePrOverview(true, "open", "page"), { wrapper });
    await waitFor(() => expect(result.current.rows).toHaveLength(1));

    expect(intervalOf(client, prOverviewKey(ROOT))).toBe(30_000);
    // The expensive half is slower even on the page: it is a second call
    // per root, and a check rollup ticking over on a row nobody has
    // opened yet is not what the page is for.
    await waitFor(() =>
      expect(intervalOf(client, prOverviewStatsKey(ROOT))).toBe(90_000),
    );
  });

  it("lets the unwatched watcher run slowly", async () => {
    mockListPrsOverview.mockResolvedValue({ viewer: "mock-dev", items: [item({ number: 1 })] });
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { client, wrapper } = clientWrapper();
    // No third argument: the badge, the toasts and the palette all take
    // the default, and the default is the cheap one.
    const { result } = renderHook(() => usePrOverview(true), { wrapper });
    await waitFor(() => expect(result.current.rows).toHaveLength(1));

    expect(intervalOf(client, prOverviewKey(ROOT))).toBe(120_000);
    await waitFor(() =>
      expect(intervalOf(client, prOverviewStatsKey(ROOT))).toBe(240_000),
    );
  });
});

// ── The budget gate ──────────────────────────────────────────────────

describe("usePrOverview — the budget gate", () => {
  const RESET_SEC = Math.floor(Date.now() / 1000) + 900;

  it("stops polling when the account's budget is spent, and says until when", async () => {
    setRoots(ROOT, OTHER);
    mockListPrsOverview.mockRejectedValue(
      "GraphQL: API rate limit already exceeded for user ID 100132710.",
    );
    mockListPrsOverviewStats.mockResolvedValue([]);
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: RESET_SEC,
      core_remaining: 4999,
      core_reset: RESET_SEC,
    });

    const { client, wrapper } = clientWrapper();
    const { result } = renderHook(() => usePrOverview(true), { wrapper });

    await waitFor(() => expect(result.current.rateLimitedUntil).toBe(RESET_SEC * 1000));

    // The budget belongs to the account, not to the repository whose
    // call happened to carry the refusal — so both roots are gated and
    // only one of them paid for the lookup.
    expect(mockGithubRateLimit).toHaveBeenCalledTimes(1);
    const query = client.getQueryCache().find({ queryKey: prOverviewKey(ROOT) });
    expect(query?.observers[0]?.options.enabled).toBe(false);
  });

  it("does not pause a repository metered by a different host", async () => {
    // The gate holds GitHub's hourly budget. A GitLab root draws on an
    // entirely separate one and has nothing to wait for — freezing its
    // badge, its toasts and its rows on GitHub's word would be a new
    // silent outage traded for the old one.
    mockWorkspaces.length = 0;
    mockWorkspaces.push({
      workspace_id: "ws-gh",
      project_root: ROOT,
      cwd: ROOT,
      provider_kind: "github",
    });
    mockWorkspaces.push({
      workspace_id: "ws-gl",
      project_root: OTHER,
      cwd: OTHER,
      provider_kind: "gitlab",
    });
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT
        ? Promise.reject("GraphQL: API rate limit already exceeded for user ID 1.")
        : Promise.resolve({ viewer: "mock-dev", items: [item({ number: 4 })] }),
    );
    mockListPrsOverviewStats.mockResolvedValue([]);
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: Math.floor(Date.now() / 1000) + 900,
      core_remaining: 1,
      core_reset: Math.floor(Date.now() / 1000) + 900,
    });

    const { client, wrapper } = clientWrapper();
    const { result } = renderHook(() => usePrOverview(true), { wrapper });
    await waitFor(() => expect(result.current.rateLimitedUntil).toBeGreaterThan(0));

    const gh = client.getQueryCache().find({ queryKey: prOverviewKey(ROOT) });
    const gl = client.getQueryCache().find({ queryKey: prOverviewKey(OTHER) });
    expect(gh?.observers[0]?.options.enabled).toBe(false);
    expect(gl?.observers[0]?.options.enabled).toBe(true);
    expect(result.current.rows.map((r) => r.number)).toEqual([4]);
  });

  it("does not raise the gate on a host it cannot speak for", async () => {
    // A GitLab root saying "rate limit" is reporting a budget this gate
    // does not hold and `gh api rate_limit` cannot read.
    mockWorkspaces.length = 0;
    mockWorkspaces.push({
      workspace_id: "ws-gl",
      project_root: OTHER,
      cwd: OTHER,
      provider_kind: "gitlab",
    });
    mockListPrsOverview.mockRejectedValue("429: API rate limit exceeded for this project");
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.failures).toHaveLength(1));

    expect(result.current.rateLimitedUntil).toBe(0);
    expect(mockGithubRateLimit).not.toHaveBeenCalled();
  });

  it("leaves an unreachable repository to the footer", async () => {
    // The narrowness of the match is what keeps one dead remote from
    // pausing every other repository along with it.
    setRoots(ROOT, OTHER);
    mockListPrsOverview.mockImplementation((path: string) =>
      path === ROOT
        ? Promise.resolve({ viewer: "mock-dev", items: [item({ number: 1 })] })
        : Promise.reject("could not resolve host github.com"),
    );
    mockListPrsOverviewStats.mockResolvedValue([]);

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });

    await waitFor(() => expect(result.current.failures).toHaveLength(1));
    expect(result.current.rateLimitedUntil).toBe(0);
    expect(mockGithubRateLimit).not.toHaveBeenCalled();
    expect(result.current.rows).toHaveLength(1);
  });

  it("keeps the rows it already had while it waits", async () => {
    // The gate stops fetching, never rendering: a page that emptied
    // itself because the budget ran out would be strictly worse than the
    // stale list it is replacing.
    mockListPrsOverview
      .mockResolvedValueOnce({ viewer: "mock-dev", items: [item({ number: 1 })] })
      .mockRejectedValue("GraphQL: API rate limit already exceeded for user ID 1.");
    mockListPrsOverviewStats.mockResolvedValue([]);
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: RESET_SEC,
      core_remaining: 1,
      core_reset: RESET_SEC,
    });

    const { client, wrapper } = clientWrapper();
    const { result } = renderHook(() => usePrOverview(true), { wrapper });
    await waitFor(() => expect(result.current.rows).toHaveLength(1));

    await act(async () => {
      await client.refetchQueries({ queryKey: prOverviewKey(ROOT) });
    });
    await waitFor(() => expect(result.current.rateLimitedUntil).toBeGreaterThan(0));

    expect(result.current.rows).toHaveLength(1);
    expect(result.current.rows[0]?.number).toBe(1);
  });

  it("lifts the gate when the user presses Retry, and recovers", async () => {
    mockListPrsOverview.mockRejectedValue(
      "GraphQL: API rate limit already exceeded for user ID 1.",
    );
    mockListPrsOverviewStats.mockResolvedValue([]);
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: RESET_SEC,
      core_remaining: 1,
      core_reset: RESET_SEC,
    });

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.rateLimitedUntil).toBeGreaterThan(0));

    // The budget refilled early, or the user simply disbelieves us.
    // Retry is allowed to find out, for the price of one request.
    mockListPrsOverview.mockResolvedValue({
      viewer: "mock-dev",
      items: [item({ number: 9 })],
    });
    act(() => result.current.refresh());

    await waitFor(() => expect(result.current.rows).toHaveLength(1));
    expect(result.current.rows[0]?.number).toBe(9);
    expect(result.current.rateLimitedUntil).toBe(0);
  });
});

describe("usePrOverview — the gate can be raised more than once", () => {
  it("goes back up when the budget is still spent after the pause lifts", async () => {
    // The failure this guards against is subtle and total: if the effect
    // that raises the gate keys off something that does not change
    // between two refusals, the gate goes up exactly once per session.
    // Every pause after the first one then lifts into a page that
    // resumes hammering a host still refusing it — which is the original
    // bug, with a fifteen-minute pause in front of it.
    const RESET_SEC = Math.floor(Date.now() / 1000) + 900;
    mockListPrsOverview.mockRejectedValue(
      "GraphQL: API rate limit already exceeded for user ID 1.",
    );
    mockListPrsOverviewStats.mockResolvedValue([]);
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: RESET_SEC,
      core_remaining: 1,
      core_reset: RESET_SEC,
    });

    const { result } = renderHook(() => usePrOverview(true), { wrapper: wrapper() });
    await waitFor(() => expect(result.current.rateLimitedUntil).toBeGreaterThan(0));
    expect(mockGithubRateLimit).toHaveBeenCalledTimes(1);

    // The pause lifting is what Retry does by hand, so this is the same
    // path the timer takes when it fires.
    act(() => result.current.refresh());

    await waitFor(() => expect(mockGithubRateLimit).toHaveBeenCalledTimes(2));
    expect(result.current.rateLimitedUntil).toBeGreaterThan(0);
  });

  it("goes back up when the timer lifts the pause, on a root that already has rows", async () => {
    // Two things at once, and the second is why the first is written
    // this way.
    //
    // The timer path: Retry also invalidates the queries, while the
    // timer only lifts the gate and leaves the re-fetch to the observers
    // re-enabling. That is the path that actually runs in production —
    // nobody is watching at 03:00 to press the button.
    //
    // And the root *succeeds first*, which is what makes this a real
    // test of the dependency. A query that has never held data has its
    // error cleared on every refetch, so the offending path goes null
    // and back and the effect re-fires on the path alone — the shape
    // every other test here has, and the shape the bug hides behind. A
    // root with rows keeps its error across refetches, so the timestamp
    // is genuinely the only thing that changes between two refusals.
    const RESET_SEC = Math.floor(Date.now() / 1000) + 900;
    mockListPrsOverview
      .mockResolvedValueOnce({ viewer: "mock-dev", items: [item({ number: 3 })] })
      .mockRejectedValue("GraphQL: API rate limit already exceeded for user ID 1.");
    mockListPrsOverviewStats.mockResolvedValue([]);
    mockGithubRateLimit.mockResolvedValue({
      graphql_remaining: 0,
      graphql_reset: RESET_SEC,
      core_remaining: 1,
      core_reset: RESET_SEC,
    });

    const { client, wrapper } = clientWrapper();
    const { result } = renderHook(() => usePrOverview(true), { wrapper });
    await waitFor(() => expect(result.current.rows).toHaveLength(1));

    await act(async () => {
      await client.refetchQueries({ queryKey: prOverviewKey(ROOT) });
    });
    await waitFor(() => expect(result.current.rateLimitedUntil).toBeGreaterThan(0));
    expect(mockGithubRateLimit).toHaveBeenCalledTimes(1);
    // The row survived the refusal, which is the state that makes the
    // second refusal indistinguishable from the first by path alone.
    expect(result.current.rows).toHaveLength(1);

    act(() => clearRateLimitPause());

    await waitFor(() => expect(mockGithubRateLimit).toHaveBeenCalledTimes(2));
    expect(result.current.rateLimitedUntil).toBeGreaterThan(0);
  });
});
