import { useEffect, useState } from "react";
import { Plus, Pencil, Trash2, FlaskConical } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { acpDeleteAgent, acpSaveAgent, type AcpAgent, type AcpAgentInput } from "@/tauri/custom-acp";
import { acpCatalogKey, useCustomAcp } from "@/stores/custom-acp-store";
import { DetailHeader, DetailRow, DetailSection, type ProviderStatus } from "./provider-parts";

const emptyAgent = (): AcpAgentInput => ({ name: "", executable: "", args: [], environment: {}, enabled: true, auth_method: null });
type EnvRow = { key: string; value: string | null };

/** Settings → Providers → a custom ACP agent, or the form for a new one
 *  (`agentId` null). Secrets exist only in this unsaved editor, never in
 *  renderer persistence. */
export function CustomAcpDetail({
  agentId,
  onSaved,
  onCancelNew,
  onLockChange,
  switchLocked = false,
}: {
  agentId: string | null;
  onSaved: (id: string) => void;
  onCancelNew: () => void;
  /** Reports while this agent is being edited or mutated, so the page can
   *  keep its list switch from saving over the editor. */
  onLockChange?: (locked: boolean) => void;
  /** Another save of an agent (from the list) is still in flight. */
  switchLocked?: boolean;
}) {
  const agents = useCustomAcp(s => s.agents);
  const catalogs = useCustomAcp(s => s.catalogs);
  const agent = agentId ? agents?.find(a => a.id === agentId) ?? null : null;
  const [editor, setEditor] = useState<AcpAgentInput | null>(agentId ? null : emptyAgent());
  const [environment, setEnvironment] = useState<EnvRow[]>([]);
  const [deleting, setDeleting] = useState(false);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [probeError, setProbeError] = useState<string | null>(null);
  useEffect(() => { let alive = true; void useCustomAcp.getState().loadAgents().catch(e => { if (alive) setError(String(e)); }); return () => { alive = false; }; }, []);
  const locked = editor !== null || busy !== null;
  useEffect(() => {
    onLockChange?.(locked);
  }, [locked, onLockChange]);
  useEffect(() => () => onLockChange?.(false), [onLockChange]);
  const edit = (a: AcpAgent) => {
    setError(null);
    setEditor({ id: a.id, name: a.name, executable: a.executable, args: [...a.args], environment: {}, enabled: a.enabled, auth_method: a.auth_method });
    setEnvironment(Object.keys(a.environment).map(key => ({ key, value: null })));
  };
  const cancel = () => {
    setEnvironment([]); setError(null);
    if (agentId) setEditor(null); else onCancelNew();
  };
  const save = async (input: AcpAgentInput, env: EnvRow[]) => {
    if (!input.name.trim() || !input.executable.trim()) { setError("Enter an agent name and executable."); return; }
    const keys = env.map(row => row.key);
    if (keys.some(key => !key || key.includes("=") || key.includes("\0")) || new Set(keys).size !== keys.length) { setError("Environment keys must be nonempty, unique, and contain no equals sign or NUL."); return; }
    setBusy("save"); setError(null);
    try {
      const result = await acpSaveAgent({ ...input, environment: Object.fromEntries(env.map(row => [row.key, row.value])) });
      const listed = await useCustomAcp.getState().loadAgents(true);
      const stored = listed.find(a => a.id === result.id);
      const publicIdentity = (a: AcpAgent) => JSON.stringify([a.id, a.revision, a.name, a.executable, a.args, a.enabled, a.auth_method, Object.keys(a.environment).sort()]);
      if (!stored || publicIdentity(stored) !== publicIdentity(result)) throw new Error("Could not verify the saved agent. Refresh and retry.");
      setEditor(null); setEnvironment([]);
      onSaved(result.id);
    } catch (e) { setError(String(e)); }
    finally { setBusy(null); }
  };
  // The switch saves only the enabled flag; saved environment values stay
  // redacted and are retained by sending null. It is disabled while the
  // editor is open or anything is saving, so it never overwrites or clears
  // unsaved edits.
  const setEnabled = async (a: AcpAgent, enabled: boolean) => {
    if (editor || busy) return;
    setBusy("toggle"); setError(null);
    try {
      await acpSaveAgent({ id: a.id, name: a.name, executable: a.executable, args: a.args, environment: Object.fromEntries(Object.keys(a.environment).map(key => [key, null])), enabled, auth_method: a.auth_method });
      await useCustomAcp.getState().loadAgents(true);
    } catch (e) { setError(String(e)); }
    finally { setBusy(null); }
  };
  const remove = async (id: string) => {
    setBusy(id); setError(null);
    try {
      await acpDeleteAgent(id);
      const listed = await useCustomAcp.getState().loadAgents(true);
      if (listed.some(a => a.id === id)) throw new Error("Could not verify agent deletion. Refresh and retry.");
      setDeleting(false);
    } catch (e) { setError(String(e)); }
    finally { setBusy(null); }
  };
  const probe = async (id: string) => {
    setBusy(id); setProbeError(null);
    try { await useCustomAcp.getState().probe(id); }
    catch (e) { setProbeError(String(e)); }
    finally { setBusy(null); }
  };

  const catalog = agent ? catalogs[acpCatalogKey(agent.id, agent.revision)] : undefined;
  const status: ProviderStatus = !agent
    ? { tone: "pending", label: "Not saved yet" }
    : !agent.enabled
      ? { tone: "off", label: "Off" }
      : catalog ? { tone: "ok", label: "Ready" } : { tone: "pending", label: "Not tested yet" };

  if (agentId && !agent) {
    return <div className="p-5 text-body-sm text-muted-foreground">{agents ? "This custom agent was removed." : "Loading custom agents…"}</div>;
  }

  return <section aria-label={agent ? agent.name : "New custom agent"}>
    <DetailHeader
      logo={<ProviderLogo provider="acp" className="size-5" />}
      name={agent?.name ?? "New custom agent"}
      tag="Custom ACP"
      status={status}
      enabled={agent?.enabled}
      onEnabledChange={agent ? (on => void setEnabled(agent, on)) : undefined}
      switchDisabled={locked || switchLocked}
      switchTitle={editor ? "Save or cancel your edits first." : undefined}
      actions={agent && !editor && <Button variant="ghost" size="sm" className="text-muted-foreground" aria-label={`Probe ${agent.name}`} disabled={!!busy || !agent.enabled} onClick={() => void probe(agent.id)}>
        {busy === agent.id ? <Spinner className="size-3.5" aria-hidden /> : <FlaskConical />}Test
      </Button>}
    />
    {error && <div className="px-5 pb-3"><p role="alert" className="text-body-sm text-destructive">{error} <button type="button" className="underline" onClick={() => { setError(null); void useCustomAcp.getState().loadAgents(true).catch(e => setError(String(e))); }}>Refresh</button></p></div>}
    {editor ? <AgentEditor
      editor={editor}
      setEditor={setEditor}
      environment={environment}
      setEnvironment={setEnvironment}
      busy={busy}
      existing={!!editor.id}
      authMethods={catalog?.auth_methods ?? []}
      onCancel={cancel}
      onSave={() => void save(editor, environment)}
    /> : agent && <>
      <DetailSection
        title="Launch"
        action={<Button variant="ghost" size="xs" aria-label={`Edit ${agent.name}`} disabled={!!busy} onClick={() => edit(agent)}><Pencil />Edit</Button>}
      >
        <DetailRow>
          <code className="min-w-0 flex-1 truncate font-mono text-label text-foreground" title={[agent.executable, ...agent.args].join(" ")}>
            {agent.executable}{agent.args.length ? ` ${agent.args.join(" ")}` : ""}
          </code>
        </DetailRow>
        <p className="mt-2 text-label text-muted-foreground">
          {agent.args.length} {agent.args.length === 1 ? "argument" : "arguments"} · {Object.keys(agent.environment).length} environment {Object.keys(agent.environment).length === 1 ? "variable" : "variables"}{agent.auth_method ? ` · sign-in ${agent.auth_method}` : ""}
        </p>
      </DetailSection>
      <DetailSection title="Test">
        {catalog
          ? <p role="status" className="text-body-sm text-muted-foreground">{catalog.capabilities.models.length ? `${catalog.capabilities.models.length} models advertised` : "No models advertised · agent default remains available"} · {catalog.supports_resume ? "Resume supported" : "Resume unavailable"} · {catalog.supports_images ? "Images supported" : "Text only"}</p>
          : <p className="text-body-sm text-muted-foreground">Test starts a temporary session without a coding prompt and reads what the agent supports.</p>}
        {probeError && <p role="alert" className="mt-2 text-body-sm text-destructive">{probeError}</p>}
      </DetailSection>
      <DetailSection title="Remove">
        {deleting
          ? <div className="space-y-2">
            <p className="text-body-sm">Delete {agent.name}? Active sessions will disconnect. Chat history remains, but this agent's chats cannot resume.</p>
            <div className="flex gap-2"><Button size="sm" variant="destructive" disabled={!!busy} onClick={() => void remove(agent.id)}>Confirm delete</Button><Button size="sm" variant="ghost" disabled={!!busy} onClick={() => setDeleting(false)}>Cancel</Button></div>
          </div>
          : <Button variant="ghost" size="sm" className="text-destructive" aria-label={`Delete ${agent.name}`} disabled={!!busy} onClick={() => setDeleting(true)}><Trash2 />Remove custom agent</Button>}
      </DetailSection>
    </>}
    <p className="border-t border-hairline px-5 py-3 text-label text-muted-foreground">ACP approvals stay supervised. Agent modes are not a sandbox. Codemux-specific MCP injection, delegation and host-managed skills are unavailable for custom harnesses.</p>
  </section>;
}

function AgentEditor({
  editor, setEditor, environment, setEnvironment, busy, existing, authMethods, onCancel, onSave,
}: {
  editor: AcpAgentInput;
  setEditor: (value: AcpAgentInput) => void;
  environment: EnvRow[];
  setEnvironment: (rows: EnvRow[]) => void;
  busy: string | null;
  existing: boolean;
  authMethods: { id: string; name: string; description: string | null }[];
  onCancel: () => void;
  onSave: () => void;
}) {
  return <form className="border-t border-hairline px-5 py-4" onSubmit={e => { e.preventDefault(); onSave(); }}>
    <fieldset disabled={!!busy} className="space-y-4">
      {!existing && <p className="text-body-sm text-muted-foreground">Connect an installed ACP v1 harness. It runs directly, without a shell, with your OS account's access. Configuration stays on this machine; environment values are stored encrypted.</p>}
      <div className="grid grid-cols-2 gap-3">
        <label className="text-label text-muted-foreground">Agent name<Input aria-label="Agent name" className="mt-1" autoFocus value={editor.name} onChange={e => setEditor({ ...editor, name: e.target.value })} /></label>
        <label className="text-label text-muted-foreground">Executable<Input aria-label="Executable" className="mt-1 font-mono" placeholder="agent-cli or /path/to/executable" value={editor.executable} onChange={e => setEditor({ ...editor, executable: e.target.value })} /></label>
      </div>
      <div><div className="mb-1 flex items-center justify-between"><span className="text-label font-medium">Arguments</span><Button type="button" variant="ghost" size="xs" onClick={() => setEditor({ ...editor, args: [...editor.args, ""] })}><Plus />Add argument</Button></div>
        <p className="mb-2 text-label text-muted-foreground">One literal argument per row. Spaces, quotes and $ are not interpreted.</p>
        {editor.args.map((arg, i) => <div key={i} className="mb-1 flex items-center gap-2"><span className="w-4 text-label tabular-nums text-muted-foreground">{i + 1}</span><Input aria-label={`Argument ${i + 1}`} className="font-mono" value={arg} onChange={e => setEditor({ ...editor, args: editor.args.map((v, j) => j === i ? e.target.value : v) })} /><Button type="button" size="icon-sm" variant="ghost" aria-label={`Remove argument ${i + 1}`} onClick={() => setEditor({ ...editor, args: editor.args.filter((_, j) => j !== i) })}><Trash2 /></Button></div>)}
      </div>
      <div><div className="mb-1 flex items-center justify-between"><span className="text-label font-medium">Environment</span><Button type="button" variant="ghost" size="xs" onClick={() => setEnvironment([...environment, { key: "", value: "" }])}><Plus />Add environment variable</Button></div>
        <p className="mb-2 text-label text-muted-foreground">Saved values are never shown. Leave a saved row untouched to retain it; remove a row to delete it.</p>
        {environment.map((row, i) => <div key={i} className="mb-1 flex gap-2"><Input aria-label={`Environment key ${i + 1}`} className="w-1/3 font-mono" value={row.key} onChange={e => setEnvironment(environment.map((v, j) => j === i ? { ...v, key: e.target.value, value: v.key === e.target.value ? v.value : "" } : v))} /><Input aria-label={`Environment value ${i + 1}`} className="flex-1 font-mono" type="password" autoComplete="new-password" placeholder={row.value === null ? "Saved · retained" : "Value (may be empty)"} value={row.value ?? ""} onChange={e => setEnvironment(environment.map((v, j) => j === i ? { ...v, value: e.target.value } : v))} /><Button type="button" size="icon-sm" variant="ghost" aria-label={`Remove environment variable ${i + 1}`} onClick={() => setEnvironment(environment.filter((_, j) => j !== i))}><Trash2 /></Button></div>)}
      </div>
      <label className="block text-label text-muted-foreground">Authentication method (optional)<Input aria-label="Authentication method" className="mt-1" placeholder="Use existing agent login" list="custom-acp-auth-methods" value={editor.auth_method ?? ""} onChange={e => setEditor({ ...editor, auth_method: e.target.value === "" ? null : e.target.value })} /></label>
      <datalist id="custom-acp-auth-methods">{authMethods.map(method => <option key={method.id} value={method.id}>{method.name}{method.description ? ` · ${method.description}` : ""}</option>)}</datalist>
      <p className="text-label text-muted-foreground">Use an advertised authentication ID only if required. Sign in with the harness itself. Saving does not launch it; Test starts a temporary session without a coding prompt.</p>
      {existing && <p className="text-label text-muted-foreground">Edits affect future starts. Existing sessions retain their launch configuration; bound chats will not silently switch instances.</p>}
      <div className="flex justify-end gap-2"><Button type="button" variant="ghost" size="sm" onClick={onCancel}>Cancel</Button><Button type="submit" size="sm">{busy === "save" && <Spinner className="size-3.5" aria-hidden />}Save agent</Button></div>
    </fieldset>
  </form>;
}
