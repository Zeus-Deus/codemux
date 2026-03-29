import { useEffect } from "react";
import { useAppStateInit } from "@/hooks/use-app-state";
import { useKeyboardShortcuts } from "@/hooks/use-keyboard-shortcuts";
import { useAuthEvents } from "@/hooks/use-auth-events";
import { AppShell } from "@/components/layout/app-shell";
import { LoginScreen } from "@/components/auth/login-screen";
import { useAuthStore } from "@/stores/auth-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";

function App() {
  const isAuthenticated = useAuthStore((s) => s.isAuthenticated);
  const isLoading = useAuthStore((s) => s.isLoading);
  const checkAuth = useAuthStore((s) => s.checkAuth);

  // Check auth on mount, then load synced settings if authenticated
  useEffect(() => {
    console.log("[AUTH-DEBUG] App.tsx useEffect MOUNT - calling checkAuth()");
    checkAuth().then(() => {
      const state = useAuthStore.getState();
      console.log("[AUTH-DEBUG] App.tsx checkAuth() resolved:", {
        isAuthenticated: state.isAuthenticated,
        userId: state.user?.id,
        devBypass: state.devBypass,
      });
      if (state.isAuthenticated) {
        useSyncedSettingsStore.getState().loadSettings();
      }
    });
  }, [checkAuth]);

  // Listen for auth state changes from Tauri (OAuth callback, token expiry)
  useAuthEvents();

  // Only initialize app state and shortcuts when authenticated
  useAppStateInit(!isAuthenticated);
  useKeyboardShortcuts();

  console.log("[AUTH-DEBUG] App.tsx render:", { isLoading, isAuthenticated });

  if (isLoading || !isAuthenticated) {
    return <LoginScreen />;
  }

  return <AppShell />;
}

export default App;
