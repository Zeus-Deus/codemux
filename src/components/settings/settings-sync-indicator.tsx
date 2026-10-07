import { useEffect, useRef, useState } from "react";
import { AlertCircle, Check, CloudOff, Loader2 } from "lucide-react";
import { cn } from "@/lib/utils";
import { useSyncedSettingsStore } from "@/stores/synced-settings-store";

/** How long "Saved" stays at full strength before it fades out. */
const SAVED_VISIBLE_MS = 2000;

/** Where the last account-synced change ended up: saving, saved, kept on
 *  this machine until the server is reachable, or not saved at all.
 *  Silent until the first change, so opening Settings shows nothing. */
export function SettingsSyncIndicator() {
  const isSyncing = useSyncedSettingsStore((s) => s.isSyncing);
  const syncIssue = useSyncedSettingsStore((s) => s.syncIssue);
  const [showSaved, setShowSaved] = useState(false);
  const wasSyncing = useRef(isSyncing);

  useEffect(() => {
    const finished = wasSyncing.current && !isSyncing;
    wasSyncing.current = isSyncing;
    if (!finished) return;
    setShowSaved(true);
    const timer = setTimeout(() => setShowSaved(false), SAVED_VISIBLE_MS);
    return () => clearTimeout(timer);
  }, [isSyncing]);

  let content: React.ReactNode = null;
  let tone = "text-muted-foreground";
  if (isSyncing) {
    content = (
      <>
        <Loader2 className="size-3.5 motion-safe:animate-spin" aria-hidden />
        Saving…
      </>
    );
  } else if (syncIssue === "saved-locally") {
    tone = "text-warning";
    content = (
      <>
        <CloudOff className="size-3.5" aria-hidden />
        Offline. Saved on this device, will sync
      </>
    );
  } else if (syncIssue === "failed") {
    tone = "text-destructive";
    content = (
      <>
        <AlertCircle className="size-3.5" aria-hidden />
        Couldn't save
      </>
    );
  } else {
    content = (
      <>
        <Check className="size-3.5" aria-hidden />
        Saved
      </>
    );
  }

  const quiet = !isSyncing && !syncIssue && !showSaved;
  return (
    <span aria-live="polite" className="flex">
      {/* "Saved" stays mounted while it fades; once faded it is hidden
          from assistive tech as well as from sight. */}
      <span
        aria-hidden={quiet || undefined}
        className={cn(
          "flex items-center gap-1.5 text-label transition-opacity duration-150 motion-reduce:transition-none",
          tone,
          quiet ? "opacity-0" : "opacity-100",
        )}
      >
        {content}
      </span>
    </span>
  );
}
