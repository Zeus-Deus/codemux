import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import { useChatGptStore } from "./chatgpt-store";
import { useProviderCapabilities } from "./provider-capabilities-store";
import { useAuthStore } from "./auth-store";
import type { ChatGptStatus } from "@/tauri/chatgpt";
import type { ProviderChatCapabilities } from "@/tauri/types";

const rpc = vi.hoisted(() => ({ get: vi.fn(), start: vi.fn(), cancel: vi.fn(), disconnect: vi.fn(), acknowledge: vi.fn(), getLocal: vi.fn(), setLocal: vi.fn(), catalog: vi.fn() }));
vi.mock("@/tauri/chatgpt", () => ({ getChatGptStatus: rpc.get, startChatGptLogin: rpc.start,
  cancelChatGptLogin: rpc.cancel, disconnectChatGpt: rpc.disconnect, acknowledgeChatGptWelcome: rpc.acknowledge,
  getLocalWorkbench: rpc.getLocal, setLocalWorkbench: rpc.setLocal }));
vi.mock("@/tauri/commands", () => ({ agentChatProviderHealth: vi.fn().mockResolvedValue({ provider: "codex", installed: true, status: "ready", message: null, version: null }), listChatProviderCapabilities: rpc.catalog }));
const disconnected: ChatGptStatus = { phase: "disconnected", attemptId: null, email: null, error: null,
  profiles: [], activeProfileId: null, welcomePending: false, installed: true };
const connected: ChatGptStatus = { ...disconnected, phase: "connected", activeProfileId: "connection", email: "builder@example.com" };
const pending: ChatGptStatus = { ...disconnected, phase: "pending", attemptId: "attempt" };
const caps: ProviderChatCapabilities = { models: [], effort_granularity: "per_turn", effort_label_map: {}, permission_modes: [], default_permission_mode: null, permission_granularity: "per_session" };
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>((done) => { resolve = done; }); return { promise, resolve }; }
beforeEach(() => {
  vi.clearAllMocks();
  rpc.get.mockResolvedValue(disconnected); rpc.getLocal.mockResolvedValue(false);
  rpc.cancel.mockResolvedValue(disconnected); rpc.start.mockResolvedValue(pending);
  rpc.disconnect.mockResolvedValue(disconnected); rpc.setLocal.mockResolvedValue(undefined);
  rpc.catalog.mockResolvedValue(caps);
  useChatGptStore.setState({ status: disconnected, localMode: false, cloudLogin: false, busy: false, loading: false, ready: false, error: null, subscriptionError: null });
  useProviderCapabilities.setState({ claude: null, codex: null });
  (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = false;
});
afterEach(() => { (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = false; });
describe("ChatGPT connection ownership", () => {
  it("invalidates Codex metadata without dropping another provider's catalog", async () => {
    useChatGptStore.setState({ status: connected });
    useProviderCapabilities.setState({ claude: caps, codex: caps });
    await useChatGptStore.getState().disconnect();
    expect(useProviderCapabilities.getState().codex).toBeNull();
    expect(useProviderCapabilities.getState().claude).toEqual(caps);
  });

  it("coalesces a status invalidation during bootstrap without losing local access", async () => {
    const first = deferred<ChatGptStatus>();
    rpc.get.mockReturnValueOnce(first.promise).mockResolvedValue(connected);
    rpc.getLocal.mockResolvedValue(true);
    const boot = useChatGptStore.getState().bootstrap();
    const refresh = useChatGptStore.getState().refresh();
    try { expect(rpc.get).toHaveBeenCalledTimes(1); }
    finally { first.resolve(connected); await Promise.all([boot, refresh]); }
    expect(useChatGptStore.getState().localMode).toBe(true);
    expect(useChatGptStore.getState().status?.phase).toBe("connected");
  });

  it("does not revive a cancelled attempt from an older read", async () => {
    const oldRead = deferred<ChatGptStatus>();
    useChatGptStore.setState({ status: pending });
    rpc.get.mockReturnValueOnce(oldRead.promise);
    const refresh = useChatGptStore.getState().refresh();
    await useChatGptStore.getState().cancel();
    oldRead.resolve(pending);
    await refresh;
    expect(rpc.cancel).toHaveBeenCalledWith("attempt");
    expect(useChatGptStore.getState().status?.phase).toBe("disconnected");
  });

  it("requires a successful native write before allowing local access", async () => {
    rpc.setLocal.mockRejectedValueOnce(new Error("Could not save local mode"));
    await useChatGptStore.getState().enterLocal();
    expect(useChatGptStore.getState().localMode).toBe(false);
    expect(useChatGptStore.getState().error).toBe("Could not save local mode");
  });

  it("keeps ChatGPT plan metadata separate from cloud authentication", async () => {
    rpc.get.mockResolvedValue(connected);
    const cloudBefore = useAuthStore.getState();
    await useChatGptStore.getState().bootstrap();
    const cloudAfter = useAuthStore.getState();
    expect(cloudAfter.user).toBe(cloudBefore.user);
    expect(cloudAfter.isAuthenticated).toBe(cloudBefore.isAuthenticated);
    expect(cloudAfter.syncAvailable).toBe(cloudBefore.syncAvailable);
    expect(useChatGptStore.getState().localMode).toBe(false);
  });

  it("does not let remote clients read or change desktop credentials", async () => {
    (window as { __CODEMUX_REMOTE__?: boolean }).__CODEMUX_REMOTE__ = true;
    await useChatGptStore.getState().bootstrap();
    await useChatGptStore.getState().start(null);
    await useChatGptStore.getState().disconnect();
    await useChatGptStore.getState().enterLocal();
    expect(rpc.get).not.toHaveBeenCalled();
    expect(rpc.start).not.toHaveBeenCalled();
    expect(rpc.disconnect).not.toHaveBeenCalled();
    expect(rpc.setLocal).not.toHaveBeenCalled();
  });

  it("prevents duplicate browser launch while the native start is pending", async () => {
    const started = deferred<ChatGptStatus>();
    rpc.start.mockReturnValueOnce(started.promise);
    const first = useChatGptStore.getState().start(null);
    await useChatGptStore.getState().start(null);
    expect(rpc.start).toHaveBeenCalledTimes(1);
    started.resolve(pending);
    await first;
    await useChatGptStore.getState().start(null);
    expect(rpc.start).toHaveBeenCalledTimes(1);
  });

  it("applies slow status reads instead of discarding them on every polling tick", async () => {
    const slow = deferred<ChatGptStatus>();
    useChatGptStore.setState({ status: pending });
    rpc.get.mockReturnValueOnce(slow.promise).mockResolvedValue(connected);
    const first = useChatGptStore.getState().refresh();
    const second = useChatGptStore.getState().refresh();
    const third = useChatGptStore.getState().refresh();
    try { expect(rpc.get).toHaveBeenCalledTimes(1); }
    finally { slow.resolve(connected); await Promise.all([first, second, third]); }
    expect(useChatGptStore.getState().status?.phase).toBe("connected");
  });

  it("retains status invalidations received while a native mutation is busy", async () => {
    const begun = deferred<ChatGptStatus>();
    rpc.start.mockReturnValueOnce(begun.promise);
    rpc.get.mockResolvedValue(connected);
    const start = useChatGptStore.getState().start(null);
    await useChatGptStore.getState().refresh();
    begun.resolve(pending);
    await start;
    await waitFor(() => expect(rpc.get).toHaveBeenCalled());
    expect(useChatGptStore.getState().status?.phase).toBe("connected");
  });

  it("keeps account bootstrap when local continuation writes only its preference", async () => {
    const first = deferred<ChatGptStatus>();
    rpc.get.mockReturnValueOnce(first.promise);
    const boot = useChatGptStore.getState().bootstrap();
    await useChatGptStore.getState().enterLocal();
    first.resolve(connected);
    await boot;
    expect(useChatGptStore.getState().localMode).toBe(true);
    expect(useChatGptStore.getState().status?.phase).toBe("connected");
  });

  it("invalidates an unqualified persisted catalog on the first managed snapshot without launching a CLI", async () => {
    useChatGptStore.setState({ status: null });
    useProviderCapabilities.setState({ codex: caps });
    rpc.get.mockResolvedValue(connected);
    await useChatGptStore.getState().bootstrap();
    expect(useProviderCapabilities.getState().codex).toBeNull();
    expect(rpc.catalog).not.toHaveBeenCalled();
  });

  it("reharvests Codex after an explicitly started login completes so an open picker recovers", async () => {
    await useChatGptStore.getState().start(null);
    rpc.get.mockResolvedValue(connected);
    await useChatGptStore.getState().refresh();
    await waitFor(() => expect(rpc.catalog).toHaveBeenCalledWith("codex"));
    await waitFor(() => expect(useProviderCapabilities.getState().loadedProviders.codex).toBe(true));
    expect(useProviderCapabilities.getState().codex).toEqual(caps);
  });
});
