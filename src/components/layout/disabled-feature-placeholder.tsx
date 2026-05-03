import { Sparkles } from "lucide-react";

import { Button } from "@/components/ui/button";
import { useUIStore } from "@/stores/ui-store";

interface DisabledFeaturePlaceholderProps {
  /** User-facing feature name shown in the headline (e.g. "Agent
   *  Chat"). Match the label in `BetaFeaturesSection` so the user
   *  knows where to flip it back on. */
  feature: string;
  /** One-line explanation of why this pane is suspended. Defaults to
   *  the standard Beta-toggle copy; override only when the suspension
   *  has a different cause. */
  description?: string;
}

/**
 * Step 13 — placeholder shown in place of `AgentChatPane` (and any
 * future Beta-only pane kind) when the master Beta toggle is off but
 * the persisted layout still references that pane kind. Hides the
 * pane chrome instead of crashing on mount, and offers a CTA that
 * jumps the user straight to Settings → Beta Features so they can
 * flip it back on if they meant to.
 *
 * Data preservation contract: this component renders, but does not
 * touch the underlying pane state. The pane node remains in the
 * persisted tree — flip the toggle back on, refresh, and the pane
 * remounts with its session intact.
 */
export function DisabledFeaturePlaceholder({
  feature,
  description = "This pane needs the Agent Chat Beta. Your data is preserved — enable it in Settings to use this pane again.",
}: DisabledFeaturePlaceholderProps) {
  const setShowSettings = useUIStore((s) => s.setShowSettings);

  const openSettings = () => {
    setShowSettings(true, "beta_features");
  };

  return (
    <div className="flex h-full w-full items-center justify-center p-6">
      <div className="max-w-sm space-y-4 text-center">
        <div className="mx-auto flex size-10 items-center justify-center rounded-full border border-warning/30 bg-warning/5">
          <Sparkles className="size-5 text-warning" />
        </div>
        <div className="space-y-1.5">
          <h3 className="text-sm font-medium">{feature} is disabled</h3>
          <p className="text-xs text-muted-foreground leading-relaxed">
            {description}
          </p>
        </div>
        <Button
          size="sm"
          variant="outline"
          onClick={openSettings}
          className="border-warning/30 hover:bg-warning/10 hover:text-warning"
        >
          Open Settings → Beta Features
        </Button>
      </div>
    </div>
  );
}
