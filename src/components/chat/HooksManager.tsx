import { useEffect, useRef, useState } from "react";
import { RotateCw } from "lucide-react";

import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { agentChatHooks, type NativeHooksList, type NativeHookUpdate } from "@/tauri/commands";
import { cn } from "@/lib/utils";

interface HooksManagerProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  cwd: string | null;
  threadId: string | null;
}

/** /hooks is a native management action, not a prompt or a skill. */
export function HooksManager({ open, onOpenChange, cwd, threadId }: HooksManagerProps) {
  const [catalogue, setCatalogue] = useState<NativeHooksList | null>(null);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [revision, setRevision] = useState(0);
  const requestId = useRef(0);

  useEffect(() => {
    const id = ++requestId.current;
    setCatalogue(null);
    setSelectedKey(null);
    setError(null);
    setBusy(open);
    if (open) {
      void agentChatHooks("codex", cwd, threadId).then((result) => {
        if (id !== requestId.current) return;
        setCatalogue(result);
        setSelectedKey(result.hooks[0]?.key ?? null);
      }).catch((failure: unknown) => {
        if (id === requestId.current) setError(String(failure));
      }).finally(() => {
        if (id === requestId.current) setBusy(false);
      });
    }
    return () => { ++requestId.current; };
  }, [open, cwd, threadId, revision]);

  const update = async (action: NativeHookUpdate) => {
    const id = ++requestId.current;
    setBusy(true);
    setError(null);
    try {
      const result = await agentChatHooks("codex", cwd, threadId, action);
      if (id === requestId.current) setCatalogue(result);
    } catch (failure) {
      if (id === requestId.current) setError(String(failure));
    } finally {
      if (id === requestId.current) setBusy(false);
    }
  };

  const selected = catalogue?.hooks.find((hook) => hook.key === selectedKey);
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-3xl">
        <DialogHeader className="pr-8">
          <DialogTitle>Codex hooks</DialogTitle>
          <DialogDescription>View lifecycle hooks and manage their trust and enabled state.</DialogDescription>
        </DialogHeader>
        <div className="flex items-center gap-3 border-b pb-3">
          <span className="min-w-0 flex-1 truncate font-mono text-label text-muted-foreground" title={catalogue?.cwd ?? cwd ?? "Home"}>{catalogue?.cwd ?? cwd ?? "Home"}</span>
          <Button size="sm" variant="ghost" disabled={busy} aria-busy={busy} onClick={() => setRevision((value) => value + 1)}><RotateCw className={cn("size-3.5", busy && "animate-spin")} />Refresh</Button>
        </div>
        {error && <p role="alert" className="break-words text-destructive">{error}</p>}
        {catalogue?.warnings.map((warning, index) => <p key={index} className="break-words text-muted-foreground">{warning}</p>)}
        {catalogue?.errors.map((failure, index) => <p key={index} role="alert" className="break-words text-destructive">{failure.path}: {failure.message}</p>)}
        {!catalogue && busy && <p role="status" className="py-6 text-muted-foreground">Loading hooks…</p>}
        {catalogue?.hooks.length === 0 && <p className="py-6 text-muted-foreground">No lifecycle hooks found in this directory. Add hooks through Codex configuration or an installed plugin, then refresh.</p>}
        {!!catalogue?.hooks.length && <div className="grid min-h-0 gap-4 overflow-y-auto sm:grid-cols-[220px_minmax(0,1fr)]">
          <div className="space-y-1" aria-label="Lifecycle hooks">
            {catalogue.hooks.map((hook) => <button key={hook.key} type="button" disabled={busy} aria-pressed={selectedKey === hook.key} onClick={() => setSelectedKey(hook.key)} className={cn("w-full rounded-md px-3 py-2 text-left hover:bg-accent disabled:opacity-60", selectedKey === hook.key && "bg-accent")}>
              <span className="block font-medium">{hook.eventName.replace(/^./, (letter) => letter.toUpperCase())}</span>
              <span className="mt-1 block text-label text-muted-foreground">{hook.source} · {hook.isManaged ? "Managed" : hook.trustStatus} · {hook.enabled ? "Enabled" : "Disabled"}</span>
            </button>)}
          </div>
          {selected && <div className="min-w-0 space-y-4 rounded-md border p-4">
            <div>
              <h3 className="font-medium">{selected.eventName.replace(/^./, (letter) => letter.toUpperCase())}</h3>
              <p className="mt-1 break-all font-mono text-label text-muted-foreground">{selected.sourcePath}</p>
            </div>
            <div>
              <p className="mb-2 text-label text-muted-foreground">{selected.handlerType === "mcpTool" ? "MCP tool" : "Command"}</p>
              <pre className="whitespace-pre-wrap break-all rounded-md bg-muted p-3 font-mono text-label">{selected.command ?? (selected.server && selected.tool ? `${selected.server}/${selected.tool}` : selected.handlerType)}</pre>
            </div>
            <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-label">
              <dt className="text-muted-foreground">Matcher</dt><dd className="break-all">{selected.matcher || "All matching events"}</dd>
              <dt className="text-muted-foreground">Timeout</dt><dd>{selected.timeoutSec}s{selected.async ? " · Asynchronous" : ""}</dd>
              {selected.additionalContextLimit != null && <><dt className="text-muted-foreground">Context limit</dt><dd>{selected.additionalContextLimit}</dd></>}
              <dt className="text-muted-foreground">Trust</dt><dd className="capitalize">{selected.trustStatus}</dd>
            </dl>
            {selected.statusMessage && <p className="break-words text-muted-foreground">{selected.statusMessage}</p>}
            {selected.isManaged ? <p className="text-muted-foreground">This hook is managed by your administrator.</p> : <>
              {(selected.trustStatus === "untrusted" || selected.trustStatus === "modified") && <p className="text-muted-foreground">Review the definition above. Trusting it allows Codex to run this version when its lifecycle event occurs.</p>}
              <div className="flex flex-wrap gap-2">
                {(selected.trustStatus === "untrusted" || selected.trustStatus === "modified") && <Button size="sm" disabled={busy} onClick={() => void update({ action: "trust", key: selected.key, hash: selected.currentHash })}>Trust this hook</Button>}
                <Button size="sm" variant="outline" disabled={busy || (!selected.enabled && selected.trustStatus !== "trusted")} onClick={() => void update({ action: "setEnabled", key: selected.key, hash: selected.currentHash, enabled: !selected.enabled })}>{selected.enabled ? "Disable hook" : "Enable hook"}</Button>
              </div>
            </>}
          </div>}
        </div>}
      </DialogContent>
    </Dialog>
  );
}
