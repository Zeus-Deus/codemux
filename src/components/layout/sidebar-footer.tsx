import { SidebarFooter as ShadcnSidebarFooter } from "@/components/ui/sidebar";

export function SidebarFooter() {
  return (
    <ShadcnSidebarFooter className="border-t border-sidebar-border p-1.5">
      <div className="flex items-center text-xs text-muted-foreground">
        <span>Codemux</span>
      </div>
    </ShadcnSidebarFooter>
  );
}
