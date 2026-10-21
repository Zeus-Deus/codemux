import { act, cleanup, renderHook } from "@testing-library/react";
import { StrictMode, useEffect } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Event } from "@tauri-apps/api/event";
import type { SkillsSyncStateSnapshot } from "@/tauri/commands";

vi.mock("@/tauri/commands", () => ({
  skillsSyncNow: vi.fn(),
  skillsSyncStatus: vi.fn(),
}));

vi.mock("@tauri-apps/api/event", () => ({ listen: vi.fn() }));

import { listen } from "@tauri-apps/api/event";
import { skillsSyncNow, skillsSyncStatus } from "@/tauri/commands";
import { useSkillsSyncStatus } from "./use-skills-sync-status";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((done, fail) => {
    resolve = done;
    reject = fail;
  });
  return { promise, resolve, reject };
}

const idle: SkillsSyncStateSnapshot = { state: "idle", lastSyncAtMillis: null };
const completed = {
  pushedCount: 0,
  pulledCount: 0,
  conflictCount: 0,
  errorCount: 0,
};

function emitStatus(payload: SkillsSyncStateSnapshot) {
  const calls = vi.mocked(listen).mock.calls;
  const callback = calls[calls.length - 1]?.[1];
  if (!callback) throw new Error("Missing skills sync listener");
  const event: Event<SkillsSyncStateSnapshot> = {
    event: "skills-sync-state-changed", id: 1, payload,
  };
  act(() => {
    callback(event);
  });
}

describe("useSkillsSyncStatus", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    vi.mocked(listen).mockResolvedValue(() => {});
    vi.mocked(skillsSyncStatus).mockResolvedValue(idle);
    vi.mocked(skillsSyncNow).mockResolvedValue(completed);
    vi.spyOn(console, "warn").mockImplementation(() => {});
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it("clears optimism when sync rejects without a completion event", async () => {
    const pending = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    const reconciliation = deferred<SkillsSyncStateSnapshot>();
    vi.mocked(skillsSyncNow).mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});
    vi.mocked(skillsSyncStatus).mockReturnValueOnce(reconciliation.promise);

    let syncing!: Promise<void>;
    act(() => { syncing = result.current.syncNow(); });
    expect(result.current.isSyncing).toBe(true);

    await act(async () => {
      pending.reject(new Error("IPC unavailable"));
    });

    // The command has settled even though its status read is still pending.
    expect(result.current.isSyncing).toBe(false);
    await act(async () => {
      reconciliation.resolve(idle);
      await syncing;
    });
  });

  it("reconciles a missed completion event after sync rejects", async () => {
    const pending = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    vi.mocked(skillsSyncNow).mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});

    let syncing!: Promise<void>;
    act(() => { syncing = result.current.syncNow(); });
    emitStatus({ state: "syncing", startedAtMillis: 100 });
    vi.mocked(skillsSyncStatus).mockResolvedValueOnce({
      state: "error", lastError: "Network unavailable", atMillis: 200,
    });

    await act(async () => {
      pending.reject(new Error("Network unavailable"));
      await syncing;
    });

    expect(result.current.isSyncing).toBe(false);
    expect(result.current.status).toEqual({
      state: "error", lastSyncAt: null, startedAt: null,
      lastError: "Network unavailable", errorAt: new Date(200),
    });
  });

  it("recovers when a successful or skipped sync emits no events", async () => {
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});
    vi.mocked(skillsSyncStatus).mockResolvedValueOnce({
      state: "idle", lastSyncAtMillis: 300,
    });

    await act(async () => { await result.current.syncNow(); });

    expect(result.current.isSyncing).toBe(false);
    expect(result.current.status?.lastSyncAt).toEqual(new Date(300));
    expect(skillsSyncStatus).toHaveBeenCalledTimes(2);
  });

  it("keeps optimism while the command is pending despite a terminal event", async () => {
    const pending = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    vi.mocked(skillsSyncNow).mockReturnValueOnce(pending.promise);
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});

    let syncing!: Promise<void>;
    act(() => { syncing = result.current.syncNow(); });
    emitStatus(idle);
    expect(result.current.isSyncing).toBe(true);

    await act(async () => {
      pending.resolve(completed);
      await syncing;
    });
    expect(result.current.isSyncing).toBe(false);
  });

  it("does not overwrite a live update with the earlier initial snapshot", async () => {
    const initial = deferred<SkillsSyncStateSnapshot>();
    vi.mocked(skillsSyncStatus).mockReturnValueOnce(initial.promise);
    const { result } = renderHook(() => useSkillsSyncStatus());

    emitStatus({ state: "syncing", startedAtMillis: 100 });
    await act(async () => { initial.resolve(idle); });

    expect(result.current.status?.startedAt).toEqual(new Date(100));
    expect(result.current.isSyncing).toBe(true);
  });

  it("does not overwrite reconciliation with the earlier initial snapshot", async () => {
    const initial = deferred<SkillsSyncStateSnapshot>();
    vi.mocked(skillsSyncStatus).mockReturnValueOnce(initial.promise);
    const { result } = renderHook(() => useSkillsSyncStatus());

    await act(async () => { await result.current.syncNow(); });
    await act(async () => {
      initial.resolve({ state: "syncing", startedAtMillis: 100 });
    });

    expect(result.current.status?.state).toBe("idle");
    expect(result.current.isSyncing).toBe(false);
  });

  it.each<{
    live: SkillsSyncStateSnapshot;
    stale: SkillsSyncStateSnapshot;
    isSyncing: boolean;
  }>([
    {
      live: { state: "syncing", startedAtMillis: 200 },
      stale: idle,
      isSyncing: true,
    },
    {
      live: { state: "idle", lastSyncAtMillis: 200 },
      stale: { state: "syncing", startedAtMillis: 100 },
      isSyncing: false,
    },
    {
      live: { state: "error", lastError: "New failure", atMillis: 200 },
      stale: idle,
      isSyncing: false,
    },
  ])("retains a live $live.state event received during reconciliation", async ({ live, stale, isSyncing }) => {
    const reconciliation = deferred<SkillsSyncStateSnapshot>();
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});
    vi.mocked(skillsSyncStatus).mockReturnValueOnce(reconciliation.promise);

    let syncing!: Promise<void>;
    await act(async () => { syncing = result.current.syncNow(); });
    expect(skillsSyncStatus).toHaveBeenCalledTimes(2);
    emitStatus(live);
    const liveStatus = result.current.status;
    await act(async () => {
      reconciliation.resolve(stale);
      await syncing;
    });

    expect(result.current.status).toEqual(liveStatus);
    expect(result.current.isSyncing).toBe(isSyncing);
  });

  it("retains the newer reconciliation when status reads finish out of order", async () => {
    const earlier = deferred<SkillsSyncStateSnapshot>();
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});
    vi.mocked(skillsSyncStatus).mockReturnValueOnce(earlier.promise);

    let firstSync!: Promise<void>;
    await act(async () => { firstSync = result.current.syncNow(); });
    vi.mocked(skillsSyncStatus).mockResolvedValueOnce({
      state: "idle", lastSyncAtMillis: 300,
    });
    await act(async () => { await result.current.syncNow(); });
    await act(async () => {
      earlier.resolve({ state: "syncing", startedAtMillis: 100 });
      await firstSync;
    });

    expect(result.current.status?.lastSyncAt).toEqual(new Date(300));
    expect(result.current.isSyncing).toBe(false);
  });

  it("stays busy when reconciliation reports another sync in progress", async () => {
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});
    vi.mocked(skillsSyncStatus).mockResolvedValueOnce({
      state: "syncing", startedAtMillis: 200,
    });

    await act(async () => { await result.current.syncNow(); });

    expect(result.current.isSyncing).toBe(true);
    expect(result.current.status?.startedAt).toEqual(new Date(200));
  });

  it("clears optimism even when both the command and reconciliation fail", async () => {
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});
    vi.mocked(skillsSyncNow).mockRejectedValueOnce(new Error("IPC unavailable"));
    vi.mocked(skillsSyncStatus).mockRejectedValueOnce(new Error("IPC unavailable"));

    await act(async () => { await result.current.syncNow(); });

    expect(result.current.isSyncing).toBe(false);
    expect(result.current.status?.state).toBe("idle");
  });

  it("tracks all overlapping manual commands until each settles", async () => {
    const first = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    const second = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    vi.mocked(skillsSyncNow).mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    const { result } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});

    let firstSync!: Promise<void>;
    let secondSync!: Promise<void>;
    act(() => {
      firstSync = result.current.syncNow();
      secondSync = result.current.syncNow();
    });
    await act(async () => {
      first.resolve(completed);
      await firstSync;
    });
    expect(result.current.isSyncing).toBe(true);

    await act(async () => {
      second.resolve(completed);
      await secondSync;
    });
    expect(result.current.isSyncing).toBe(false);
  });

  it("does not reconcile a command that settles after unmount", async () => {
    const pending = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    vi.mocked(skillsSyncNow).mockReturnValueOnce(pending.promise);
    const { result, unmount } = renderHook(() => useSkillsSyncStatus());
    await act(async () => {});
    let syncing!: Promise<void>;
    act(() => { syncing = result.current.syncNow(); });
    unmount();

    await act(async () => {
      pending.reject(new Error("Late failure"));
      await syncing;
    });

    expect(skillsSyncStatus).toHaveBeenCalledTimes(1);
  });

  it("ignores an initial fetch from the previous Strict Mode lifecycle", async () => {
    const previous = deferred<SkillsSyncStateSnapshot>();
    const current = deferred<SkillsSyncStateSnapshot>();
    vi.mocked(skillsSyncStatus).mockReturnValueOnce(previous.promise).mockReturnValueOnce(current.promise);
    const { result } = renderHook(() => useSkillsSyncStatus(), { wrapper: StrictMode });
    expect(skillsSyncStatus).toHaveBeenCalledTimes(2);

    await act(async () => {
      previous.resolve({ state: "syncing", startedAtMillis: 100 });
    });
    expect(result.current.status).toBeNull();

    await act(async () => { current.resolve(idle); });
    expect(result.current.status?.state).toBe("idle");
    expect(result.current.isSyncing).toBe(false);
  });

  it("ignores pending commands from the previous Strict Mode lifecycle", async () => {
    const previous = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    const current = deferred<Awaited<ReturnType<typeof skillsSyncNow>>>();
    vi.mocked(skillsSyncNow).mockReturnValueOnce(previous.promise).mockReturnValueOnce(current.promise);
    const { result } = renderHook(() => {
      const sync = useSkillsSyncStatus();
      useEffect(() => { void sync.syncNow(); }, [sync.syncNow]);
      return sync;
    }, { wrapper: StrictMode });
    await act(async () => {});
    expect(skillsSyncNow).toHaveBeenCalledTimes(2);

    await act(async () => { previous.resolve(completed); });
    expect(skillsSyncStatus).toHaveBeenCalledTimes(2);
    expect(result.current.isSyncing).toBe(true);

    await act(async () => { current.resolve(completed); });
    expect(skillsSyncStatus).toHaveBeenCalledTimes(3);
    expect(result.current.isSyncing).toBe(false);
  });
});
