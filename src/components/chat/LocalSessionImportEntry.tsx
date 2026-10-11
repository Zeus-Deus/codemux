import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useMobileLayout } from "@/hooks/use-mobile-layout";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { Button } from "@/components/ui/button";

/** Importing reads chat history from this computer, so it needs agent chat,
 *  the desktop app itself (not a web remote client) and the desktop layout. */
export function useCanImportLocalSessions(): boolean {
  const enabled = useFeatureFlags((s) => s.enableAgentChat);
  const mobile = useMobileLayout();
  return enabled && !isRemoteClient() && !mobile;
}

export function LocalSessionImportEntry({ firstRun = false }: { firstRun?: boolean }) {
  const canImport = useCanImportLocalSessions();
  const empty = useAppStore((s) => s.appState !== null && (s.appState.workspaces?.length ?? 0) === 0);
  const dismissed = useUIStore((s) => s.localSessionImportOfferDismissed);
  const dismiss = useUIStore((s) => s.dismissLocalSessionImportOffer);
  const setOpen = useUIStore((s) => s.setShowLocalSessionImport);
  if (!canImport || (firstRun && (!empty || dismissed))) return null;
  return <div className="space-y-2 text-body text-muted-foreground">
    <p>{firstRun ? "Already working with Claude Code or Codex?" : "Copy recent Claude Code and Codex chats from this computer into read-only conversations."}</p>
    <div className="flex items-center gap-2">
      <Button variant="ghost" size="sm" onClick={() => setOpen(true)}>Import recent chats</Button>
      {firstRun && <Button variant="ghost" size="sm" onClick={dismiss}>Not now</Button>}
    </div>
  </div>;
}
