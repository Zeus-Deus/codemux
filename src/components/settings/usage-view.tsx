import { useEffect } from "react";

import { UtilityPage } from "@/components/layout/utility-page";
import { escapeClaimedElsewhere } from "@/lib/escape-guard";
import { useUIStore } from "@/stores/ui-store";
import { UsageSection } from "./usage-section";

/**
 * The Usage page — the same ledger as Settings ▸ Usage, opened beside the
 * sidebar from the footer or `/usage`, so checking spend doesn't take you
 * out of the workspace you were in.
 */
export function UsageView() {
  const setShowUsage = useUIStore((s) => s.setShowUsage);

  // Escape closes the view, matching the other utility pages — unless a
  // menu or field on the page owns the key.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || escapeClaimedElsewhere(event)) return;
      setShowUsage(false);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [setShowUsage]);

  return (
    <UtilityPage title="Usage" backLabel="Close usage" onBack={() => setShowUsage(false)}>
      <div className="thin-scrollbar min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
        <div className="mx-auto w-full max-w-6xl px-5 pb-12 pt-3 sm:px-6">
          <UsageSection showTitle={false} />
        </div>
      </div>
    </UtilityPage>
  );
}
