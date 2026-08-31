import { useCallback, useEffect, useRef } from "react";
import { useTauriEvent } from "./use-tauri-event";
import {
  onAuthStateChanged,
  onSettingsSynced,
  onSyncStateChanged,
  type SyncStateChangedPayload,
} from "@/tauri/events";
import { useAuthStore } from "@/stores/auth-store";
import { useSyncedSettingsStore, DEFAULT_SETTINGS } from "@/stores/synced-settings-store";
import type { AuthStatePayload, UserSettings } from "@/tauri/types";

/**
 * Listens to "auth-state-changed" Tauri events (from OAuth callback, token expiry, etc.)
 * and updates the auth store. Also re-verifies the token on window focus
 * (debounced to once per 5 minutes).
 */
export function useAuthEvents() {
  const setUser = useAuthStore((s) => s.setUser);
  const refreshSession = useAuthStore((s) => s.refreshSession);
  const applySettings = useSyncedSettingsStore((s) => s.applySettingsFromEvent);
  const lastCheckRef = useRef(0);

  // Handle auth-state-changed events from the Rust backend
  const handleAuthEvent = useCallback(
    (payload: AuthStatePayload) => {
      const oauthWasPending = useAuthStore.getState().isSigningIn;
      if (payload.authenticated && payload.user) {
        if (useAuthStore.getState().user?.id !== payload.user.id) {
          useSyncedSettingsStore
            .getState()
            .replaceSessionSettings(DEFAULT_SETTINGS);
        }
        setUser(payload.user);
        void useAuthStore.getState().refreshSession();
      } else {
        setUser(null);
        useSyncedSettingsStore
          .getState()
          .replaceSessionSettings(DEFAULT_SETTINGS);
        // A callback can persist a valid OAuth token while its immediate user
        // lookup times out. Re-read local state so it becomes
        // `pending-verification`; App will retry verification only after the
        // login frame has painted. Definitive sign-out events skip this path.
        if (oauthWasPending) {
          void useAuthStore.getState().bootstrapSession();
        }
      }
      // Also clear the signing-in state since the flow completed
      useAuthStore.setState({ isSigningIn: false });
    },
    [setUser],
  );

  useTauriEvent(onAuthStateChanged, handleAuthEvent, [handleAuthEvent]);

  // Handle settings-synced events from the Rust backend
  const handleSettingsSynced = useCallback(
    (settings: UserSettings) => {
      applySettings(settings);
    },
    [applySettings],
  );

  useTauriEvent(onSettingsSynced, handleSettingsSynced, [handleSettingsSynced]);

  // Handle Stage 2 sync state changes (signin, signout, sync setup, sync repair).
  const handleSyncEvent = useCallback((payload: SyncStateChangedPayload) => {
    useAuthStore.getState().setSyncStatus(payload);
  }, []);

  useTauriEvent(onSyncStateChanged, handleSyncEvent, [handleSyncEvent]);

  // Re-verify token on window focus (at most once per 5 minutes)
  useEffect(() => {
    const RECHECK_INTERVAL = 5 * 60 * 1000; // 5 minutes

    const handleVisibility = () => {
      if (document.visibilityState !== "visible") return;
      if (!useAuthStore.getState().isAuthenticated) return;

      const now = Date.now();
      if (now - lastCheckRef.current < RECHECK_INTERVAL) return;
      lastCheckRef.current = now;

      void refreshSession();
    };

    document.addEventListener("visibilitychange", handleVisibility);
    return () =>
      document.removeEventListener("visibilitychange", handleVisibility);
  }, [refreshSession]);
}
