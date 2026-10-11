import { useEffect } from "react";

import { UtilityPage } from "@/components/layout/utility-page";
import { escapeClaimedElsewhere } from "@/lib/escape-guard";
import { useUIStore } from "@/stores/ui-store";
import { DevicesSection } from "./devices-section";

/**
 * The Devices page — the account's other machines and the work that
 * lives on them. Local workspaces stay in the sidebar; this page only
 * moves work between devices.
 */
export function DevicesView() {
  const setShowDevices = useUIStore((s) => s.setShowDevices);

  // Escape closes the view, matching the other utility pages — but not
  // while the sweep or pull dialog owns the key: unmounting the page
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
    <UtilityPage
      title="Devices"
      backLabel="Close devices"
      onBack={() => setShowDevices(false)}
      actions={
        <span className="hidden truncate text-label text-muted-foreground/70 lg:inline">
          Remote Control lets you <em>use</em> another device — this page
          moves work <em>between</em> them.
        </span>
      }
    >
      <div className="min-h-0 flex-1 overflow-hidden">
        <DevicesSection />
      </div>
    </UtilityPage>
  );
}
