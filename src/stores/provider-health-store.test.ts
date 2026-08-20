// Pins the provider-health store's cache/recovery contract. The three
// behaviours that were wrong before and are easy to regress:
//
// 1. A FORCED probe issued while an unforced one is in flight must not
//    coalesce onto it. The forced call always reacts to an event the
//    running probe predates (a send just failed, a session just
//    succeeded), so coalescing answers with stale facts and the banner
//    never changes.
// 2. Recovery. A banner that only ever appears is worse than none:
//    a successful start/turn (`noteProviderSuccess`) and the mounted
//    surface's poll (`reprobeUnhealthy`) both have to be able to clear
//    a failure the user fixed.
// 3. `healthBannerKey` separates status from message with a real NUL,
//    written as the `\0` ESCAPE — a literal NUL byte in the source makes
//    git treat the whole store file as binary.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mockProbe = vi.fn();

vi.mock("@/tauri/commands", () => ({
  agentChatProviderHealth: (...args: unknown[]) => mockProbe(...args),
}));

import {
  emptyHealthSlot,
  healthBannerKey,
  selectVisibleHealthReport,
  useProviderHealth,
} from "./provider-health-store";
import type { ProviderHealthReport } from "@/tauri/types";

function report(
  status: ProviderHealthReport["status"],
  message: string | null = null,
): ProviderHealthReport {
  return {
    provider: "claude",
    status,
    installed: status !== "error",
    message,
    version: null,
  };
}

/** A promise plus its resolver, so a test can hold a probe in flight. */
function deferred() {
  let resolve!: (value: ProviderHealthReport) => void;
  const promise = new Promise<ProviderHealthReport>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

/** Seed a slot as if a probe had just returned `value`. */
function seed(value: ProviderHealthReport) {
  useProviderHealth.setState((state) => ({
    slots: {
      ...state.slots,
      claude: { ...state.slots.claude, report: value, fetchedAt: Date.now() },
    },
  }));
}

describe("provider-health-store", () => {
  beforeEach(() => {
    mockProbe.mockReset();
    useProviderHealth.setState({
      slots: {
        claude: emptyHealthSlot(),
        codex: emptyHealthSlot(),
        cursor: emptyHealthSlot(),
        opencode: emptyHealthSlot(),
      },
    });
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it("separates status from message with a NUL, not a printable char", () => {
    const key = healthBannerKey(report("error", "boom"));
    expect(key).toBe("error\0boom");
    // The separator must be impossible to forge from message text:
    // with a space, `error` + " boom" and `error ` + "boom" collide.
    expect(key).not.toContain(" ");
  });

  it("coalesces concurrent unforced refreshes onto one probe", async () => {
    const first = deferred();
    mockProbe.mockReturnValueOnce(first.promise);

    const a = useProviderHealth.getState().refresh("claude");
    const b = useProviderHealth.getState().refresh("claude");
    first.resolve(report("ready"));
    await Promise.all([a, b]);

    expect(mockProbe).toHaveBeenCalledTimes(1);
  });

  it("serves an unforced refresh from the TTL cache", async () => {
    mockProbe.mockResolvedValue(report("ready"));
    await useProviderHealth.getState().refresh("claude");
    await useProviderHealth.getState().refresh("claude");
    expect(mockProbe).toHaveBeenCalledTimes(1);

    await useProviderHealth.getState().refresh("claude", { force: true });
    expect(mockProbe).toHaveBeenCalledTimes(2);
  });

  it("queues a forced probe behind an in-flight one instead of dropping it", async () => {
    // The mount probe runs while the provider is still healthy; the user
    // then breaks it and a send fails. The forced post-failure probe MUST
    // run its own invoke, or the banner keeps showing the stale answer.
    const mount = deferred();
    mockProbe.mockReturnValueOnce(mount.promise);
    mockProbe.mockResolvedValueOnce(report("error", "Not authenticated."));

    const mountProbe = useProviderHealth.getState().refresh("claude");
    const forced = useProviderHealth
      .getState()
      .refresh("claude", { force: true });

    mount.resolve(report("ready"));
    await mountProbe;
    await forced;

    expect(mockProbe).toHaveBeenCalledTimes(2);
    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude")
        ?.message,
    ).toBe("Not authenticated.");
    expect(useProviderHealth.getState().slots.claude.queuedForce).toBeNull();
  });

  it("shares one follow-up across a burst of forced probes", async () => {
    const mount = deferred();
    mockProbe.mockReturnValueOnce(mount.promise);
    mockProbe.mockResolvedValue(report("error", "Not authenticated."));

    const mountProbe = useProviderHealth.getState().refresh("claude");
    const forced = [
      useProviderHealth.getState().refresh("claude", { force: true }),
      useProviderHealth.getState().refresh("claude", { force: true }),
      useProviderHealth.getState().refresh("claude", { force: true }),
    ];

    mount.resolve(report("ready"));
    await mountProbe;
    await Promise.all(forced);

    // One mount probe + exactly one follow-up, not one per caller.
    expect(mockProbe).toHaveBeenCalledTimes(2);
  });

  it("noteProviderSuccess clears a stale banner and re-probes", async () => {
    seed(report("error", "Claude CLI is not authenticated."));
    useProviderHealth.getState().dismiss("claude");
    mockProbe.mockResolvedValue(report("ready"));

    await useProviderHealth.getState().noteProviderSuccess("claude");

    expect(mockProbe).toHaveBeenCalledTimes(1);
    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude"),
    ).toBeNull();
    expect(useProviderHealth.getState().slots.claude.report?.status).toBe(
      "ready",
    );
    // The dismissal is retired too, so the NEXT failure banners again.
    expect(useProviderHealth.getState().slots.claude.dismissedKey).toBeNull();
  });

  it("noteProviderSuccess re-banners when the provider is still broken", async () => {
    seed(report("error", "Claude CLI is not authenticated."));
    mockProbe.mockResolvedValue(report("error", "Claude CLI is not installed."));

    await useProviderHealth.getState().noteProviderSuccess("claude");

    // We clear optimistically but never fabricate `ready`: the probe is
    // still the source of truth and puts the banner back.
    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude")
        ?.message,
    ).toBe("Claude CLI is not installed.");
  });

  it("noteProviderSuccess keeps a dismissal when the same warning returns", async () => {
    // A dismissed advisory must not come back on every successful send
    // just because the re-probe is still inconclusive.
    seed(report("warning", "Could not verify Claude authentication status."));
    useProviderHealth.getState().dismiss("claude");
    mockProbe.mockResolvedValue(
      report("warning", "Could not verify Claude authentication status."),
    );

    await useProviderHealth.getState().noteProviderSuccess("claude");

    expect(mockProbe).toHaveBeenCalledTimes(1);
    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude"),
    ).toBeNull();
  });

  it("keeps a dismissal narrow: a different failure still banners", async () => {
    // The counterweight to the test above — the dismissal survives a
    // successful send, but it must stay scoped to the failure the user
    // actually closed, or a real new problem goes unreported.
    seed(report("warning", "Could not verify Claude authentication status."));
    useProviderHealth.getState().dismiss("claude");
    mockProbe.mockResolvedValue(report("error", "Claude CLI is not installed."));

    await useProviderHealth.getState().noteProviderSuccess("claude");

    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude")
        ?.message,
    ).toBe("Claude CLI is not installed.");
  });

  it("retires a dismissal on recovery so the same failure banners again", async () => {
    // Recovery is the ONLY thing that clears a dismissal now, so pin it:
    // fail → dismiss → recover → fail identically → visible again.
    const failure = report("warning", "Could not verify Claude auth.");
    seed(failure);
    useProviderHealth.getState().dismiss("claude");

    mockProbe.mockResolvedValueOnce(report("ready"));
    await useProviderHealth.getState().refresh("claude", { force: true });
    expect(useProviderHealth.getState().slots.claude.dismissedKey).toBeNull();

    mockProbe.mockResolvedValueOnce(failure);
    await useProviderHealth.getState().refresh("claude", { force: true });
    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude")
        ?.message,
    ).toBe("Could not verify Claude auth.");
  });

  it("noteProviderSuccess costs nothing on the healthy path", async () => {
    seed(report("ready"));
    await useProviderHealth.getState().noteProviderSuccess("claude");
    expect(mockProbe).not.toHaveBeenCalled();
  });

  it("reprobeUnhealthy polls only a failing provider", async () => {
    mockProbe.mockResolvedValue(report("ready"));

    // Healthy (and unprobed) providers are not worth a CLI spawn.
    await useProviderHealth.getState().reprobeUnhealthy("claude");
    expect(mockProbe).not.toHaveBeenCalled();
    seed(report("ready"));
    await useProviderHealth.getState().reprobeUnhealthy("claude");
    expect(mockProbe).not.toHaveBeenCalled();

    // A failing one recovers on its own once the user fixes it.
    seed(report("error", "Claude CLI is not authenticated."));
    await useProviderHealth.getState().reprobeUnhealthy("claude");
    expect(mockProbe).toHaveBeenCalledTimes(1);
    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude"),
    ).toBeNull();
  });

  it("reprobeUnhealthy coalesces onto an in-flight probe", async () => {
    const inFlight = deferred();
    mockProbe.mockReturnValueOnce(inFlight.promise);
    seed(report("error", "Claude CLI is not authenticated."));

    const running = useProviderHealth
      .getState()
      .refresh("claude", { force: true });
    const polled = useProviderHealth.getState().reprobeUnhealthy("claude");

    inFlight.resolve(report("ready"));
    await Promise.all([running, polled]);

    // The poll has no triggering event, so the running probe's answer is
    // already good enough — no second spawn.
    expect(mockProbe).toHaveBeenCalledTimes(1);
  });

  it("keeps the last report when the probe command itself rejects", async () => {
    seed(report("error", "Claude CLI is not authenticated."));
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    mockProbe.mockRejectedValue(new Error("IPC down"));

    await useProviderHealth.getState().refresh("claude", { force: true });

    // "Probe-backed fact" is the banner's contract — an IPC hiccup is
    // not a provider failure and must not synthesize one.
    expect(
      selectVisibleHealthReport(useProviderHealth.getState(), "claude")
        ?.message,
    ).toBe("Claude CLI is not authenticated.");
    warn.mockRestore();
  });
});
