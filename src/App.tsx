import { useEffect } from "react";
import { useAppStateInit } from "@/hooks/use-app-state";
import { useKeyboardShortcuts } from "@/hooks/use-keyboard-shortcuts";
import { useAuthEvents } from "@/hooks/use-auth-events";
import { useTunnelStatusEvents } from "@/hooks/use-tunnel-status-events";
import { useSkillsSync } from "@/hooks/use-skills-sync";
import { useScrollbackSerializer } from "@/hooks/use-scrollback-serializer";
import { useTerminalCacheGc } from "@/hooks/use-terminal-cache-gc";
import { useTerminalThemeSync } from "@/hooks/use-terminal-theme-sync";
import { useTerminalCwdPoll } from "@/hooks/use-terminal-cwd-poll";
import { useAutomationFireToast } from "@/hooks/use-automation-fire-toast";
import { useWebNotifications } from "@/hooks/use-web-notifications";
import { useSmoothScrollingInit } from "@/hooks/use-smooth-scrolling";
import { useRendererModeInit } from "@/hooks/use-renderer-mode";
import { AppShell } from "@/components/layout/app-shell";
import { ThemeStudio } from "@/components/settings/theme-studio";
import { RemotePathPicker } from "@/components/remote/remote-path-picker";
import { RemoteConnectionBanner } from "@/components/remote/remote-connection-indicator";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { Toaster } from "@/components/ui/sonner";
import { UpdateToast } from "@/components/update/update-toast";
import { LoginScreen } from "@/components/auth/login-screen";
import { useAppStore } from "@/stores/app-store";
import { useAuthStore } from "@/stores/auth-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { useFeatureFlagsInit } from "@/stores/feature-flags";
import { useProviderCapabilitiesInit } from "@/stores/provider-capabilities-store";
import { useEnsureDraftWhenEmpty } from "@/hooks/use-ensure-draft-when-empty";
import { useTranscriptSelectionHighlight } from "@/hooks/use-transcript-selection-highlight";
import { getHomeDir } from "@/tauri/commands";
import { RenameWorkspaceDialog } from "@/components/overlays/rename-workspace-dialog";

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

  // Bridge SSH tunnel health into the tunnel-status store so remote
  // workspaces can show a "Reconnecting…" / "Connection lost" pill.
  useTunnelStatusEvents();

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
  useTerminalCacheGc();
  useTerminalThemeSync();
  useTerminalCwdPoll();
  useAutomationFireToast();
  // Re-apply a persisted "smooth scrolling: on" to the fresh webview once the
  // machine-local settings have loaded. Off is the native default — no-op.
  useSmoothScrollingInit();
  // Ask the backend which renderer it ended up on, so composited-only UI
  // effects (the transcript edge-fade) switch themselves off when the webview
  // is running CPU-rendered.
  useRendererModeInit();
  // WebKitGTK otherwise paints the virtualized boxes between selected chat
  // messages. Keep native selection semantics and paint exact text ranges.
  useTranscriptSelectionHighlight();
  // Web remote client only: bridge backend `notification` events into the
  // browser (Web Notifications API with a toast fallback). No-op on desktop.
  useWebNotifications();

  if (isLoading || !isAuthenticated) {
    return <LoginScreen />;
  }

  return (
    <>
      <AppShell />
      <RenameWorkspaceDialog />
      {/* App-level, not a child of the Appearance page: both of its doors —
          the palette's create/paste rows and Appearance's Customize button —
          live on surfaces that replace each other. */}
      <ThemeStudio />
      <UpdateToast />
      {/* Web remote client only: the in-app path browser that stands in for
          the native OS file dialog. Never mounted on desktop. */}
      {isRemoteClient() && <RemotePathPicker />}
      {/* Web remote client only: the loud reconnecting/offline banner. The
          quiet connected state lives as a chip in the title bar. Never
          mounted on desktop. */}
      {isRemoteClient() && <RemoteConnectionBanner />}
      <Toaster />
    </>
  );
}

export default App;
