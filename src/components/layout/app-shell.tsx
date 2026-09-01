import { useState, useEffect, useLayoutEffect, useMemo } from "react";
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
import { BrowserPeekOverlay } from "@/components/browser/BrowserPeekOverlay";
import { AppSidebar } from "./app-sidebar";
import { TitleBar } from "./title-bar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";
import { SettingsView } from "@/components/settings/settings-view";
import { AutomationsView } from "@/components/automations/automations-view";
import { DevicesView } from "@/components/devices/devices-view";
import { PullRequestsView } from "@/components/pull-requests/pull-requests-view";
import { CommandPalette } from "@/components/overlays/command-palette";
import { NewProjectScreen } from "@/components/overlays/new-project-screen";
import { FileSearchDialog } from "@/components/search/file-search-dialog";
import { ContentSearchDialog } from "@/components/search/content-search-dialog";
import { useWorktreeIncludeToast } from "@/hooks/use-worktree-include-toast";

const EMPTY_THEME_PAYLOADS: unknown[] = [];

export function AppShell() {
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
    return <SettingsView />;
  }

  // Full-screen Automations — a first-class destination, like Settings
  if (showAutomations) {
    return <AutomationsView />;
  }

  // Full-screen Devices page — the account's other machines and the work
  // that lives on them. Same overlay shape as Settings / Automations.
  if (showDevices) {
    return <DevicesView />;
  }

  // Full-screen Pull requests — the review surface for work that isn't
  // in a workspace yet. Same overlay shape as the pages above it.
  if (showPullRequests) {
    return <PullRequestsView />;
  }

  // Full-screen new project — replaces entire app including sidebar
  if (showNewProjectScreen) {
    return <NewProjectScreen />;
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
          <BrowserPeekOverlay />
        </SidebarInset>
        <CommandPalette
          open={commandPaletteOpen}
          onOpenChange={setCommandPaletteOpen}
        />
        <FileSearchDialog />
        <ContentSearchDialog />
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
