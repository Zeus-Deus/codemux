import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useMobileLayout } from "@/hooks/use-mobile-layout";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore } from "@/stores/ui-store";
import { useAppStore } from "@/stores/app-store";
import { Button } from "@/components/ui/button";

const INLINE_LINK =
  "rounded-sm text-foreground/80 underline-offset-4 transition-colors duration-100 hover:text-foreground hover:underline";

export function LocalSessionImportEntry({ firstRun = false }: { firstRun?: boolean }) {
  const enabled = useFeatureFlags((s) => s.enableAgentChat);
  const mobile = useMobileLayout();
  const empty = useAppStore((s) => s.appState !== null && (s.appState.workspaces?.length ?? 0) === 0);
  const dismissed = useUIStore((s) => s.localSessionImportOfferDismissed);
  const dismiss = useUIStore((s) => s.dismissLocalSessionImportOffer);
  const setOpen = useUIStore((s) => s.setShowLocalSessionImport);
  if (!enabled || isRemoteClient() || mobile || (firstRun && (!empty || dismissed))) return null;
  // First run: one quiet line under the project actions, which are the
  // primary way in. Import is an aside for people arriving with history.
  if (firstRun) {
    return <p className="px-4 text-center text-body-sm text-muted-foreground">
      Already working with Claude Code or Codex?{" "}
      <button type="button" className={INLINE_LINK} onClick={() => setOpen(true)}>Import recent chats</button>
      <span aria-hidden className="mx-1.5 text-muted-foreground/50">·</span>
      <button type="button" className={INLINE_LINK} onClick={dismiss}>Not now</button>
    </p>;
  }
  return <div className="space-y-2 text-body text-muted-foreground">
    <p>Copy recent Claude Code and Codex chats from this computer into read-only conversations.</p>
    <div className="flex items-center gap-2">
      <Button variant="ghost" size="sm" onClick={() => setOpen(true)}>Import recent chats</Button>
    </div>
  </div>;
}
