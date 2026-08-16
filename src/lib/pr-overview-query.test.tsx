import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { renderHook, waitFor, cleanup } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";

const mockListPrsOverview = vi.fn();
const mockListPrsOverviewStats = vi.fn();
const mockListPullRequests = vi.fn().mockResolvedValue([]);

vi.mock("@/tauri/commands", () => ({
  listPrsOverview: (...a: unknown[]) => mockListPrsOverview(...a),
  listPrsOverviewStats: (...a: unknown[]) => mockListPrsOverviewStats(...a),
  listPullRequests: (...a: unknown[]) => mockListPullRequests(...a),
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
  _resetSnapshotWriteGuard,
} from "./pr-overview-query";
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
