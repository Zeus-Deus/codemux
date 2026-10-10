import { useState } from "react";
import { FlaskConical, LoaderCircle } from "lucide-react";
import { useCustomAcpThread } from "@/hooks/use-custom-acp-thread";
import { useCustomAcp } from "@/stores/custom-acp-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";

export function CustomAcpModels({ threadId, projectPath, model, onSelect, disabled: controlsDisabled = false }: {
  threadId?: string | null;
  projectPath?: string | null;
  model: string | null;
  onSelect: (model: string | null) => void;
  disabled?: boolean;
}) {
  const { catalog, binding, agent, error, warning, loading, busy } = useCustomAcpThread(threadId, true);
  const agents = useCustomAcp(s => s.agents);
  const [actionError, setActionError] = useState<string | null>(null);
  const [probing, setProbing] = useState(false);
  const fixed = !!binding;
  const streaming = useAgentChatStore(s => threadId ? s.threads[threadId]?.streaming ?? false : false);
  const disabled = controlsDisabled || streaming || loading || busy || probing || !!error || !agent?.enabled;
  const changeConfig = async (configId: string, value: string | boolean) => {
    if (!threadId || !fixed || disabled) return;
    setActionError(null);
    try {
      const updated = await useCustomAcp.getState().setConfig(threadId, configId, value);
      useAgentChatStore.getState().setModel(threadId, updated.current_model);
    } catch (e) { setActionError(String(e)); }
  };
  return <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto p-3 text-body-sm" data-testid="custom-acp-models">
    <div className="flex items-center justify-between"><strong>Custom agents</strong><span className="text-label text-muted-foreground">Local ACP</span></div>
    <label className="text-label text-muted-foreground">Custom agent {fixed && "· fixed for this chat"}
      <select aria-label="Custom agent" disabled={controlsDisabled || streaming || loading || fixed || !threadId || busy} value={agent?.id ?? binding?.agent_id ?? ""} className="mt-1 w-full rounded-md border border-border bg-background p-2 text-body-sm text-foreground" onChange={e => {
        if (!threadId || !e.target.value) return;
        useCustomAcp.getState().select(threadId, e.target.value);
        setActionError(null);
        onSelect(null);
      }}>
        <option value="">{loading ? "Loading saved agents…" : "Choose an agent"}</option>
        {binding && !agents?.some(a => a.id === binding.agent_id) && <option value={binding.agent_id}>{binding.catalog.agent_name} · unavailable</option>}
        {agents?.map(a => <option key={a.id} value={a.id} disabled={!a.enabled}>{a.name}{!a.enabled ? " · Disabled" : ""}</option>)}
      </select>
    </label>
    {warning && <p role="status" className="rounded-md bg-muted/50 p-2 text-label text-muted-foreground">{warning}</p>}
    {(error || actionError) && <p role="alert" className="rounded-md border border-destructive/30 p-2 text-destructive">{actionError ?? error}</p>}
    {!agents?.length && !loading && <p className="text-muted-foreground">Add a custom agent in Settings → Agent.</p>}
    {agent && <button type="button" className="flex items-center justify-end gap-1.5 text-label text-muted-foreground hover:text-foreground disabled:opacity-40" disabled={loading || busy || probing || !agent.enabled} onClick={() => {
      setActionError(null); setProbing(true);
      void (fixed && threadId ? useCustomAcp.getState().refreshThread(threadId) : useCustomAcp.getState().probe(agent.id, projectPath)).catch(e => setActionError(String(e))).finally(() => setProbing(false));
    }}>{probing ? <LoaderCircle className="size-3 animate-spin" /> : <FlaskConical className="size-3" />}{fixed ? "Refresh session controls" : "Probe saved agent"}</button>}
    {!fixed && agent && <button type="button" className="rounded-md p-2 text-left font-medium hover:bg-muted disabled:opacity-40" disabled={disabled} aria-pressed={model === null} onClick={() => onSelect(null)}>Use agent default</button>}
    {catalog?.capabilities.models.map(m => <button type="button" key={m.id} className="rounded-md p-2 text-left hover:bg-muted disabled:opacity-40" disabled={disabled} aria-pressed={model === m.id} title={m.description ?? m.id} onClick={() => onSelect(m.id)}><span className="block font-medium">{m.label}</span>{m.description && <span className="block text-label text-muted-foreground">{m.description}</span>}</button>)}
    {catalog?.capabilities.models.length === 0 && <p className="text-label text-muted-foreground">No models advertised. {fixed ? "The session uses the agent default." : "The agent default is available."}</p>}
    {!catalog && agent && <p className="text-label text-muted-foreground">Start with the agent default, or explicitly probe to discover models. Opening this picker does not launch a harness.</p>}
    {model !== null && catalog && !catalog.capabilities.models.some(m => m.id === model) && <p role="status" className="text-label text-muted-foreground">Selected model unavailable: {model}. Choose an advertised model or start a new chat with the agent default.</p>}
    {fixed && catalog?.config_options.filter(option => option.type === "select" || option.type === "boolean").map(option => option.type === "boolean" ? <label key={option.id} className="flex items-center gap-2 border-t border-border/40 pt-2" title={option.description ?? undefined}><input aria-label={option.name} type="checkbox" checked={option.current_value === true} disabled={disabled} onChange={e => void changeConfig(option.id, e.target.checked)} />{option.name}</label> : <label key={option.id} className="border-t border-border/40 pt-2 text-label text-muted-foreground" title={option.description ?? undefined}>{option.name}<select aria-label={option.name} value={String(option.current_value)} disabled={disabled} className="mt-1 w-full rounded-md border border-border bg-background p-2 text-body-sm text-foreground" onChange={e => void changeConfig(option.id, e.target.value)}>{option.options.map((value, i) => <option key={i} value={value.value}>{value.name}</option>)}</select></label>)}
    {catalog && <p className="text-label text-muted-foreground">{catalog.supports_resume ? "Resume supported" : "Resume not advertised · restarting this chat may be unavailable"} · {catalog.supports_images ? "Images supported" : "Text only"}</p>}
    <p className="border-t border-border/40 pt-2 text-label text-muted-foreground">Supervised approvals · modes do not grant automatic access. Host MCP, skills and delegation unavailable. {!fixed && "Additional runtime controls appear after the session starts."}</p>
  </div>;
}
