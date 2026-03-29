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

  // Check auth on mount, then load synced settings if authenticated
  useEffect(() => {
    useAuthStore.getState().checkAuth().then(() => {
      if (useAuthStore.getState().isAuthenticated) {
        useSyncedSettingsStore.getState().loadSettings();
      }
    });
  }, []);

  // Listen for auth state changes from Tauri (OAuth callback, token expiry)
  useAuthEvents();

  // Only initialize app state and shortcuts when authenticated
  useAppStateInit(!isAuthenticated);
  useKeyboardShortcuts();

  if (isLoading || !isAuthenticated) {
    return <LoginScreen />;
  }

  return <AppShell />;
}

export default App;
