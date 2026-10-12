import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { AuthStatePayload } from "@/tauri/types";

const events = vi.hoisted(() => ({
  authHandler: null as ((payload: AuthStatePayload) => void) | null,
}));

vi.mock("@/tauri/events", () => ({
  onAuthStateChanged: (cb: (payload: AuthStatePayload) => void) => {
    events.authHandler = cb;
    return Promise.resolve(() => {});
  },
  onSettingsSynced: () => Promise.resolve(() => {}),
  onSyncStateChanged: () => Promise.resolve(() => {}),
}));

vi.mock("@/tauri/commands", () => ({
  bootstrapSession: vi.fn(),
  refreshSession: vi.fn(),
  startOauthFlow: vi.fn(),
  signinEmail: vi.fn(),
  signupEmail: vi.fn(),
  signOut: vi.fn(),
  getSyncStatus: vi.fn(),
  listChatProviderCapabilities: vi.fn(),
  agentChatProviderHealth: vi.fn(),
}));

import { useAuthEvents } from "./use-auth-events";
import {
  OAUTH_CALLBACK_GRACE_MS,
  OAUTH_TIMEOUT_MS,
  useAuthStore,
} from "@/stores/auth-store";
import { startOauthFlow } from "@/tauri/commands";

const bootstrapSession = vi.fn();

/** The backend saved the callback's token but could not verify it yet. */
const unverifiedCallback: AuthStatePayload = {
  authenticated: false,
  user: null,
};

function emitAuthState(payload: AuthStatePayload) {
  act(() => events.authHandler?.(payload));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  vi.mocked(startOauthFlow).mockResolvedValue(undefined);
  useAuthStore.setState({
    isLoading: false,
    user: null,
    isAuthenticated: false,
    oauthPending: false,
    oauthCallbackExpected: false,
    error: null,
    bootstrapSession,
  });
  renderHook(() => useAuthEvents());
});

afterEach(() => {
  useAuthStore.getState().finishOAuthFlow();
  cleanup();
  vi.useRealTimers();
});

describe("useAuthEvents GitHub callback", () => {
  it("ends the GitHub wait when the callback arrives", async () => {
    await useAuthStore.getState().startOAuthFlow();

    emitAuthState(unverifiedCallback);

    expect(bootstrapSession).toHaveBeenCalledTimes(1);
    expect(useAuthStore.getState().oauthPending).toBe(false);
    expect(useAuthStore.getState().oauthCallbackExpected).toBe(false);
  });

  it("still bootstraps a callback finished after Cancel", async () => {
    await useAuthStore.getState().startOAuthFlow();
    useAuthStore.getState().cancelOAuthFlow();

    emitAuthState(unverifiedCallback);

    expect(bootstrapSession).toHaveBeenCalledTimes(1);
  });

  it("still bootstraps a callback that trails the UI timeout", async () => {
    await useAuthStore.getState().startOAuthFlow();
    vi.advanceTimersByTime(OAUTH_TIMEOUT_MS + 10_000);
    expect(useAuthStore.getState().oauthPending).toBe(false);

    emitAuthState(unverifiedCallback);

    expect(bootstrapSession).toHaveBeenCalledTimes(1);
  });

  it("treats a sign-out event as definitive once no callback can arrive", async () => {
    await useAuthStore.getState().startOAuthFlow();
    vi.advanceTimersByTime(OAUTH_TIMEOUT_MS + OAUTH_CALLBACK_GRACE_MS);

    emitAuthState(unverifiedCallback);

    expect(bootstrapSession).not.toHaveBeenCalled();
  });

  it("does not bootstrap a sign-out event when GitHub was never started", () => {
    emitAuthState(unverifiedCallback);

    expect(bootstrapSession).not.toHaveBeenCalled();
  });
});
