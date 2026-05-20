import { useEffect } from "react";

import { ArrowLeft } from "lucide-react";

import { Button } from "@/components/ui/button";
import { WindowChrome } from "@/components/layout/window-chrome";
import { useUIStore } from "@/stores/ui-store";
import { AutomationsSection } from "./automations-section";

/**
 * Full-screen Automations view.
 *
 * A first-class destination reached from the left sidebar (not a
 * Settings sub-page) — the same placement Codex and Superset give it.
 * Mirrors `SettingsView`'s full-screen chrome: a `WindowChrome` drag
 * strip, a back-button header, then the management panel.
 */
export function AutomationsView() {
  const setShowAutomations = useUIStore((s) => s.setShowAutomations);

  // Escape closes the view, matching the other full-screen overlays.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setShowAutomations(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setShowAutomations]);

  return (
    <div className="relative flex h-screen flex-col bg-background">
      <WindowChrome />
      {/* `pt-7` (= the 28px WindowChrome drag strip) keeps the back
          button's hit area entirely below the drag region. */}
      <div className="flex shrink-0 items-center gap-2 border-b border-border px-3 pt-7 pb-2">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Close automations"
          className="text-muted-foreground hover:text-foreground hover:bg-muted/50"
          onClick={() => setShowAutomations(false)}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <span className="text-[13px] font-medium text-foreground">
          Automations
        </span>
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto px-6 py-6">
        <div className="mx-auto max-w-5xl">
          <AutomationsSection />
        </div>
      </div>
    </div>
  );
}
