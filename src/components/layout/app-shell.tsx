import { lazy, useState, useEffect, useLayoutEffect, useMemo } from "react";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import {
  useSettingsStore,
  selectDensity,
} from "@/stores/settings-store";
import {
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { applyTheme, parseCustomThemes, resolveTheme } from "@/lib/themes";
import { applyTypography, resolveTypographySettings } from "@/lib/typography";
import { SidebarProvider, SidebarInset, useSidebar } from "@/components/ui/sidebar";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import { AppSidebar } from "./app-sidebar";
import { TitleBar } from "./title-bar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";
import { useWorktreeIncludeToast } from "@/hooks/use-worktree-include-toast";
import { LazyBoundary } from "@/components/ui/lazy-boundary";
import { markStartup } from "@/lib/perf/interaction-trace";
import { scheduleSequentialIdlePrefetch } from "@/lib/idle-prefetch";

const loadSettingsView = () => import("@/components/settings/settings-view");
const loadCommandPalette = () => import("@/components/overlays/command-palette");
const loadFileSearchDialog = () => import("@/components/search/file-search-dialog");
const loadContentSearchDialog = () => import("@/components/search/content-search-dialog");

const SettingsView = lazy(() =>
  loadSettingsView().then((module) => ({ default: module.SettingsView })),
);
const AutomationsView = lazy(() =>
  import("@/components/automations/automations-view").then((module) => ({ default: module.AutomationsView })),
);
const DevicesView = lazy(() =>
  import("@/components/devices/devices-view").then((module) => ({
    default: module.DevicesView,
  })),
);
const PullRequestsView = lazy(() =>
  import("@/components/pull-requests/pull-requests-view").then((module) => ({
    default: module.PullRequestsView,
  })),
);
const CommandPalette = lazy(() =>
  loadCommandPalette().then((module) => ({ default: module.CommandPalette })),
);
const NewProjectScreen = lazy(() =>
  import("@/components/overlays/new-project-screen").then((module) => ({ default: module.NewProjectScreen })),
);
const FileSearchDialog = lazy(() =>
  loadFileSearchDialog().then((module) => ({ default: module.FileSearchDialog })),
);
const ContentSearchDialog = lazy(() =>
  loadContentSearchDialog().then((module) => ({ default: module.ContentSearchDialog })),
);
const BrowserPeekOverlay = lazy(() =>
  import("@/components/browser/BrowserPeekOverlay").then((module) => ({ default: module.BrowserPeekOverlay })),
);

const EMPTY_THEME_PAYLOADS: unknown[] = [];

export function AppShell({ onFirstPaint }: { onFirstPaint?: () => void } = {}) {
  const isLoading = useAppStore((s) => s.appState === null);
  const settingsLoaded = useSettingsStore((s) => s.loaded);
  const syncedLoading = useSyncedSettingsStore((s) => s.isLoading);
  const hasWorkspaces = useAppStore(
    (s) => (s.appState?.workspaces.length ?? 0) > 0,
  );
  // Under lazy creation, a client-side draft is enough to keep the
  // main app shell alive even before any workspace exists. The draft
  // surface is rendered by WorkspaceMain.
  const lazyEnabled = useFeatureFlags((s) => s.enableLazyWorkspaceCreation);
  const hasActiveDraft = useChatDraftStore((s) => s.activeDraftId !== null);
  const showSettings = useUIStore((s) => s.showSettings);
  const showAutomations = useUIStore((s) => s.showAutomations);
  const showDevices = useUIStore((s) => s.showDevices);
  const showPullRequests = useUIStore((s) => s.showPullRequests);
  const showNewProjectScreen = useUIStore((s) => s.showNewProjectScreen);
  const commandPaletteOpen = useUIStore((s) => s.showCommandPalette);
  const fileSearchOpen = useUIStore((s) => s.showFileSearch);
  const contentSearchOpen = useUIStore((s) => s.showContentSearch);
  const browserPeekOpen = useBrowserPeekStore((s) => s.openWorkspaceId !== null);
  const setCommandPaletteOpen = useUIStore((s) => s.setShowCommandPalette);
  const [sidebarOpen, setSidebarOpen] = useState(true);

  // Appearance uses the synced theme id and payloads. Density stays local: it
  // controls layout rhythm rather than the color system.
  const syncedThemeId = useSyncedSettingsStore((s) => s.settings?.appearance?.theme ?? "default");
  const customThemePayloads = useSyncedSettingsStore((s) => s.settings?.appearance?.custom_themes ?? EMPTY_THEME_PAYLOADS);
  const typographyAppearance = useSyncedSettingsStore((s) => s.settings?.appearance);
  const updateSyncedSetting = useSyncedSettingsStore((s) => s.updateSetting);
  const legacyPalette = useSettingsStore((s) => s.settings["appearance.palette"]);
  const density = useSettingsStore(selectDensity);
  const customThemes = useMemo(
    () => parseCustomThemes(customThemePayloads),
    [customThemePayloads],
  );
  // Both stores must have answered before the synced theme id means anything:
  // until then `syncedThemeId` is the DEFAULT_SETTINGS placeholder ("default")
  // and `legacyPalette` is undefined, so acting on either would be acting on a
  // value the user never chose.
  const appearanceReady = settingsLoaded && !syncedLoading;
  const effectiveThemeId =
    legacyPalette === "warm" &&
    (syncedThemeId === "system" || syncedThemeId === "dark" || syncedThemeId === "default")
      ? "warm"
      : syncedThemeId;
  const activeTheme = useMemo(
    () => resolveTheme(effectiveThemeId, customThemes),
    [effectiveThemeId, customThemes],
  );
  const typography = useMemo(
    () => resolveTypographySettings(typographyAppearance),
    [typographyAppearance],
  );

  useEffect(() => {
    useSettingsStore.getState().load();
  }, []);

  // The inline boot script already painted the last applied theme, so until the
  // stores load there is nothing to do: applying here would repaint the shell
  // to Graphite and — because `applyTheme` persists by default — overwrite the
  // boot shadow with it, so a crash (or a failed load) during that window would
  // open the *next* launch on Graphite too. Density is a layout attribute, not
  // a color, and stays unconditional.
  useLayoutEffect(() => {
    const root = document.documentElement;
    if (appearanceReady) {
      applyTheme(activeTheme);
      applyTypography(root, typography);
    }
    delete root.dataset.pal;
    root.dataset.density = density;
  }, [activeTheme, density, appearanceReady, typography]);

  // One-time migration from the machine-local Cool/Warm axis to the unified,
  // synced theme id. Only an explicitly stored legacy value participates, and
  // only once the real settings are in hand — reading the placeholder id would
  // stamp "warm" over whatever theme the account actually holds. The local key
  // is rewritten as soon as the synced write lands, which both retires the
  // `default → warm` display mapping above and stops the migration from firing
  // again the next time the user deliberately picks Graphite.
  useEffect(() => {
    if (!appearanceReady) return;
    if (legacyPalette !== "warm") return;
    if (syncedThemeId !== "system" && syncedThemeId !== "dark" && syncedThemeId !== "default") return;
    updateSyncedSetting("appearance", "theme", "warm")
      .then(() => {
        useSettingsStore.getState().set("appearance.palette", "cool");
      })
      .catch(console.error);
  }, [appearanceReady, legacyPalette, syncedThemeId, updateSyncedSetting]);

  useWorktreeIncludeToast();

  useEffect(() => {
    if (isLoading || !settingsLoaded || syncedLoading) return;
    markStartup("shell-ready");
    const raf =
      typeof requestAnimationFrame === "function"
        ? requestAnimationFrame
        : (callback: FrameRequestCallback) =>
            setTimeout(() => callback(performance.now()), 0) as unknown as number;
    const cancelRaf =
      typeof cancelAnimationFrame === "function"
        ? cancelAnimationFrame
        : (handle: number) => clearTimeout(handle);
    let secondFrame = 0;
    let cancelled = false;
    let cancelPrefetch = () => {};
    const firstFrame = raf(() => {
      secondFrame = raf(() => {
        if (cancelled) return;
        markStartup("shell-first-paint");
        onFirstPaint?.();
        // Warm the most common post-shell destinations only after the useful
        // shell has painted. requestIdleCallback is allowed to run before a
        // pending rAF, so arming this earlier could put chunk parse/eval back
        // inside the startup gate.
        cancelPrefetch = scheduleSequentialIdlePrefetch([
          loadCommandPalette,
          // The two pane kinds a workspace switch mounts. Without these
          // warm, the first switch to a not-yet-seen pane kind pays chunk
          // fetch + parse inside the switch itself.
          () => import("@/components/chat/AgentChatPane"),
          () => import("@/components/terminal/TerminalPane"),
          loadFileSearchDialog,
          loadContentSearchDialog,
          loadSettingsView,
        ]);
      });
    });
    return () => {
      cancelled = true;
      cancelRaf(firstFrame);
      cancelRaf(secondFrame);
      cancelPrefetch();
    };
  }, [isLoading, settingsLoaded, syncedLoading, onFirstPaint]);

  // Baseline sidebar toggle for the central keyboard hook and the command
  // palette. `SidebarToggleBridge` (rendered inside `SidebarProvider` below)
  // replaces this with the provider's own `toggleSidebar` as soon as the
  // sidebar mounts — that one also handles the narrow-viewport Sheet, which
  // `setSidebarOpen` alone cannot reach. This registration still matters for
  // the branches that return before `SidebarProvider` renders.
  useEffect(() => {
    useUIStore.getState().setSidebarToggleFn(() => setSidebarOpen((o) => !o));
    return () => useUIStore.getState().setSidebarToggleFn(null);
  }, []);

  if (isLoading || !settingsLoaded || syncedLoading) {
    return (
      <div className="flex h-screen items-center justify-center bg-background text-muted-foreground">
        Loading…
      </div>
    );
  }

  // Full-screen settings — replaces entire app including sidebar
  if (showSettings) {
    return (
      <LazyBoundary label="Settings" className="h-screen">
        <SettingsView />
      </LazyBoundary>
    );
  }

  // Full-screen Automations — a first-class destination, like Settings
  if (showAutomations) {
    return (
      <LazyBoundary label="Automations" className="h-screen">
        <AutomationsView />
      </LazyBoundary>
    );
  }

  // Full-screen Devices page — the account's other machines and the work
  // that lives on them. Same overlay shape as Settings / Automations.
  if (showDevices) {
    return (
      <LazyBoundary label="Devices" className="h-screen">
        <DevicesView />
      </LazyBoundary>
    );
  }

  // Full-screen Pull requests — the review surface for work that isn't
  // in a workspace yet. Same overlay shape as the pages above it.
  if (showPullRequests) {
    return (
      <LazyBoundary label="Pull requests" className="h-screen">
        <PullRequestsView />
      </LazyBoundary>
    );
  }

  // Full-screen new project — replaces entire app including sidebar
  if (showNewProjectScreen) {
    return (
      <LazyBoundary label="New project" className="h-screen">
        <NewProjectScreen />
      </LazyBoundary>
    );
  }

  // Full-screen empty state — no sidebar, no title bar. Bypassed when
  // a lazy-creation draft is active, so the draft surface can render
  // inside the normal app shell (sidebar, title bar, WorkspaceMain).
  if (!hasWorkspaces && !(lazyEnabled && hasActiveDraft)) {
    return <EmptyState />;
  }

  return (
    <div className="relative flex h-screen max-h-screen flex-col overflow-hidden">
      <TitleBar
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() => setSidebarOpen((o) => !o)}
      />
      <SidebarProvider
        open={sidebarOpen}
        onOpenChange={setSidebarOpen}
        className="flex-1 min-h-0"
      >
        <SidebarToggleBridge />
        <AppSidebar />
        <SidebarInset className="flex flex-col overflow-hidden h-full min-w-0">
          <WorkspaceMain />
          {/* GUI-mode background browser peek — absolutely positioned
              inside this `relative` SidebarInset, so it floats over
              WorkspaceMain without resizing it. Renders nothing unless
              GUI chrome applies and the peek is explicitly opened. */}
          {browserPeekOpen && (
            <LazyBoundary
              label="browser peek"
              className="absolute right-3.5 top-3.5 z-30 h-[300px] w-[440px] rounded-xl border border-border"
            >
              <BrowserPeekOverlay />
            </LazyBoundary>
          )}
        </SidebarInset>
        {commandPaletteOpen && (
          <LazyBoundary
            label="command palette"
            className="fixed inset-0 z-50 h-screen"
            presentation="overlay"
          >
            <CommandPalette
              open={commandPaletteOpen}
              onOpenChange={setCommandPaletteOpen}
            />
          </LazyBoundary>
        )}
        {fileSearchOpen && (
          <LazyBoundary
            label="file search"
            className="fixed inset-0 z-50 h-screen"
            presentation="overlay"
          >
            <FileSearchDialog />
          </LazyBoundary>
        )}
        {contentSearchOpen && (
          <LazyBoundary
            label="content search"
            className="fixed inset-0 z-50 h-screen"
            presentation="overlay"
          >
            <ContentSearchDialog />
          </LazyBoundary>
        )}
      </SidebarProvider>
    </div>
  );
}

/**
 * Publishes the sidebar's own `toggleSidebar` to the UI store, so every
 * non-React caller — the Ctrl+B keybind and the command palette's "Toggle
 * sidebar" row, both of which go through `dispatch()` — flips whichever
 * sidebar is actually on screen. Below 768px `SidebarProvider` renders a
 * Sheet driven by a separate `openMobile` state, so driving the desktop
 * `open` prop alone is a silent no-op there.
 *
 * Must render inside `SidebarProvider` (it needs `useSidebar`), and after it
 * unmounts the shell's own baseline registration is restored.
 */
function SidebarToggleBridge() {
  const { toggleSidebar } = useSidebar();
  useEffect(() => {
    useUIStore.getState().setSidebarToggleFn(toggleSidebar);
  }, [toggleSidebar]);
  return null;
}
