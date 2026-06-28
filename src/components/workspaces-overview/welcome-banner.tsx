import { useState } from "react";

import { ArrowRight, Sparkles, X } from "lucide-react";

import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";

const DISMISS_KEY = "codemux.workspaces.welcomeDismissed";

interface Props {
  /** Number of configured devices on this account. Drives which
   *  variant of the welcome copy renders. */
  deviceCount: number;
  /** Number of sibling-device workspaces visible (i.e. rows with
   *  workspace_id null — owned by another device of this account). */
  siblingWorkspaceCount: number;
  /** Number of LOCAL workspaces (in app_state). Used to pick the
   *  brand-new-to-this-device variant. */
  localWorkspaceCount: number;
}

/**
 * First-run welcome banner for the Workspaces overview. Renders one
 * of three variants based on the user's state, dismissable, persists
 * dismissal per-install in localStorage.
 *
 * Variants:
 * - **fresh**: no devices configured, no sibling workspaces. The
 *   user just installed and signed in for the first time. Banner
 *   teaches the core model (push to a device → shows up everywhere)
 *   and offers an `Add a device →` CTA.
 * - **device-no-siblings**: at least one device configured but
 *   nothing has been pushed yet. Banner nudges them to try pushing.
 * - **has-siblings**: there's content from other devices waiting to
 *   be pulled. Banner counts the visible workspaces and tells the
 *   user how to bring them over.
 *
 * Dismissal is per-install (one localStorage key). Reinstall clears
 * it — acceptable. Machine-id-scoped keys would be more precise but
 * the wider key is simpler and the cost of re-showing on a fresh
 * install is negligible.
 */
export function WelcomeBanner({
  deviceCount,
  siblingWorkspaceCount,
  localWorkspaceCount,
}: Props) {
  const [dismissed, setDismissed] = useState<boolean>(() => {
    try {
      return localStorage.getItem(DISMISS_KEY) === "1";
    } catch {
      return false;
    }
  });
  const setShowSettings = useUIStore((s) => s.setShowSettings);
  const setShowWorkspacesOverview = useUIStore(
    (s) => s.setShowWorkspacesOverview,
  );

  // If the user reaches an "interesting" state (gains a sibling
  // workspace) after they dismissed the banner, we DON'T re-show
  // it — the dismissal is final. Re-showing would be intrusive.
  if (dismissed) return null;

  const handleDismiss = () => {
    try {
      localStorage.setItem(DISMISS_KEY, "1");
    } catch {
      // localStorage disabled — just hide for this session.
    }
    setDismissed(true);
  };

  // Pick the variant. Order matters: sibling-presence wins over
  // device-presence (more useful signal), then device-presence,
  // then brand-new fallback.
  let body: React.ReactNode;
  let cta: React.ReactNode = null;

  if (siblingWorkspaceCount > 0) {
    body = (
      <>
        You have{" "}
        <span className="font-medium text-foreground">
          {siblingWorkspaceCount}{" "}
          {siblingWorkspaceCount === 1 ? "workspace" : "workspaces"}
        </span>{" "}
        on your other devices. Use the row's <code>⋯</code> menu to pull
        any of them here. Workspace files copy on demand — they never
        sync silently behind your back.
      </>
    );
  } else if (deviceCount > 0) {
    body = (
      <>
        You have a device set up. Try{" "}
        <span className="font-medium text-foreground">
          right-click any workspace → Push to your device
        </span>
        . It'll show up here on every device of your account, and you
        can pull it back any time.
      </>
    );
  } else {
    body = (
      <>
        This is where your work lives across devices. Add a device —
        your home desktop, an always-on box, or a cloud server — then
        push a workspace to it. You'll see it here on every device you
        sign in to.
      </>
    );
    cta = (
      <Button
        variant="secondary"
        size="sm"
        className="h-7 gap-1.5 px-2.5 text-[12px]"
        onClick={() => {
          // Send the user to Settings → Devices. The overview
          // overlay closes so the settings overlay can open
          // cleanly (mutually exclusive in app-shell.tsx).
          setShowWorkspacesOverview(false);
          setShowSettings(true, "hosts");
        }}
      >
        Add a device
        <ArrowRight className="size-3" />
      </Button>
    );
  }

  // Local-only quick-orientation: brand-new user who has zero
  // anything benefits from the most reassuring tone; users with
  // many local workspaces don't need to be told what a workspace
  // is. We surface a different verb in the headline accordingly.
  const headline =
    siblingWorkspaceCount > 0
      ? "Welcome to Codemux on this device"
      : localWorkspaceCount > 0
        ? "Workspaces, now across all your devices"
        : "Welcome to Workspaces";

  return (
    <div
      className={cn(
        "mx-auto max-w-[1180px] flex items-start gap-3 rounded-[12px] border border-sky-500/25 bg-sky-500/8 px-4 py-3.5 mb-5",
      )}
    >
      <Sparkles className="mt-0.5 size-4 shrink-0 text-sky-400" />
      <div className="min-w-0 flex-1 space-y-1.5">
        <h3 className="text-[13px] font-semibold text-foreground">
          {headline}
        </h3>
        <p className="text-[12px] leading-relaxed text-muted-foreground/85">
          {body}
        </p>
        {cta && <div className="pt-1">{cta}</div>}
      </div>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Dismiss welcome message"
        className="size-6 shrink-0 text-muted-foreground/60 hover:text-foreground"
        onClick={handleDismiss}
      >
        <X className="size-3" />
      </Button>
    </div>
  );
}

/** Test helper: reset the dismissal flag so the banner re-renders. */
export function __resetWelcomeBannerDismissalForTests() {
  try {
    localStorage.removeItem(DISMISS_KEY);
  } catch {
    // no-op
  }
}

// Also useful in tests + dev.
export const WELCOME_BANNER_DISMISS_KEY = DISMISS_KEY;
