import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { act, cleanup, renderHook } from "@testing-library/react";
import type { AuthStatePayload } from "@/tauri/types";

const events = vi.hoisted(() => ({
  authHandler: null as ((payload: AuthStatePayload) => void) | null,
  storageErrorClearedHandler: null as ((clearedError: string) => void) | null,
}));

vi.mock("@/tauri/events", () => ({
  onAuthStateChanged: (cb: (payload: AuthStatePayload) => void) => {
    events.authHandler = cb;
    return Promise.resolve(() => {});
  },
  onAuthStorageErrorCleared: (cb: (clearedError: string) => void) => {
    events.storageErrorClearedHandler = cb;
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
import { bootstrapSession as bootstrapSessionCmd, startOauthFlow } from "@/tauri/commands";
import { DEFAULT_SETTINGS, useSyncedSettingsStore } from "@/stores/synced-settings-store";

const storeBootstrapSession = useAuthStore.getState().bootstrapSession;

/** The backend saved the callback's token but could not verify it yet. */
const unverifiedCallback: AuthStatePayload = {
  authenticated: false,
  user: null,
  pendingVerification: true,
};

function emitAuthState(payload: AuthStatePayload) {
  act(() => events.authHandler?.(payload));
}

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  vi.mocked(startOauthFlow).mockResolvedValue(undefined);
  vi.mocked(bootstrapSessionCmd).mockResolvedValue({
    authenticated: false,
    user: null,
    settings: DEFAULT_SETTINGS,
    authMethod: "github",
    status: "pending-verification",
  });
  useAuthStore.setState({
    isLoading: false,
    user: null,
    isAuthenticated: false,
    oauthPending: false,
    oauthCallbackExpected: false,
    error: null,
    bootstrapSession: storeBootstrapSession,
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

    expect(bootstrapSessionCmd).toHaveBeenCalledTimes(1);
    expect(useAuthStore.getState().oauthPending).toBe(false);
    expect(useAuthStore.getState().oauthCallbackExpected).toBe(false);
  });

  it("still bootstraps a callback finished after Cancel", async () => {
    await useAuthStore.getState().startOAuthFlow();
    useAuthStore.getState().cancelOAuthFlow();

    emitAuthState(unverifiedCallback);

    expect(bootstrapSessionCmd).toHaveBeenCalledTimes(1);
  });

  it("still bootstraps a callback that trails the UI timeout", async () => {
    await useAuthStore.getState().startOAuthFlow();
    vi.advanceTimersByTime(OAUTH_TIMEOUT_MS + 10_000);
    expect(useAuthStore.getState().oauthPending).toBe(false);

    emitAuthState(unverifiedCallback);

    expect(bootstrapSessionCmd).toHaveBeenCalledTimes(1);
  });

  it("treats a sign-out event as definitive once no callback can arrive", async () => {
    await useAuthStore.getState().startOAuthFlow();
    vi.advanceTimersByTime(OAUTH_TIMEOUT_MS + OAUTH_CALLBACK_GRACE_MS);

    emitAuthState(unverifiedCallback);

    expect(bootstrapSessionCmd).not.toHaveBeenCalled();
  });

  it("does not bootstrap a sign-out event when GitHub was never started", () => {
    emitAuthState(unverifiedCallback);

    expect(bootstrapSessionCmd).not.toHaveBeenCalled();
  });

  it("keeps a rejected GitHub callback's cleanup warning until recovery", async () => {
    const warning = "Account access is disabled, but local sign-out cleanup could not be saved. Codemux will retry.";
    await useAuthStore.getState().startOAuthFlow();
    useAuthStore.setState({
      user: { id: "u1", email: "u1@example.com", name: null, image: null },
      isAuthenticated: true,
      sessionStatus: "offline",
    });
    useSyncedSettingsStore.getState().replaceSessionSettings({
      ...DEFAULT_SETTINGS,
      appearance: { ...DEFAULT_SETTINGS.appearance, theme: "dark" },
    });

    emitAuthState({ authenticated: false, user: null, storageError: warning });

    expect(useAuthStore.getState()).toMatchObject({
      user: null,
      isAuthenticated: false,
      sessionStatus: "signed-out",
      error: warning,
    });
    expect(bootstrapSessionCmd).not.toHaveBeenCalled();
    expect(useAuthStore.getState().oauthCallbackExpected).toBe(false);
    expect(useSyncedSettingsStore.getState().settings).toEqual(DEFAULT_SETTINGS);
    act(() => events.storageErrorClearedHandler?.(warning));
    expect(useAuthStore.getState().error).toBeNull();
  });

  it("clears only the old cleanup warning without ending a new OAuth flow", async () => {
    const warning = "Account access is disabled, but local sign-out cleanup could not be saved. Codemux will retry.";
    useAuthStore.setState({ error: warning });
    await useAuthStore.getState().startOAuthFlow();
    act(() => events.storageErrorClearedHandler?.(warning));
    expect(useAuthStore.getState().oauthPending).toBe(true);
    expect(useAuthStore.getState().oauthCallbackExpected).toBe(true);
    expect(bootstrapSessionCmd).not.toHaveBeenCalled();

    useAuthStore.setState({ error: "A newer sign-in error" });
    act(() => events.storageErrorClearedHandler?.(warning));
    expect(useAuthStore.getState().error).toBe("A newer sign-in error");
  });
});
