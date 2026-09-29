import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import { QueryClient, QueryClientProvider, useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { PR_POLL_IDLE_MS, usePrPollingActive } from "./use-pr-polling-active";

let client: QueryClient;
let visibility: "visible" | "hidden";

beforeEach(() => {
  vi.useFakeTimers();
  visibility = "visible";
  vi.spyOn(document, "visibilityState", "get").mockImplementation(() => visibility);
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
});

afterEach(() => {
  cleanup();
  client.clear();
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function wrapper({ children }: { children: ReactNode }) {
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

async function advance(ms: number) {
  await act(async () => { await vi.advanceTimersByTimeAsync(ms); });
}

function usePolling(read: () => Promise<string>) {
  const active = usePrPollingActive();
  const query = useQuery({
    queryKey: ["test-pr"],
    queryFn: read,
    enabled: active,
    staleTime: 30_000,
    refetchInterval: 30_000,
    refetchOnWindowFocus: false,
  });
  return { active, data: query.data };
}

describe("PR polling activity", () => {
  it("stops real query reads while hidden, retains data, and refreshes once on return", async () => {
    const read = vi.fn(async () => "loaded");
    const { result } = renderHook(() => usePolling(read), { wrapper });
    await advance(1);
    expect(result.current.data).toBe("loaded");
    await advance(30_000);
    expect(read).toHaveBeenCalledTimes(2);

    act(() => {
      visibility = "hidden";
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await advance(120_000);
    expect(read).toHaveBeenCalledTimes(2);
    expect(result.current.data).toBe("loaded");

    act(() => {
      visibility = "visible";
      document.dispatchEvent(new Event("visibilitychange"));
    });
    await advance(1);
    expect(read).toHaveBeenCalledTimes(3);
    act(() => window.dispatchEvent(new Event("focus")));
    await advance(1);
    expect(read).toHaveBeenCalledTimes(3);
  });

  it("stops unattended visible reads and resumes on interaction", async () => {
    const read = vi.fn(async () => "loaded");
    const { result } = renderHook(() => usePolling(read), { wrapper });
    await advance(PR_POLL_IDLE_MS + 1);
    expect(result.current.active).toBe(false);
    const beforeIdle = read.mock.calls.length;
    await advance(60 * 60_000);
    expect(read).toHaveBeenCalledTimes(beforeIdle);

    act(() => document.dispatchEvent(new Event("keydown")));
    await advance(1);
    expect(result.current.active).toBe(true);
    expect(read).toHaveBeenCalledTimes(beforeIdle + 1);
  });

  it("does not restart the shared idle clock when another surface mounts", async () => {
    const read = vi.fn(async () => "loaded");
    const first = renderHook(() => usePolling(read), { wrapper });
    await advance(PR_POLL_IDLE_MS - 30_000);
    const second = renderHook(() => usePolling(read), { wrapper });
    await advance(30_001);
    expect(first.result.current.active).toBe(false);
    expect(second.result.current.active).toBe(false);
    const beforeIdle = read.mock.calls.length;
    await advance(5 * 60_000);
    expect(read).toHaveBeenCalledTimes(beforeIdle);
    first.unmount();
    act(() => document.dispatchEvent(new Event("wheel")));
    await advance(1);
    expect(second.result.current.active).toBe(true);
    expect(second.result.current.data).toBe("loaded");
    expect(read).toHaveBeenCalledTimes(beforeIdle + 1);
  });

  it("does not start a request when mounted in a hidden window", async () => {
    visibility = "hidden";
    const read = vi.fn(async () => "loaded");
    renderHook(() => usePolling(read), { wrapper });
    await advance(120_000);
    expect(read).not.toHaveBeenCalled();
  });
});
