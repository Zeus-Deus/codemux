import { useAppStore } from "@/stores/app-store";
import { SidebarProvider, SidebarInset } from "@/components/ui/sidebar";
import { AppSidebar } from "./app-sidebar";
import { WorkspaceMain } from "./workspace-main";
import { EmptyState } from "./empty-state";

export function AppShell() {
  const isLoading = useAppStore((s) => s.appState === null);
  const hasWorkspaces = useAppStore(
    (s) => (s.appState?.workspaces.length ?? 0) > 0,
  );

  if (isLoading) {
    return (
      <div className="flex h-screen items-center justify-center bg-background text-muted-foreground">
        Loading…
      </div>
    );
  }

  return (
    <SidebarProvider defaultOpen>
      <AppSidebar />
      <SidebarInset className="flex flex-col overflow-hidden">
        {hasWorkspaces ? <WorkspaceMain /> : <EmptyState />}
      </SidebarInset>
    </SidebarProvider>
  );
}
