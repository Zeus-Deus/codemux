import { useState, useEffect } from "react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { SidebarProvider, SidebarInset } from "@/components/ui/sidebar";
import { AppSidebar } from "./app-sidebar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";
import { SettingsView } from "@/components/settings/settings-view";
import { CommandPalette } from "@/components/overlays/command-palette";

export function AppShell() {
  const isLoading = useAppStore((s) => s.appState === null);
  const hasWorkspaces = useAppStore(
    (s) => (s.appState?.workspaces.length ?? 0) > 0,
  );
  const showSettings = useUIStore((s) => s.showSettings);
  const [commandPaletteOpen, setCommandPaletteOpen] = useState(false);

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
    <SidebarProvider defaultOpen className="h-screen max-h-screen">
      <AppSidebar />
      <SidebarInset className="flex flex-col overflow-hidden h-full min-w-0">
        {hasWorkspaces ? <WorkspaceMain /> : <EmptyState />}
      </SidebarInset>
      <CommandPalette
        open={commandPaletteOpen}
        onOpenChange={setCommandPaletteOpen}
      />
    </SidebarProvider>
  );
}
