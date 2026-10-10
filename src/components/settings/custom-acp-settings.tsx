import { useCustomAcpEvents } from "@/hooks/use-custom-acp-events";
import { useEffect, useState } from "react";
import { Plus, Pencil, Trash2, FlaskConical, LoaderCircle, Terminal } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { acpDeleteAgent, acpSaveAgent, type AcpAgent, type AcpAgentInput } from "@/tauri/custom-acp";
import { acpCatalogKey, useCustomAcp } from "@/stores/custom-acp-store";

const emptyAgent = (): AcpAgentInput => ({ name: "", executable: "", args: [], environment: {}, enabled: true, auth_method: null });
type EnvRow = { key: string; value: string | null };
/** Secrets exist only in this unsaved editor, never in renderer persistence. */
export function CustomAcpSettings() {
  const warning = useCustomAcpEvents();
  const agents = useCustomAcp(s => s.agents);
  const catalogs = useCustomAcp(s => s.catalogs);
  const [editor, setEditor] = useState<AcpAgentInput | null>(null);
  const [environment, setEnvironment] = useState<EnvRow[]>([]);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [probeError, setProbeError] = useState<Record<string, string>>({});
  useEffect(() => { let alive = true; void useCustomAcp.getState().loadAgents().catch(e => { if (alive) setError(String(e)); }); return () => { alive = false; }; }, []);
  const edit = (agent?: AcpAgent) => {
    setError(null);
    setEditor(agent ? { id: agent.id, name: agent.name, executable: agent.executable, args: [...agent.args], environment: {}, enabled: agent.enabled, auth_method: agent.auth_method } : emptyAgent());
    setEnvironment(agent ? Object.keys(agent.environment).map(key => ({ key, value: null })) : []);
  };
  const cancel = () => { setEditor(null); setEnvironment([]); setError(null); };
  const save = async () => {
    if (!editor) return;
    if (!editor.name.trim() || !editor.executable.trim()) { setError("Enter an agent name and executable."); return; }
    const keys = environment.map(row => row.key);
    if (keys.some(key => !key || key.includes("=") || key.includes("\0")) || new Set(keys).size !== keys.length) { setError("Environment keys must be nonempty, unique, and contain no equals sign or NUL."); return; }
    setBusy("save"); setError(null);
    try {
      const result = await acpSaveAgent({ ...editor, environment: Object.fromEntries(environment.map(row => [row.key, row.value])) });
      const listed = await useCustomAcp.getState().loadAgents(true);
      const stored = listed.find(a => a.id === result.id);
      const publicIdentity = (a: AcpAgent) => JSON.stringify([a.id, a.revision, a.name, a.executable, a.args, a.enabled, a.auth_method, Object.keys(a.environment).sort()]);
      if (!stored || publicIdentity(stored) !== publicIdentity(result)) throw new Error("Could not verify the saved agent. Refresh and retry.");
      cancel();
    } catch (e) { setError(String(e)); }
    finally { setBusy(null); }
  };
  const remove = async (id: string) => {
    setBusy(id); setError(null);
    try {
      await acpDeleteAgent(id);
      const listed = await useCustomAcp.getState().loadAgents(true);
      if (listed.some(a => a.id === id)) throw new Error("Could not verify agent deletion. Refresh and retry.");
      setDeleting(null);
    } catch (e) { setError(String(e)); }
    finally { setBusy(null); }
  };
  const probe = async (id: string) => {
    setBusy(id); setProbeError(s => ({ ...s, [id]: "" }));
    try { await useCustomAcp.getState().probe(id); }
    catch (e) { setProbeError(s => ({ ...s, [id]: String(e) })); }
    finally { setBusy(null); }
  };
  return <section className="py-5" aria-labelledby="custom-acp-heading">
    <div className="mb-2 flex items-center justify-between gap-3">
      <h3 id="custom-acp-heading" className="text-body font-medium">Custom agents <span className="ml-1 text-label font-normal text-muted-foreground">Local ACP</span></h3>
      <Button variant="outline" size="sm" disabled={!!busy || editor !== null} onClick={() => edit()}><Plus className="size-3.5" />Add custom agent</Button>
    </div>
    <p className="mb-3 text-body-sm text-muted-foreground">Connect an installed ACP v1 harness. Runs directly, without a shell, with your OS account's access. Configuration stays on this machine; environment values are stored encrypted.</p>
    {warning && <p role="status" className="mb-3 text-body-sm text-muted-foreground">{warning}</p>}
    {error && <p role="alert" className="mb-3 text-body-sm text-destructive">{error} <button type="button" className="underline" onClick={() => { setError(null); void useCustomAcp.getState().loadAgents().catch(e => setError(String(e))); }}>Refresh</button></p>}
    {!agents && !error && <p className="text-body-sm text-muted-foreground">Loading custom agents…</p>}
    {agents?.length === 0 && !editor && <div className="rounded-lg border border-dashed border-border p-4 text-body-sm text-muted-foreground">No custom agents yet. Add an executable and its literal arguments, then probe when you're ready.</div>}
    {!!agents?.length && <div className="divide-y divide-border/50 overflow-hidden rounded-lg border border-border/60">
      {agents.map(agent => {
        const catalog = catalogs[acpCatalogKey(agent.id, agent.revision)];
        return <div key={agent.id} className="px-3 py-2.5">
          <div className="flex items-center gap-2">
            <Terminal className="size-4 shrink-0 text-muted-foreground" />
            <div className="min-w-0 flex-1"><div className="truncate text-body font-medium">{agent.name} {!agent.enabled && <span className="text-label font-normal text-muted-foreground">· Disabled</span>}</div><code className="block truncate text-label text-muted-foreground" title={agent.executable}>{agent.executable} · {agent.args.length} arguments</code></div>
            <Button size="sm" variant="ghost" aria-label={`Probe ${agent.name}`} disabled={!!busy || !agent.enabled} onClick={() => void probe(agent.id)}>{busy === agent.id ? <LoaderCircle className="size-3.5 animate-spin" /> : <FlaskConical className="size-3.5" />}Probe</Button>
            <Button size="icon" variant="ghost" aria-label={`Edit ${agent.name}`} disabled={!!busy} onClick={() => edit(agent)}><Pencil className="size-3.5" /></Button>
            <Button size="icon" variant="ghost" aria-label={`Delete ${agent.name}`} disabled={!!busy} onClick={() => setDeleting(agent.id)}><Trash2 className="size-3.5" /></Button>
          </div>
          {catalog && <p role="status" className="mt-2 text-label text-muted-foreground">{catalog.capabilities.models.length ? `${catalog.capabilities.models.length} models advertised` : "No models advertised · agent default remains available"} · {catalog.supports_resume ? "Resume supported" : "Resume unavailable"} · {catalog.supports_images ? "Images supported" : "Text only"}</p>}
          {probeError[agent.id] && <p role="alert" className="mt-2 text-body-sm text-destructive">{probeError[agent.id]}</p>}
          {deleting === agent.id && <div className="mt-2 rounded-md bg-muted/50 p-3 text-body-sm"><p>Delete {agent.name}? Active sessions will disconnect. Chat history remains, but this agent's chats cannot resume.</p><div className="mt-2 flex gap-2"><Button size="sm" variant="destructive" disabled={!!busy} onClick={() => void remove(agent.id)}>Confirm delete</Button><Button size="sm" variant="ghost" disabled={!!busy} onClick={() => setDeleting(null)}>Cancel</Button></div></div>}
        </div>;
      })}
    </div>}
    {editor && <form className="mt-3 space-y-3 rounded-lg border border-border bg-muted/20 p-3" onSubmit={e => { e.preventDefault(); void save(); }}>
      <fieldset disabled={!!busy} className="space-y-3">
        <div className="grid grid-cols-2 gap-3">
          <label className="text-label text-muted-foreground">Agent name<Input aria-label="Agent name" autoFocus value={editor.name} onChange={e => setEditor({ ...editor, name: e.target.value })} /></label>
          <label className="text-label text-muted-foreground">Executable<Input aria-label="Executable" className="font-mono" placeholder="agent-cli or /path/to/executable" value={editor.executable} onChange={e => setEditor({ ...editor, executable: e.target.value })} /></label>
        </div>
        <div><div className="mb-1 flex items-center justify-between"><span className="text-label font-medium">Arguments</span><Button type="button" variant="ghost" size="sm" onClick={() => setEditor({ ...editor, args: [...editor.args, ""] })}><Plus className="size-3" />Add argument</Button></div>
          <p className="mb-2 text-label text-muted-foreground">One literal argument per row. Spaces, quotes and $ are not interpreted.</p>
          {editor.args.map((arg, i) => <div key={i} className="mb-1 flex items-center gap-2"><span className="w-4 text-label tabular-nums text-muted-foreground">{i + 1}</span><Input aria-label={`Argument ${i + 1}`} className="font-mono" value={arg} onChange={e => setEditor({ ...editor, args: editor.args.map((v, j) => j === i ? e.target.value : v) })} /><Button type="button" size="icon" variant="ghost" aria-label={`Remove argument ${i + 1}`} onClick={() => setEditor({ ...editor, args: editor.args.filter((_, j) => j !== i) })}><Trash2 className="size-3.5" /></Button></div>)}
        </div>
        <div><div className="mb-1 flex items-center justify-between"><span className="text-label font-medium">Environment</span><Button type="button" variant="ghost" size="sm" onClick={() => setEnvironment([...environment, { key: "", value: "" }])}><Plus className="size-3" />Add environment variable</Button></div>
          <p className="mb-2 text-label text-muted-foreground">Saved values are never shown. Leave a saved row untouched to retain it; remove a row to delete it.</p>
          {environment.map((row, i) => <div key={i} className="mb-1 flex gap-2"><Input aria-label={`Environment key ${i + 1}`} className="w-1/3 font-mono" value={row.key} onChange={e => setEnvironment(environment.map((v, j) => j === i ? { ...v, key: e.target.value, value: v.key === e.target.value ? v.value : "" } : v))} /><Input aria-label={`Environment value ${i + 1}`} className="flex-1 font-mono" type="password" autoComplete="new-password" placeholder={row.value === null ? "Saved · retained" : "Value (may be empty)"} value={row.value ?? ""} onChange={e => setEnvironment(environment.map((v, j) => j === i ? { ...v, value: e.target.value } : v))} /><Button type="button" size="icon" variant="ghost" aria-label={`Remove environment variable ${i + 1}`} onClick={() => setEnvironment(environment.filter((_, j) => j !== i))}><Trash2 className="size-3.5" /></Button></div>)}
        </div>
        <div className="flex items-end gap-3"><label className="flex-1 text-label text-muted-foreground">Authentication method (optional)<Input aria-label="Authentication method" placeholder="Use existing agent login" list="custom-acp-auth-methods" value={editor.auth_method ?? ""} onChange={e => setEditor({ ...editor, auth_method: e.target.value === "" ? null : e.target.value })} /></label><label className="flex items-center gap-2 py-2 text-body-sm"><input type="checkbox" checked={editor.enabled} onChange={e => setEditor({ ...editor, enabled: e.target.checked })} />Enabled</label></div>
        <datalist id="custom-acp-auth-methods">{(() => { const agent = agents?.find(a => a.id === editor.id); return agent ? catalogs[acpCatalogKey(agent.id, agent.revision)]?.auth_methods.map(method => <option key={method.id} value={method.id}>{method.name}{method.description ? ` · ${method.description}` : ""}</option>) : null; })()}</datalist>
        <p className="text-label text-muted-foreground">Use an advertised authentication ID only if required. Sign in with the harness itself. Saving does not launch it; Probe starts a temporary session without a coding prompt.</p>
        {editor.id && <p className="text-label text-muted-foreground">Edits affect future starts. Existing sessions retain their launch configuration; bound chats will not silently switch instances.</p>}
        <div className="flex justify-end gap-2"><Button type="button" variant="ghost" size="sm" onClick={cancel}>Cancel</Button><Button type="submit" size="sm">{busy === "save" && <LoaderCircle className="size-3.5 animate-spin" />}Save agent</Button></div>
      </fieldset>
    </form>}
    <p className="mt-3 text-label text-muted-foreground">ACP approvals stay supervised. Agent modes are not a sandbox. Codemux-specific MCP injection, delegation and host-managed skills are unavailable for custom harnesses.</p>
  </section>;
}
