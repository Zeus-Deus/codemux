import { NativeSelect } from "@/components/ui/native-select";
import { Checkbox } from "@/components/ui/checkbox";
import { useState } from "react";
import { FlaskConical } from "lucide-react";
import { Spinner } from "@/components/ui/spinner";
import { useCustomAcpThread } from "@/hooks/use-custom-acp-thread";
import { acpCatalogKey, useCustomAcp } from "@/stores/custom-acp-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";

/** Models and runtime controls for one custom agent. The picker rail lists
 *  each agent separately; picking a model here binds the chat to `agentId`. */
export function CustomAcpModels({ agentId, threadId, projectPath, active, model, onSelect, disabled: controlsDisabled = false }: {
  agentId: string | null;
  threadId?: string | null;
  projectPath?: string | null;
  /** Whether the chat currently uses this agent. */
  active: boolean;
  model: string | null;
  onSelect: (model: string | null) => void;
  disabled?: boolean;
}) {
  const { catalog: threadCatalog, binding, error: threadError, warning, loading, busy } = useCustomAcpThread(threadId, true);
  const agent = useCustomAcp(s => s.agents?.find(a => a.id === agentId));
  const fixed = !!binding;
  const agentCatalog = useCustomAcp(s => agent ? s.catalogs[acpCatalogKey(agent.id, agent.revision)] ?? null : null);
  const catalog = fixed ? threadCatalog : agentCatalog;
  const [actionError, setActionError] = useState<string | null>(null);
  const [probing, setProbing] = useState(false);
  // Thread errors describe the agent the chat uses, not one being browsed.
  const error = fixed || active ? threadError : null;
  const streaming = useAgentChatStore(s => threadId ? s.threads[threadId]?.streaming ?? false : false);
  const disabled = controlsDisabled || streaming || loading || busy || probing || !!error || !threadId || !agent?.enabled;
  const pick = (value: string | null) => {
    if (threadId && agentId && !fixed) useCustomAcp.getState().select(threadId, agentId);
    onSelect(value);
  };
  const changeConfig = async (configId: string, value: string | boolean) => {
    if (!threadId || !fixed || disabled) return;
    setActionError(null);
    try {
      const updated = await useCustomAcp.getState().setConfig(threadId, configId, value);
      useAgentChatStore.getState().setModel(threadId, updated.current_model);
    } catch (e) { setActionError(String(e)); }
  };
  return <div className="flex min-h-0 flex-1 flex-col gap-2 overflow-y-auto p-3 text-body-sm" data-testid="custom-acp-models">
    <div className="flex items-center justify-between gap-2"><strong className="truncate">{agent?.name ?? binding?.catalog.agent_name ?? "Custom agent"}</strong><span className="shrink-0 text-label text-muted-foreground">{fixed ? "Fixed for this chat" : "Local ACP"}</span></div>
    {binding && !agent && <p className="text-label text-muted-foreground">This agent was removed from Settings → Providers.</p>}
    {warning && <p role="status" className="rounded-md bg-muted/50 p-2 text-label text-muted-foreground">{warning}</p>}
    {(error || actionError) && <p role="alert" className="rounded-md border border-destructive/30 p-2 text-destructive">{actionError ?? error}</p>}
    {agent && <button type="button" className="flex items-center justify-end gap-1.5 text-label text-muted-foreground hover:text-foreground disabled:opacity-40" disabled={loading || busy || probing || !agent.enabled} onClick={() => {
      setActionError(null); setProbing(true);
      void (fixed && threadId ? useCustomAcp.getState().refreshThread(threadId) : useCustomAcp.getState().probe(agent.id, projectPath)).catch(e => setActionError(String(e))).finally(() => setProbing(false));
    }}>{probing ? <Spinner className="size-3" aria-hidden /> : <FlaskConical className="size-3" />}{fixed ? "Refresh session controls" : "Probe saved agent"}</button>}
    {!fixed && agent && <button type="button" className="rounded-md p-2 text-left font-medium hover:bg-surface-2 disabled:opacity-40" disabled={disabled} aria-pressed={active && model === null} onClick={() => pick(null)}>Use agent default</button>}
    {catalog?.capabilities.models.map(m => <button type="button" key={m.id} className="rounded-md p-2 text-left hover:bg-surface-2 disabled:opacity-40" disabled={disabled} aria-pressed={active && model === m.id} title={m.description ?? m.id} onClick={() => pick(m.id)}><span className="block font-medium">{m.label}</span>{m.description && <span className="block text-label text-muted-foreground">{m.description}</span>}</button>)}
    {catalog?.capabilities.models.length === 0 && <p className="text-label text-muted-foreground">No models advertised. {fixed ? "The session uses the agent default." : "The agent default is available."}</p>}
    {!catalog && agent && <p className="text-label text-muted-foreground">Start with the agent default, or explicitly probe to discover models. Opening this picker does not launch a harness.</p>}
    {active && model !== null && catalog && !catalog.capabilities.models.some(m => m.id === model) && <p role="status" className="text-label text-muted-foreground">Selected model unavailable: {model}. Choose an advertised model or start a new chat with the agent default.</p>}
    {fixed && catalog?.config_options.filter(option => option.type === "select" || option.type === "boolean").map(option => option.type === "boolean" ? <label key={option.id} className="flex items-center gap-2 border-t border-border/40 pt-2" title={option.description ?? undefined}><Checkbox aria-label={option.name} checked={option.current_value === true} disabled={disabled} onCheckedChange={(checked) => void changeConfig(option.id, (checked === true))} />{option.name}</label> : <label key={option.id} className="border-t border-border/40 pt-2 text-label text-muted-foreground" title={option.description ?? undefined}>{option.name}<NativeSelect aria-label={option.name} value={String(option.current_value)} disabled={disabled} className="mt-1" onChange={e => void changeConfig(option.id, e.target.value)}>{option.options.map((value, i) => <option key={i} value={value.value}>{value.name}</option>)}</NativeSelect></label>)}
    {catalog && <p className="text-label text-muted-foreground">{catalog.supports_resume ? "Resume supported" : "Resume not advertised · restarting this chat may be unavailable"} · {catalog.supports_images ? "Images supported" : "Text only"}</p>}
    <p className="border-t border-border/40 pt-2 text-label text-muted-foreground">Supervised approvals · modes do not grant automatic access. Skills and delegation unavailable. {!fixed && "Additional runtime controls appear after the session starts."}</p>
  </div>;
}
