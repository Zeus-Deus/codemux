import { useCallback, useEffect, useRef } from "react";
import { useTauriEvent } from "./use-tauri-event";
import { onAuthStateChanged } from "@/tauri/events";
import { useAuthStore } from "@/stores/auth-store";
import type { AuthStatePayload } from "@/tauri/types";

/**
 * Listens to "auth-state-changed" Tauri events (from OAuth callback, token expiry, etc.)
 * and updates the auth store. Also re-verifies the token on window focus
 * (debounced to once per 5 minutes).
 */
export function useAuthEvents() {
  const setUser = useAuthStore((s) => s.setUser);
  const checkAuth = useAuthStore((s) => s.checkAuth);
  const lastCheckRef = useRef(0);

  // Handle auth-state-changed events from the Rust backend
  const handleAuthEvent = useCallback(
    (payload: AuthStatePayload) => {
      if (payload.authenticated && payload.user) {
        setUser(payload.user);
      } else {
        setUser(null);
      }
      // Also clear the signing-in state since the flow completed
      useAuthStore.setState({ isSigningIn: false });
    },
    [setUser],
  );

  useTauriEvent(onAuthStateChanged, handleAuthEvent, [handleAuthEvent]);

  // Re-verify token on window focus (at most once per 5 minutes)
  useEffect(() => {
    const RECHECK_INTERVAL = 5 * 60 * 1000; // 5 minutes

    const handleVisibility = () => {
      if (document.visibilityState !== "visible") return;
      if (!useAuthStore.getState().isAuthenticated) return;

      const now = Date.now();
      if (now - lastCheckRef.current < RECHECK_INTERVAL) return;
      lastCheckRef.current = now;

      checkAuth();
    };

    document.addEventListener("visibilitychange", handleVisibility);
    return () =>
      document.removeEventListener("visibilitychange", handleVisibility);
  }, [checkAuth]);
}
