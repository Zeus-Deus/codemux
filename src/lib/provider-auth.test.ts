import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockCheckProviderAuth = vi.fn();

vi.mock("@/tauri/commands", () => ({
  checkProviderAuth: (...args: unknown[]) => mockCheckProviderAuth(...args),
}));

import {
  NO_OPERATIONS,
  PROVIDER_AUTH_TTL_MS,
  _resetProviderAuthCache,
  fetchProviderAuth,
  getCachedProviderAuth,
} from "./provider-auth";

function ready(over: Record<string, unknown> = {}) {
  return {
    kind: "gitlab",
    supported: true,
    installed: true,
    authenticated: true,
    username: "root",
    ...over,
  };
}

beforeEach(() => {
  _resetProviderAuthCache();
  mockCheckProviderAuth.mockReset();
  mockCheckProviderAuth.mockResolvedValue(ready());
});

afterEach(() => {
  vi.useRealTimers();
});

describe("fetchProviderAuth", () => {
  it("probes the path it was given", async () => {
    await fetchProviderAuth("/repo", "gitlab");
    expect(mockCheckProviderAuth).toHaveBeenCalledWith("/repo");
  });

  it("serves a usable verdict from cache within the TTL", async () => {
    await fetchProviderAuth("/repo", "gitlab");
    await fetchProviderAuth("/repo", "gitlab");
    expect(mockCheckProviderAuth).toHaveBeenCalledTimes(1);
  });

  it("expires a cached verdict once the TTL lapses", async () => {
    vi.useFakeTimers();
    await fetchProviderAuth("/repo", "gitlab");
    vi.advanceTimersByTime(PROVIDER_AUTH_TTL_MS + 1);
    expect(getCachedProviderAuth("/repo", "gitlab")).toBeNull();
  });

  /// A signed-out or unsupported answer is the state the user is about to
  /// fix. Caching it would hide the recovery for a minute — the same rule
  /// the review panel's repo check has always followed.
  it("never caches an unusable verdict", async () => {
    mockCheckProviderAuth.mockResolvedValue(
      ready({ authenticated: false, username: null }),
    );
    await fetchProviderAuth("/repo", "gitlab");
    expect(getCachedProviderAuth("/repo", "gitlab")).toBeNull();

    mockCheckProviderAuth.mockResolvedValue(ready());
    await fetchProviderAuth("/repo", "gitlab");
    expect(mockCheckProviderAuth).toHaveBeenCalledTimes(2);
    expect(getCachedProviderAuth("/repo", "gitlab")?.authenticated).toBe(true);
  });

  it("keeps two paths, and two products on one path, apart", async () => {
    await fetchProviderAuth("/repo-a", "gitlab");
    expect(getCachedProviderAuth("/repo-b", "gitlab")).toBeNull();
    expect(getCachedProviderAuth("/repo-a", "github")).toBeNull();
    expect(getCachedProviderAuth("/repo-a", "gitlab")).not.toBeNull();
  });

  it("degrades to nothing-usable rather than rejecting", async () => {
    mockCheckProviderAuth.mockRejectedValue("backend exploded");
    await expect(fetchProviderAuth("/repo")).resolves.toEqual({
      kind: "unknown",
      supported: false,
      installed: false,
      authenticated: false,
      username: null,
      // A probe that failed knows nothing about what the host can do,
      // and the backend refuses undeclared operations — so this is the
      // safe answer, not a degraded one.
      operations: NO_OPERATIONS,
    });
  });
});
