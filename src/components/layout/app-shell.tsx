import { useState, useEffect } from "react";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { useSettingsStore } from "@/stores/settings-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { SidebarProvider, SidebarInset } from "@/components/ui/sidebar";
import { AppSidebar } from "./app-sidebar";
import { TitleBar } from "./title-bar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";
import { SettingsView } from "@/components/settings/settings-view";
import { AutomationsView } from "@/components/automations/automations-view";
import { WorkspacesOverviewView } from "@/components/workspaces-overview/workspaces-overview-view";
import { CommandPalette } from "@/components/overlays/command-palette";
import { NewProjectScreen } from "@/components/overlays/new-project-screen";
import { FileSearchDialog } from "@/components/search/file-search-dialog";
import { ContentSearchDialog } from "@/components/search/content-search-dialog";
import { useWorktreeIncludeToast } from "@/hooks/use-worktree-include-toast";

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
  const showWorkspacesOverview = useUIStore((s) => s.showWorkspacesOverview);
  const showNewProjectScreen = useUIStore((s) => s.showNewProjectScreen);
  const commandPaletteOpen = useUIStore((s) => s.showCommandPalette);
  const setCommandPaletteOpen = useUIStore((s) => s.setShowCommandPalette);
  const [sidebarOpen, setSidebarOpen] = useState(true);

  useEffect(() => {
    useSettingsStore.getState().load();
  }, []);

  useWorktreeIncludeToast();

  // Register sidebar toggle in UIStore so the central keyboard hook can call it
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

  // Full-screen Workspaces overview — one pane to see every workspace
  // this device knows about, across local + every remote host. Same
  // overlay shape as Settings / Automations.
  if (showWorkspacesOverview) {
    return <WorkspacesOverviewView />;
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
    <div className="flex flex-col h-screen max-h-screen">
      <TitleBar
        sidebarOpen={sidebarOpen}
        onToggleSidebar={() => setSidebarOpen((o) => !o)}
      />
      <SidebarProvider
        open={sidebarOpen}
        onOpenChange={setSidebarOpen}
        className="flex-1 min-h-0"
      >
        <AppSidebar />
        <SidebarInset className="flex flex-col overflow-hidden h-full min-w-0">
          <WorkspaceMain />
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
