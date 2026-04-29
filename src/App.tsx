import { useEffect } from "react";
import { useAppStateInit } from "@/hooks/use-app-state";
import { useKeyboardShortcuts } from "@/hooks/use-keyboard-shortcuts";
import { useAuthEvents } from "@/hooks/use-auth-events";
import { useSkillsSync } from "@/hooks/use-skills-sync";
import { useScrollbackSerializer } from "@/hooks/use-scrollback-serializer";
import { AppShell } from "@/components/layout/app-shell";
import { Toaster } from "@/components/ui/sonner";
import { UpdateToast } from "@/components/update/update-toast";
import { LoginScreen } from "@/components/auth/login-screen";
import { useAppStore } from "@/stores/app-store";
import { useAuthStore } from "@/stores/auth-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useFeatureFlagsInit } from "@/stores/feature-flags";
import { useProviderCapabilitiesInit } from "@/stores/provider-capabilities-store";
import { useEnsureDraftWhenEmpty } from "@/hooks/use-ensure-draft-when-empty";
import { getHomeDir } from "@/tauri/commands";

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

  // Skills sync engine triggers: kick a sync after sync becomes
  // available and after the file watcher reports `skills-changed`
  // (debounced). Engine itself serializes concurrent calls.
  useSkillsSync();

  // Cache $HOME at App mount. Downstream selectors (project-group
  // labelling, future Step-5 home-rooted chat detection) compare
  // project_root against this value — without it, home-rooted
  // workspaces render under the literal path basename.
  useEffect(() => {
    void (async () => {
      try {
        const homeDir = await getHomeDir();
        useAppStore.getState().setHomeDir(homeDir);
      } catch (err) {
        console.error("Failed to cache home dir:", err);
      }
    })();
  }, []);

  // Only initialize app state and shortcuts when authenticated
  useAppStateInit(!isAuthenticated);
  useKeyboardShortcuts();
  useScrollbackSerializer();
  useFeatureFlagsInit();
  useProviderCapabilitiesInit();
  useEnsureDraftWhenEmpty();

  if (isLoading || !isAuthenticated) {
    return <LoginScreen />;
  }

  return (
    <>
      <AppShell />
      <UpdateToast />
      <Toaster />
    </>
  );
}

export default App;
