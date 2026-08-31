import { lazy, useCallback, useEffect, useRef } from "react";
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
import { RemoteConnectionBanner } from "@/components/remote/remote-connection-indicator";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { Toaster } from "@/components/ui/sonner";
import { PrEventWatcher } from "@/components/pull-requests/pr-event-watcher";
import { UpdateToast } from "@/components/update/update-toast";
import { LoginScreen } from "@/components/auth/login-screen";
import { useAppStore } from "@/stores/app-store";
import { useAuthStore } from "@/stores/auth-store";
import { useFeatureFlagsInit } from "@/stores/feature-flags";
import { useEnsureDraftWhenEmpty } from "@/hooks/use-ensure-draft-when-empty";
import { useTranscriptSelectionHighlight } from "@/hooks/use-transcript-selection-highlight";
import { getHomeDir, repairInactiveMcpConfigs } from "@/tauri/commands";
import { useUIStore } from "@/stores/ui-store";
import { useRemotePathPickerStore } from "@/components/remote/remote-path-picker-store";
import { LazyBoundary } from "@/components/ui/lazy-boundary";
import { useActiveWorkspacePersistenceErrors } from "@/hooks/use-active-workspace-persistence-errors";
import { markStartup } from "@/lib/perf/interaction-trace";
import { usePostPaintPendingSessionRefresh } from "@/hooks/use-post-paint-session-refresh";

const ThemeStudio = lazy(() =>
  import("@/components/settings/theme-studio").then((module) => ({
    default: module.ThemeStudio,
  })),
);
const RemotePathPicker = lazy(() =>
  import("@/components/remote/remote-path-picker").then((module) => ({
    default: module.RemotePathPicker,
  })),
);
const RenameWorkspaceDialog = lazy(() =>
  import("@/components/overlays/rename-workspace-dialog").then((module) => ({
    default: module.RenameWorkspaceDialog,
  })),
);

function App() {
  const isAuthenticated = useAuthStore((s) => s.isAuthenticated);
  const isLoading = useAuthStore((s) => s.isLoading);
  const sessionStatus = useAuthStore((s) => s.sessionStatus);
  const renameWorkspaceId = useUIStore((s) => s.renameWorkspaceId);
  const themeStudioRequest = useUIStore((s) => s.themeStudio);
  const remotePathRequest = useRemotePathPickerStore((s) => s.request);
  const postPaintStartupBegan = useRef(false);

  // Paint from the local auth/settings cache first. AppShell explicitly tells
  // us when the useful shell (not the interim login/loading frame) has painted;
  // remote work is started by that callback below.
  useEffect(() => {
    markStartup("local-session-start");
    void useAuthStore
      .getState()
      .bootstrapSession()
      .finally(() => markStartup("local-session-ready"));
  }, []);

  // Verification runs on every invocation — the pending-verification retry
  // schedule and later session boundaries (a second account signing in after
  // a sign-out) depend on it. The expensive startup one-shots — the deferred
  // MCP config repair and the startup trace marks — fire once per process,
  // not once per retry tick.
  const handleShellFirstPaint = useCallback(() => {
    const firstRun = !postPaintStartupBegan.current;
    if (firstRun) {
      postPaintStartupBegan.current = true;
      markStartup("remote-session-start");
      void repairInactiveMcpConfigs().catch((error) => {
        console.warn("[startup] deferred MCP config repair failed:", error);
      });
    }
    const refreshed = useAuthStore.getState().refreshSession();
    void (firstRun
      ? refreshed.finally(() => markStartup("remote-session-ready"))
      : refreshed);
  }, []);

  // A non-expired token may have no cached user yet. There is no AppShell to
  // report its first paint in that state, so the painted login frame owns the
  // same guarded post-paint verification path.
  usePostPaintPendingSessionRefresh(
    isLoading,
    sessionStatus,
    handleShellFirstPaint,
  );

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
  useEnsureDraftWhenEmpty();
  useTerminalCacheGc();
  useTerminalThemeSync();
  useTerminalCwdPoll();
  useAutomationFireToast();
  useActiveWorkspacePersistenceErrors();
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
      <AppShell onFirstPaint={handleShellFirstPaint} />
      {renameWorkspaceId && (
        <LazyBoundary
          label="workspace rename"
          className="fixed inset-0 z-50 h-screen"
          presentation="overlay"
        >
          <RenameWorkspaceDialog />
        </LazyBoundary>
      )}
      {/* App-level, not a child of the Appearance page: both of its doors —
          the palette's create/paste rows and Appearance's Customize button —
          live on surfaces that replace each other. */}
      {themeStudioRequest && (
        <LazyBoundary
          label="Theme Studio"
          className="fixed inset-0 z-50 h-screen"
          presentation="overlay"
        >
          <ThemeStudio />
        </LazyBoundary>
      )}
      <UpdateToast />
      {/* Web remote client only: the in-app path browser that stands in for
          the native OS file dialog. Never mounted on desktop. */}
      {isRemoteClient() && remotePathRequest && (
        <LazyBoundary
          label="remote path picker"
          className="fixed inset-0 z-50 h-screen"
          presentation="overlay"
        >
          <RemotePathPicker />
        </LazyBoundary>
      )}
      {/* Web remote client only: the loud reconnecting/offline banner. The
          quiet connected state lives as a chip in the title bar. Never
          mounted on desktop. */}
      {isRemoteClient() && <RemoteConnectionBanner />}
      {/* Two toasts and one link index — see `pr-events.ts`. Renders
          nothing, and sits above the full-screen destinations so it keeps
          watching from every screen. */}
      <PrEventWatcher />
      <Toaster />
    </>
  );
}

export default App;
