import { useEffect } from "react";

import { ArrowLeft } from "lucide-react";

import { Button } from "@/components/ui/button";
import { WindowChrome } from "@/components/layout/window-chrome";
import { useUIStore } from "@/stores/ui-store";
import { WorkspacesOverviewSection } from "./workspaces-overview-section";

/**
 * Full-screen Workspaces overview.
 *
 * A first-class destination reached from the left sidebar — a single
 * pane that lists every workspace this device knows about (local +
 * each host it has pushed to), with filters, search, and per-row
 * push/pull/open actions.
 *
 * Mirrors `AutomationsView`'s full-screen chrome: a `WindowChrome`
 * drag strip, a back-button header, then the management panel.
 */
export function WorkspacesOverviewView() {
  const setShowWorkspacesOverview = useUIStore(
    (s) => s.setShowWorkspacesOverview,
  );

  // Escape closes the view, matching the other full-screen overlays.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setShowWorkspacesOverview(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setShowWorkspacesOverview]);

  return (
    <div className="relative flex h-screen flex-col bg-background">
      <WindowChrome />
      {/* `pt-7` (= the 28px WindowChrome drag strip) keeps the back
          button's hit area entirely below the drag region. */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 pt-7 pb-2">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Close workspaces"
          className="text-muted-foreground hover:text-foreground hover:bg-muted/50"
          onClick={() => setShowWorkspacesOverview(false)}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <span className="text-[13px] font-medium text-foreground">
          Workspaces
        </span>
      </div>

      <div className="flex-1 min-h-0 overflow-hidden">
        <WorkspacesOverviewSection />
      </div>
    </div>
  );
}
