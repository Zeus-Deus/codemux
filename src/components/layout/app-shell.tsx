import { useNotificationLink } from "@/hooks/use-notification-link";
import { useMobileLayout, useMobileViewport } from "@/hooks/use-mobile-layout";
import { lazy, useState, useEffect, useLayoutEffect, useMemo, useRef } from "react";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { selectUtilityPage, type UtilityPage } from "@/lib/utility-pages";
import {
  useSettingsStore,
  selectDensity,
} from "@/stores/settings-store";
import {
  useSyncedSettingsStore,
} from "@/stores/synced-settings-store";
import { applyTheme } from "@/lib/themes";
import { useAppTheme, setThemeSource } from "@/hooks/use-app-theme";
import { useOmarchyStore } from "@/stores/omarchy-store";
import { applyTypography, resolveTypographySettings } from "@/lib/typography";
import { SidebarProvider, SidebarInset, useSidebar } from "@/components/ui/sidebar";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import { AppSidebar } from "./app-sidebar";
import { TitleBar } from "./title-bar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";
import { useWorktreeIncludeToast } from "@/hooks/use-worktree-include-toast";
import { LazyBoundary } from "@/components/ui/lazy-boundary";
import { UtilityPageStandaloneContext } from "./utility-page";
import { markStartup } from "@/lib/perf/interaction-trace";
import { scheduleSequentialIdlePrefetch } from "@/lib/idle-prefetch";

import { isRemoteClient } from "@/components/remote/is-remote-client";

const LocalSessionImport = lazy(() => import("@/components/chat/LocalSessionImport").then(m => ({ default: m.LocalSessionImport })));
const MobileShell = lazy(() => import("@/components/mobile/mobile-shell").then(m => ({ default: m.MobileShell })));

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
const UsageView = lazy(() =>
  import("@/components/settings/usage-view").then((module) => ({
    default: module.UsageView,
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

export function AppShell(props: { onFirstPaint?: () => void } = {}) {
  const showImport = useUIStore((s) => s.showLocalSessionImport);
  const mobile = useMobileLayout();
  // A stable sibling of all route gates: native import emits workspaces before
  // its promise returns, so placing this under EmptyState would lose results.
  return <>
    {showImport && !mobile && !isRemoteClient() && <LazyBoundary label="recent chat import" presentation="overlay"><LocalSessionImport /></LazyBoundary>}
    <AppShellContent {...props} />
  </>;
}

function AppShellContent({ onFirstPaint }: { onFirstPaint?: () => void } = {}) {
  const mobile = useMobileLayout();
  useMobileViewport();
  useNotificationLink();
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
  const activeDraftId = useChatDraftStore((s) => s.activeDraftId);
  const hasActiveDraft = activeDraftId !== null;
  const showSettings = useUIStore((s) => s.showSettings);
  const utilityPage = useUIStore(selectUtilityPage);
  const showNewProjectScreen = useUIStore((s) => s.showNewProjectScreen);
  const commandPaletteOpen = useUIStore((s) => s.showCommandPalette);
  const fileSearchOpen = useUIStore((s) => s.showFileSearch);
  const contentSearchOpen = useUIStore((s) => s.showContentSearch);
  const browserPeekOpen = useBrowserPeekStore((s) => s.openWorkspaceId !== null);
  const setCommandPaletteOpen = useUIStore((s) => s.setShowCommandPalette);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  // Width lives in the persisted UI store: Settings and the standalone
  // pages below unmount `SidebarProvider`, which would otherwise reset it.
  const sidebarWidth = useUIStore((s) => s.sidebarWidth);
  const setSidebarWidth = useUIStore((s) => s.setSidebarWidth);

  // Appearance uses the synced theme id and payloads. Density stays local: it
  // controls layout rhythm rather than the color system.
  const syncedThemeId = useSyncedSettingsStore((s) => s.settings?.appearance?.theme ?? "default");

  const typographyAppearance = useSyncedSettingsStore((s) => s.settings?.appearance);
  const updateSyncedSetting = useSyncedSettingsStore((s) => s.updateSetting);
  const legacyPalette = useSettingsStore((s) => s.settings["appearance.palette"]);
  const density = useSettingsStore(selectDensity);
  const { theme: activeTheme, discovered, source, savedSource } = useAppTheme();
  // Both stores must have answered before the synced theme id means anything:
  // until then `syncedThemeId` is the DEFAULT_SETTINGS placeholder ("default")
  // and `legacyPalette` is undefined, so acting on either would be acting on a
  // value the user never chose.
  const appearanceReady = settingsLoaded && !syncedLoading && discovered;
  const typography = useMemo(
    () => resolveTypographySettings(typographyAppearance),
    [typographyAppearance],
  );

  useEffect(() => {
    useSettingsStore.getState().load();
    void useOmarchyStore.getState().load();
  }, []);

  useEffect(() => {
    if (appearanceReady && !savedSource) setThemeSource(source);
  }, [appearanceReady, savedSource, source]);

  // The inline boot script already painted the last applied theme, so until the
  // stores load there is nothing to do: applying here would repaint the shell
  // to Graphite and — because `applyTheme` persists by default — overwrite the
  // boot shadow with it, so a crash (or a failed load) during that window would
  // open the *next* launch on Graphite too. Density is a layout attribute, not
  // a color, and stays unconditional.
  useLayoutEffect(() => {
    const root = document.documentElement;
    if (appearanceReady) {
      // The palette owns its preview until it closes; desktop updates must
      // not repaint over the user's highlighted choice.
      if (!commandPaletteOpen) applyTheme(activeTheme);
      applyTypography(root, typography);
    }
    delete root.dataset.pal;
    root.dataset.density = density;
  }, [activeTheme, density, appearanceReady, typography, commandPaletteOpen]);

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

  // Picking a workspace or starting a new agent while a utility page is
  // open means "take me there", not "switch underneath the page". Every
  // workspace activation stamps `pendingActivationAt` — even re-picking
  // the workspace already under the page. A subscription, not a selector:
  // a fast backend confirms (and clears the stamp) before React renders.
  useEffect(
    () =>
      useAppStore.subscribe((state, prev) => {
        if (
          state.pendingActivationAt !== null &&
          state.pendingActivationAt !== prev.pendingActivationAt
        ) {
          useUIStore.getState().closeUtilityPages();
        }
      }),
    [],
  );
  const lastDraftId = useRef(activeDraftId);
  useEffect(() => {
    if (activeDraftId !== null && activeDraftId !== lastDraftId.current) {
      useUIStore.getState().closeUtilityPages();
    }
    lastDraftId.current = activeDraftId;
  }, [activeDraftId]);

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

  // Utility pages normally open beside the sidebar (see the main layout
  // below). With no sidebar to sit beside — on mobile, or before any
  // workspace exists — they take the whole window and bring their own
  // back arrow.
  const shellAvailable = hasWorkspaces || (lazyEnabled && hasActiveDraft);
  if (utilityPage && (mobile || !shellAvailable)) {
    return (
      <UtilityPageStandaloneContext.Provider value={true}>
        <LazyBoundary label={UTILITY_PAGE_LABEL[utilityPage]} className="h-screen">
          <UtilityPageContent page={utilityPage} />
        </LazyBoundary>
      </UtilityPageStandaloneContext.Provider>
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

  if (mobile) return <LazyBoundary label="mobile workspace" className="h-screen"><MobileShell overlays={<>
    {commandPaletteOpen && <LazyBoundary label="commands" presentation="overlay"><CommandPalette open onOpenChange={setCommandPaletteOpen}/></LazyBoundary>}
    {fileSearchOpen && <LazyBoundary label="file search" presentation="overlay"><FileSearchDialog/></LazyBoundary>}
    {contentSearchOpen && <LazyBoundary label="search" presentation="overlay"><ContentSearchDialog/></LazyBoundary>}
  </>} /></LazyBoundary>;

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
        width={sidebarWidth}
        onWidthChange={setSidebarWidth}
        className="flex-1 min-h-0"
      >
        <SidebarToggleBridge />
        <AppSidebar />
        <SidebarInset className="flex flex-col overflow-hidden h-full min-w-0">
          {/* The workspace stays mounted under a utility page, so Back is
              instant and terminals and chats keep their state. `inert`
              keeps focus and pointer input out of it meanwhile. */}
          <div
            className="flex min-h-0 min-w-0 flex-1 flex-col"
            inert={utilityPage !== null}
          >
            <WorkspaceMain />
          </div>
          {utilityPage && (
            <div className="absolute inset-0 z-20 flex flex-col bg-background">
              <LazyBoundary label={UTILITY_PAGE_LABEL[utilityPage]} className="h-full">
                <UtilityPageContent page={utilityPage} />
              </LazyBoundary>
            </div>
          )}
          {/* GUI-mode background browser peek — absolutely positioned
              inside this `relative` SidebarInset, so it floats over
              WorkspaceMain without resizing it. Renders nothing unless
              GUI chrome applies and the peek is explicitly opened. */}
          {browserPeekOpen && !utilityPage && (
            <LazyBoundary
              label="browser peek"
              className="absolute right-3.5 top-3.5 z-30 h-[300px] w-[440px] rounded-lg border border-border"
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

const UTILITY_PAGE_LABEL: Record<UtilityPage, string> = {
  automations: "Automations",
  devices: "Devices",
  "pull-requests": "Pull requests",
  usage: "Usage",
};

function UtilityPageContent({ page }: { page: UtilityPage }) {
  if (page === "automations") return <AutomationsView />;
  if (page === "devices") return <DevicesView />;
  if (page === "usage") return <UsageView />;
  return <PullRequestsView />;
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
