import { useState, useEffect } from "react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { SidebarProvider, SidebarInset } from "@/components/ui/sidebar";
import { AppSidebar } from "./app-sidebar";
import { TitleBar } from "./title-bar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";
import { SettingsView } from "@/components/settings/settings-view";
import { CommandPalette } from "@/components/overlays/command-palette";
import { FileSearchDialog } from "@/components/search/file-search-dialog";
import { ContentSearchDialog } from "@/components/search/content-search-dialog";

export function AppShell() {
  const isLoading = useAppStore((s) => s.appState === null);
  const hasWorkspaces = useAppStore(
    (s) => (s.appState?.workspaces.length ?? 0) > 0,
  );
  const showSettings = useUIStore((s) => s.showSettings);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.ctrlKey && e.key === "k") {
        e.preventDefault();
        setCommandPaletteOpen((o) => !o);
      }
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, []);

  if (isLoading) {
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
          {hasWorkspaces ? <WorkspaceMain /> : <EmptyState />}
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
