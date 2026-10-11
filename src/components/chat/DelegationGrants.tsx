import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { delegationGrants, delegationRevoke, type Grant } from "@/lib/delegation";
import { isDelegationPaneOwner, refreshDelegationTasks } from "@/stores/delegation-store";

interface Props { paneId: string; threadId: string; writable: boolean }
export function DelegationGrants({ paneId, threadId, writable }: Props) {
  const [grants, setGrants] = useState<Grant[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const generation = useRef(0);
  const pending = useRef(false);
  const latest = useRef({ paneId, threadId, writable });
  latest.current = { paneId, threadId, writable };
  useEffect(() => {
    generation.current++;
    pending.current = false;
    setGrants(null);
    setError(null);
    setBusy(false);
    return () => { generation.current++; };
  }, [paneId, threadId]);
  const active = (token: number) => generation.current === token && latest.current.paneId === paneId && latest.current.threadId === threadId && latest.current.writable && isDelegationPaneOwner(paneId, threadId);
  const read = async (revoke?: Grant) => {
    if (!writable || pending.current || !isDelegationPaneOwner(paneId, threadId)) return;
    pending.current = true;
    const token = generation.current;
    setBusy(true);
    setError(null);
    try {
      if (revoke) await delegationRevoke(paneId, revoke.id);
      if (!active(token)) return;
      const confirmed = await delegationGrants(paneId);
      if (!active(token)) return;
      if (revoke && !confirmed.some(grant => grant.id === revoke.id && !grant.enabled)) throw new Error("Revocation not confirmed. Refresh access before retrying.");
      setGrants(confirmed);
      if (revoke) await refreshDelegationTasks(paneId, threadId);
    } catch (cause) {
      if (active(token)) setError(cause instanceof Error ? cause.message : String(cause));
    } finally {
      if (generation.current === token) { pending.current = false; setBusy(false); }
    }
  };
  return <details className="rounded-lg border border-border bg-surface-1 px-3 py-2 text-body-sm" onToggle={event => { if (event.currentTarget.open && grants === null) void read(); }}>
    <summary className="cursor-pointer font-medium">Authorized targets in this workspace</summary>
    <p className="mt-2 text-muted-foreground">Access applies to conversations in this workspace, within their permission ceilings. Revoke blocks future delegation and requests stop for this target’s unfinished tasks.</p>
    {error && <p role="alert" className="mt-2 break-words text-destructive">{error}</p>}
    {grants?.map(grant => <div key={grant.id} className="mt-3 flex items-start gap-3">
      <div className="min-w-0 flex-1"><p className="font-medium">{grant.host_name} · {grant.workspace_name}</p><p className="break-all text-muted-foreground">{grant.workspace_path}</p><p className="text-label text-muted-foreground">{grant.provider} · {grant.permission_mode}</p></div>
      {grant.enabled ? <Button type="button" variant="ghost" size="sm" aria-label={`Revoke ${grant.host_name} access`} disabled={busy || !writable} onClick={() => void read(grant)}>Revoke</Button> : <span className="text-muted-foreground">Revoked</span>}
    </div>)}
    {grants?.length === 0 && <p className="mt-2 text-muted-foreground">No targets authorized yet.</p>}
    <Button type="button" variant="ghost" size="sm" className="mt-2" disabled={busy || !writable} onClick={() => void read()}>{busy ? "Checking access…" : "Refresh access"}</Button>
  </details>;
}
