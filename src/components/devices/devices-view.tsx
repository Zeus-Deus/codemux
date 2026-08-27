import { useEffect } from "react";

import { ArrowLeft } from "lucide-react";

import { Button } from "@/components/ui/button";
import { WindowChrome } from "@/components/layout/window-chrome";
import { escapeClaimedElsewhere } from "@/lib/escape-guard";
import { useUIStore } from "@/stores/ui-store";
import { DevicesSection } from "./devices-section";

/**
 * Full-screen Devices page — the account's other machines and the work
 * that lives on them. Local workspaces stay in the sidebar; this page only
 * moves work between devices.
 *
 * Mirrors `AutomationsView`'s chrome: a `WindowChrome` drag strip, a
 * back-button header, then the body.
 */
export function DevicesView() {
  const setShowDevices = useUIStore((s) => s.setShowDevices);

  // Escape closes the view, matching the other full-screen overlays — but
  // not while the sweep or pull dialog owns the key: unmounting the page
  // mid-transfer would take the dialog's spinner and outcome toast wiring
  // with it.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || escapeClaimedElsewhere(event)) return;
      setShowDevices(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setShowDevices]);

  return (
    <div className="relative flex h-screen flex-col bg-background">
      <WindowChrome />
      {/* `pt-7` (= the 28px WindowChrome drag strip) keeps the back
          button's hit area entirely below the drag region. */}
      <div className="flex shrink-0 items-center gap-2 px-3 pt-7 pb-2">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label="Close devices"
          className="text-muted-foreground hover:text-foreground hover:bg-foreground/[0.06]"
          onClick={() => setShowDevices(false)}
        >
          <ArrowLeft className="h-4 w-4" />
        </Button>
        <span className="text-[13px] font-bold tracking-tight text-foreground">
          Devices
        </span>
        <span className="ml-auto hidden truncate text-[11px] text-muted-foreground/70 sm:inline">
          Remote Control lets you <em>use</em> another device — this page
          moves work <em>between</em> them.
        </span>
      </div>

      <div className="flex-1 min-h-0 overflow-hidden">
        <DevicesSection />
      </div>
    </div>
  );
}
