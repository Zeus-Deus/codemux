import { useEffect, useMemo } from "react";
import { ArrowUpRight, Check, Download, LoaderCircle, X } from "lucide-react";
import { selectVisibleHealthReport, useProviderHealth } from "@/stores/provider-health-store";
import { useHermes } from "@/stores/hermes-store";
import { UPDATE_INTERVAL, updateIdentity, updateTargetKey, useProviderUpdates } from "@/stores/provider-update-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { ProviderLogo } from "./provider-logo";

const labels: Record<AgentChatProviderKind, string> = {
  claude: "Claude", codex: "Codex", cursor: "Cursor", grok: "Grok", hermes: "Hermes", opencode: "OpenCode",
};
const docs: Record<AgentChatProviderKind, string> = {
  claude: "https://code.claude.com/docs/en/setup",
  codex: "https://developers.openai.com/codex/cli/",
  cursor: "https://cursor.com/docs/cli/installation",
  grok: "https://www.npmjs.com/package/@xai-official/grok",
  hermes: "https://hermes-agent.nousresearch.com/docs/getting-started/updating",
  opencode: "https://opencode.ai/docs/cli/",
};

/** Passive, hourly version checks only. No model discovery or authentication probes. */
export function ProviderUpdateNotice({ provider, threadId, remote = false }: { provider: AgentChatProviderKind; threadId: string | null; remote?: boolean }) {
  const health = useProviderHealth(s => selectVisibleHealthReport(s, provider));
  const profile = useHermes(s => threadId ? s.selections[threadId] : undefined);
  const installation = provider === "hermes" ? profile?.installation : undefined;
  const enabled = !remote && !isRemoteClient() && (provider !== "hermes" || !!profile);
  const target = useMemo(() => ({ provider, installation }), [provider, installation]);
  const slot = useProviderUpdates(s => s.slots[updateTargetKey(target)]);
  const check = useProviderUpdates(s => s.check);
  const update = useProviderUpdates(s => s.update);
  const dismiss = useProviderUpdates(s => s.dismiss);
  useEffect(() => {
    if (!enabled) return;
    void check(target);
    const poll = () => { if (document.visibilityState === "visible") void check(target); };
    const timer = setInterval(poll, UPDATE_INTERVAL);
    document.addEventListener("visibilitychange", poll);
    return () => { clearInterval(timer); document.removeEventListener("visibilitychange", poll); };
  }, [target, check, enabled]);

  const report = slot?.report;
  if (!enabled || health || !report || (!report.available && !slot.updated)) return null;
  if (slot.dismissed === (slot.updated ? "updated" : updateIdentity(report))) return null;
  const label = labels[provider];
  return (
    <div className="pointer-events-none absolute inset-x-0 top-12 z-30 flex justify-end px-4">
      <section role="status" aria-label={`${label} update`} className="pointer-events-auto w-[340px] max-w-full rounded-xl border border-border bg-popover p-4 text-popover-foreground shadow-lg" data-testid="provider-update-notice">
        <div className="flex items-start gap-3">
          <div className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-border bg-muted/40">
            {slot.updated ? <Check className="size-4 text-success" /> : <ProviderLogo provider={provider} className="size-4" />}
          </div>
          <div className="min-w-0 flex-1">
            <p className="text-body-sm font-medium">{slot.updated ? `${label} updated` : `${label} update available`}</p>
            <p className="mt-0.5 text-caption text-muted-foreground">{slot.updated ? report.installed_version : `${report.installed_version} → ${report.latest_version}`}</p>
          </div>
          <button type="button" aria-label={`Dismiss ${label} update`} disabled={slot.updating} onClick={() => dismiss(target)} className="rounded-sm p-0.5 text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-30"><X className="size-3.5" /></button>
        </div>
        <p className="mt-3 text-caption leading-relaxed text-muted-foreground">
          {slot.updated ? "Restart Codemux when you're ready to use the new version. Your running chats have not been restarted."
            : report.manager === "Omarchy · mise" ? "Managed by Omarchy. Updates through mise with fresh version metadata."
            : report.can_update ? `Updates through ${report.manager}. Restart Codemux afterward to use the new version.`
            : report.message}
        </p>
        {slot.error && <p role="alert" className="mt-2 max-h-28 overflow-auto break-words text-caption text-destructive">{slot.error}</p>}
        {!slot.updated && <div className="mt-3 flex items-center justify-between gap-2">
          <span className="text-caption text-muted-foreground">{report.manager}</span>
          {report.can_update ? <button type="button" disabled={slot.updating} onClick={() => void update(target)} className="inline-flex shrink-0 items-center gap-1.5 rounded-md bg-primary px-3 py-1.5 text-caption font-medium text-primary-foreground transition-opacity duration-150 hover:opacity-90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-60">
            {slot.updating ? <LoaderCircle className="size-3 animate-spin" /> : <Download className="size-3" />}
            {slot.updating ? "Updating…" : slot.error ? "Try again" : "Update"}
          </button> : <a href={report.manager === "Omarchy system package" ? "https://omarchy.org/" : docs[provider]} target="_blank" rel="noreferrer" className="inline-flex items-center gap-1 text-caption text-primary">Update guide<ArrowUpRight className="size-3" /></a>}
        </div>}
      </section>
    </div>
  );
}
