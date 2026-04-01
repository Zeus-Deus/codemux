import { useState, useEffect } from "react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { useSettingsStore } from "@/stores/settings-store";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";
import { SidebarProvider, SidebarInset } from "@/components/ui/sidebar";
import { AppSidebar } from "./app-sidebar";
import { TitleBar } from "./title-bar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";
import { SettingsView } from "@/components/settings/settings-view";
import { CommandPalette } from "@/components/overlays/command-palette";
import { NewProjectScreen } from "@/components/overlays/new-project-screen";
import { FileSearchDialog } from "@/components/search/file-search-dialog";
import { ContentSearchDialog } from "@/components/search/content-search-dialog";
import { UpdateToast } from "@/components/update/update-toast";

export function AppShell() {
  const isLoading = useAppStore((s) => s.appState === null);
  const settingsLoaded = useSettingsStore((s) => s.loaded);
  const syncedLoading = useSyncedSettingsStore((s) => s.isLoading);
  const hasWorkspaces = useAppStore(
    (s) => (s.appState?.workspaces.length ?? 0) > 0,
  );
  const showSettings = useUIStore((s) => s.showSettings);
  const showNewProjectScreen = useUIStore((s) => s.showNewProjectScreen);
  const commandPaletteOpen = useUIStore((s) => s.showCommandPalette);
  const setCommandPaletteOpen = useUIStore((s) => s.setShowCommandPalette);
  const [sidebarOpen, setSidebarOpen] = useState(true);

  useEffect(() => {
    useSettingsStore.getState().load();
  }, []);

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

  // Full-screen new project — replaces entire app including sidebar
  if (showNewProjectScreen) {
    return <NewProjectScreen />;
  }

  // Full-screen empty state — no sidebar, no title bar
  if (!hasWorkspaces) {
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
      <UpdateToast />
    </div>
  );
}
