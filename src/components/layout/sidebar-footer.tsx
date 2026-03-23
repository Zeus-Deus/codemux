import { SidebarFooter as ShadcnSidebarFooter } from "@/components/ui/sidebar";

export function SidebarFooter() {
  return (
    <ShadcnSidebarFooter className="border-t border-sidebar-border p-2">
      <div className="flex items-center justify-between text-xs text-muted-foreground">
        <span>Codemux</span>
        <kbd className="text-[10px] opacity-50">⌘B</kbd>
      </div>
    </ShadcnSidebarFooter>
  );
}
