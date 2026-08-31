import { create } from "zustand";
import type { AuthUser } from "@/tauri/types";
import {
  bootstrapSession as bootstrapSessionCmd,
  refreshSession as refreshSessionCmd,
  getSyncStatus as getSyncStatusCmd,
  startOauthFlow as startOauthFlowCmd,
  signinEmail as signinEmailCmd,
  signupEmail as signupEmailCmd,
  signOut as signOutCmd,
  type SyncStatus,
} from "@/tauri/commands";
import type { AuthSessionStatus } from "@/tauri/types";
import {
  DEFAULT_SETTINGS,
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { useProviderRuntimeIntent } from "@/stores/provider-runtime-intent-store";
import { resetProviderCapabilities } from "@/stores/provider-capabilities-store";

export type AuthMethod = "email" | "github" | null;

/** Provider intent is scoped to one authenticated shell, not to the renderer.
 *  A sign-out/sign-in cycle remounts persisted chat panes in the same webview;
 *  retaining the previous account's intent would let an untouched empty pane
 *  launch its provider as soon as the new account's shell appears.
 *
 *  Sign-out (user → null) additionally drops the persisted provider capability
 *  catalog. Catalogs are public model metadata, so this is hygiene — a
 *  signed-out shell shouldn't keep replaying the previous session's picker
 *  cache. Launch bootstrap (null → cached user) deliberately does NOT clear,
 *  so pickers keep painting from the persisted catalog. */
function resetProviderScopedStateOnIdentityChange(
  currentUser: AuthUser | null,
  nextUser: AuthUser | null,
): void {
  if (currentUser?.id !== nextUser?.id) {
    useProviderRuntimeIntent.getState().reset();
  }
  if (currentUser !== null && nextUser === null) {
    resetProviderCapabilities();
  }
}

/** Singleflight for remote verification. Email sign-in awaits refreshSession
 *  while the backend's auth-state-changed event fires a second one through
 *  the event handler; without coalescing, two concurrent verifies (and
 *  settings GETs) race and the later-arriving response overwrites the
 *  earlier one. Concurrent callers share the same in-flight promise. */
let refreshSessionInFlight: Promise<void> | null = null;

/** Monotonic identity-transition counter. A verification flight belongs to
 *  the identity it started under: every transition (sign-out, email sign-in,
 *  auth-event user swap) bumps the epoch and drops the in-flight promise so
 *  the next caller starts a fresh verify for the new identity instead of
 *  joining a stale one. A stale flight that settles later compares its
 *  captured epoch and discards its result instead of clobbering the new
 *  identity's state. */
let sessionEpoch = 0;

function invalidateInFlightSessionRefresh(): void {
  sessionEpoch += 1;
  refreshSessionInFlight = null;
}

interface AuthStore {
  user: AuthUser | null;
  isAuthenticated: boolean;
  isLoading: boolean;
  isSigningIn: boolean;
  error: string | null;
  devBypass: boolean;
  sessionStatus: AuthSessionStatus;

  // Skills sync. Stored server-side (no client-held key), so
  // `syncAvailable` is simply "the user is signed in." `authMethod`
  // is kept so the UI can tailor copy.
  //   - syncAvailable=true  → Settings → Sync shows the dashboard
  //   - syncAvailable=false → "sign in to sync" hint
  syncAvailable: boolean;
  authMethod: AuthMethod;

  checkAuth: () => Promise<void>;
  bootstrapSession: () => Promise<void>;
  refreshSession: () => Promise<void>;
  startOAuthFlow: () => Promise<void>;
  signInEmail: (email: string, password: string) => Promise<void>;
  signUpEmail: (
    email: string,
    password: string,
    name: string,
  ) => Promise<void>;
  signOut: () => Promise<void>;
  setUser: (user: AuthUser | null) => void;
  setSyncStatus: (status: SyncStatus) => void;
  refreshSyncStatus: () => Promise<void>;
  clearError: () => void;
}

export const useAuthStore = create<AuthStore>((set, get) => ({
  user: null,
  isAuthenticated: false,
  isLoading: true,
  isSigningIn: false,
  error: null,
  devBypass: false,
  sessionStatus: "signed-out",
  syncAvailable: false,
  authMethod: null,

  bootstrapSession: async () => {
    set({ isLoading: true, error: null });
    try {
      const session = await bootstrapSessionCmd();
      resetProviderScopedStateOnIdentityChange(get().user, session.user);
      useSyncedSettingsStore.getState().replaceSessionSettings(session.settings);
      set({
        user: session.user,
        isAuthenticated: session.authenticated,
        isLoading: false,
        syncAvailable: session.authenticated,
        authMethod: session.authMethod,
        sessionStatus: session.status,
        devBypass: false,
      });
    } catch (error) {
      resetProviderScopedStateOnIdentityChange(get().user, null);
      useSyncedSettingsStore.getState().replaceSessionSettings(DEFAULT_SETTINGS);
      set({
        user: null,
        isAuthenticated: false,
        isLoading: false,
        syncAvailable: false,
        authMethod: null,
        sessionStatus: "signed-out",
        error: error instanceof Error ? error.message : String(error),
      });
    }
  },

  refreshSession: () => {
    if (refreshSessionInFlight) return refreshSessionInFlight;
    const request = (async () => {
      const flightEpoch = sessionEpoch;
      const startingUserId = get().user?.id ?? null;
      const settingsToken = useSyncedSettingsStore
        .getState()
        .remoteReconcileToken();
      try {
        const session = await refreshSessionCmd();
        // The identity changed while this flight was in the air; its result
        // describes the abandoned session, not the current one.
        if (flightEpoch !== sessionEpoch) return;
        resetProviderScopedStateOnIdentityChange(get().user, session.user);
        if ((session.user?.id ?? null) !== startingUserId) {
          useSyncedSettingsStore
            .getState()
            .replaceSessionSettings(session.settings);
        } else {
          useSyncedSettingsStore
            .getState()
            .reconcileRemoteSettings(session.settings, settingsToken);
        }
        set({
          user: session.user,
          isAuthenticated: session.authenticated,
          isLoading: false,
          syncAvailable: session.authenticated,
          authMethod: session.authMethod,
          sessionStatus: session.status,
          devBypass: false,
        });
      } catch (error) {
        if (flightEpoch !== sessionEpoch) return;
        // A rejection here is an invocation/transport failure, never a
        // verdict on the token: the backend resolves authoritative outcomes
        // (expired or rejected token) as a signed-out session instead of
        // rejecting. Keep the painted local session and surface its state.
        // A pending verification in particular must stay pending so the
        // post-paint retry path can re-verify instead of flashing a login
        // screen over a still-valid token.
        set((state) => ({
          isLoading: false,
          sessionStatus:
            state.sessionStatus === "pending-verification"
              ? state.sessionStatus
              : state.isAuthenticated
                ? "offline"
                : "signed-out",
          error: error instanceof Error ? error.message : String(error),
        }));
      } finally {
        // Safe for a discarded flight too: the identity transition already
        // replaced the settings session, which invalidated this token.
        useSyncedSettingsStore
          .getState()
          .finishRemoteReconcile(settingsToken);
      }
    })().finally(() => {
      if (refreshSessionInFlight === request) refreshSessionInFlight = null;
    });
    refreshSessionInFlight = request;
    return request;
  },

  checkAuth: async () => {
    await get().refreshSession();
  },

  startOAuthFlow: async () => {
    set({ isSigningIn: true, error: null });
    try {
      await startOauthFlowCmd();
      // Don't set isSigningIn=false here — the OAuth callback
      // will trigger an auth-state-changed event which updates the store
    } catch (err) {
      set({
        isSigningIn: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  signInEmail: async (email, password) => {
    set({ isSigningIn: true, error: null });
    try {
      const resp = await signinEmailCmd(email, password);
      // The signed-in account must not join, or be clobbered by, a verify
      // started under the previous identity/token.
      invalidateInFlightSessionRefresh();
      resetProviderScopedStateOnIdentityChange(get().user, resp.user);
      if (get().user?.id !== resp.user.id) {
        useSyncedSettingsStore
          .getState()
          .replaceSessionSettings(DEFAULT_SETTINGS);
      }
      set({
        user: resp.user,
        isAuthenticated: true,
        isSigningIn: false,
        sessionStatus: "local",
      });
      await get().refreshSession();
    } catch (err) {
      set({
        isSigningIn: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  signUpEmail: async (email, password, name) => {
    set({ isSigningIn: true, error: null });
    try {
      await signupEmailCmd(email, password, name);
      set({ isSigningIn: false });
    } catch (err) {
      set({
        isSigningIn: false,
        error: err instanceof Error ? err.message : String(err),
      });
    }
  },

  signOut: async () => {
    try {
      await signOutCmd();
    } catch {
      // Ignore errors — clear local state regardless
    }
    invalidateInFlightSessionRefresh();
    resetProviderScopedStateOnIdentityChange(get().user, null);
    useSyncedSettingsStore.getState().replaceSessionSettings(DEFAULT_SETTINGS);
    set({
      user: null,
      isAuthenticated: false,
      isSigningIn: false,
      devBypass: false,
      syncAvailable: false,
      authMethod: null,
      sessionStatus: "signed-out",
    });
  },

  setUser: (user) => {
    if (get().user?.id !== user?.id) {
      invalidateInFlightSessionRefresh();
    }
    resetProviderScopedStateOnIdentityChange(get().user, user);
    if (user) {
      set({ user, isAuthenticated: true, isSigningIn: false, sessionStatus: "local" });
    } else {
      set({ user: null, isAuthenticated: false, sessionStatus: "signed-out" });
    }
  },

  setSyncStatus: (status) =>
    set({ syncAvailable: status.syncAvailable, authMethod: status.authMethod }),

  refreshSyncStatus: async () => {
    try {
      const status = await getSyncStatusCmd();
      set({
        syncAvailable: status.syncAvailable,
        authMethod: status.authMethod,
      });
    } catch {
      // Tauri unavailable — leave whatever we had before.
      void get();
    }
  },

  clearError: () => set({ error: null }),
}));
