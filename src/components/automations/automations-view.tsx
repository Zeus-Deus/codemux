import { useEffect } from "react";

import { UtilityPage } from "@/components/layout/utility-page";
import { useUIStore } from "@/stores/ui-store";
import { AutomationsSection } from "./automations-section";

/**
 * The Automations page.
 *
 * A first-class destination reached from the left sidebar (not a
 * Settings sub-page) — the same placement Codex and Superset give it.
 * Opens beside the sidebar like the other utility pages.
 */
export function AutomationsView() {
  const setShowAutomations = useUIStore((s) => s.setShowAutomations);

  // Escape closes the view, matching the other utility pages.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") setShowAutomations(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setShowAutomations]);

  return (
    <UtilityPage
      title="Automations"
      backLabel="Close automations"
      onBack={() => setShowAutomations(false)}
    >
      <div className="min-h-0 flex-1 overflow-y-auto px-6 pb-6 pt-4">
        <div className="mx-auto max-w-5xl">
          <AutomationsSection />
        </div>
      </div>
    </UtilityPage>
  );
}
