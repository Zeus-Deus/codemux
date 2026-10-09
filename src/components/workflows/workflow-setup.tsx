import { Code2, Play, Plus, RotateCw, Save, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import type { WorkflowDraft } from "@/stores/workflow-ui-store";
import type { AgentChatProviderKind } from "@/tauri/types";
import type { WorkflowCapability, WorkflowScript } from "@/tauri/workflows";
import { WORKFLOW_STARTER } from "./workflow-starter";

const PROVIDERS: AgentChatProviderKind[] = ["claude", "codex", "hermes", "opencode", "cursor", "grok"];
const LABELS: Record<AgentChatProviderKind, string> = { claude: "Claude", codex: "Codex", hermes: "Hermes", opencode: "OpenCode", cursor: "Cursor", grok: "Grok" };
const SELECT = "h-8 min-w-0 w-full rounded-md border border-border bg-background px-2 text-body-sm text-foreground focus-visible:outline-2 focus-visible:outline-ring";
const SUMMARY = "cursor-pointer py-2 text-body-sm font-medium focus-visible:rounded-md focus-visible:outline-2 focus-visible:outline-ring";
const validOpenCodeModel = (model: string) => /^[A-Za-z0-9._-]+\/\S+$/.test(model);

export interface WorkflowSetupProps {
  draft: WorkflowDraft;
  capabilities: WorkflowCapability[];
  scripts: WorkflowScript[];
  pending: boolean;
  saving: boolean;
  saved: boolean;
  onChange: (patch: Partial<WorkflowDraft>) => void;
  onLaunch: (mode: "dry_run" | "live") => void;
  onSave: () => void;
}

export function WorkflowSetup({ draft, capabilities, scripts, pending, saving, saved, onChange, onLaunch, onSave }: WorkflowSetupProps) {
  const unavailable = [...new Set(draft.routes.filter((route) => !capabilities.find((item) => item.provider === route.provider)?.live).map((route) => LABELS[route.provider]))];
  const requiredModels = [...new Set(draft.routes.filter((route) => capabilities.find((item) => item.provider === route.provider)?.requires_model).map((route) => LABELS[route.provider]))];
  const invalidModels = draft.routes.filter((route) => capabilities.find((item) => item.provider === route.provider)?.requires_model && (!route.model?.trim() || (route.provider === "opencode" && !validOpenCodeModel(route.model.trim()))));
  const unsupportedEfforts = draft.routes.filter((route) => capabilities.find((item) => item.provider === route.provider)?.effort_supported === false && !!route.effort?.trim());
  const routeName = (route: WorkflowDraft["routes"][number]) => route.id === route.provider ? LABELS[route.provider] : `${LABELS[route.provider]} (${route.id})`;
  const configurationHints = [invalidModels.length > 0 ? `Set a valid model for ${invalidModels.map(routeName).join(", ")} in Models and routes.` : "", unsupportedEfforts.length > 0 ? `Clear effort for ${unsupportedEfforts.map(routeName).join(", ")} in Models and routes.` : ""].filter(Boolean);
  const canStart = !!draft.title.trim() && !!draft.goal.trim() && !!draft.source.trim() && draft.routes.length > 0 && !pending;
  const canLive = canStart && unavailable.length === 0 && configurationHints.length === 0;

  return <div className="min-w-0" data-testid="workflow-setup">
    <div className="space-y-4 px-3.5 pb-4 pt-3.5">
      <div><h2 className="text-body-lg font-semibold">New workflow</h2><p className="mt-1 text-body-sm leading-relaxed text-muted-foreground">Give it a goal, choose who can work on it, then review the results.</p></div>
      <label className="block space-y-1.5"><Eyebrow tone="strong">Run name</Eyebrow><Input aria-label="Workflow name" data-testid="workflow-title" value={draft.title} onChange={(event) => onChange({ title: event.target.value })} maxLength={160} /></label>
      <label className="block space-y-1.5"><Eyebrow tone="strong">Goal</Eyebrow><Textarea aria-label="Workflow goal" data-testid="workflow-goal" value={draft.goal} onChange={(event) => onChange({ goal: event.target.value })} rows={3} className="min-h-20 resize-y" maxLength={32000} /></label>

      <section aria-label="Workflow providers" className="space-y-1">
        <div className="mb-1 flex items-center justify-between gap-2"><Eyebrow tone="strong">Providers</Eyebrow><span className="font-mono text-caption text-muted-foreground">{draft.routes.length} {draft.routes.length === 1 ? "route" : "routes"}</span></div>
        {PROVIDERS.map((provider) => {
          const selected = draft.routes.some((route) => route.provider === provider);
          const capability = capabilities.find((item) => item.provider === provider);
          return <label key={provider} className="flex min-w-0 cursor-pointer items-center gap-2 rounded-md py-1.5 text-body-sm">
            <input type="checkbox" aria-label={`Use ${LABELS[provider]}`} data-testid={`workflow-route-${provider}`} className="shrink-0 accent-[var(--primary)]" checked={selected} onChange={(event) => onChange({ routes: event.target.checked ? [...draft.routes, { id: provider, provider }] : draft.routes.filter((route) => route.provider !== provider) })} />
            <span className={cn(selected ? "text-foreground" : "text-muted-foreground")}>{LABELS[provider]}</span>
            <span className={cn("ml-auto shrink-0 text-label", selected && capability?.live ? "text-status-open" : "text-muted-foreground")}>{capability ? capability.live ? "Live" : "Dry run only" : "Checking…"}</span>
          </label>;
        })}
        <details data-testid="workflow-route-settings" className="border-t border-border/50">
          <summary className={SUMMARY}>Models and routes<span className="ml-2 text-label font-normal text-muted-foreground">{requiredModels.length > 0 ? `${requiredModels.join(", ")} model required` : "Optional"}</span></summary>
          <RouteSettings draft={draft} capabilities={capabilities} onChange={onChange} />
        </details>
      </section>

      <LimitSettings draft={draft} onChange={onChange} />
      <label className="flex cursor-pointer items-start gap-2 text-body-sm"><input type="checkbox" className="mt-1 shrink-0 accent-[var(--primary)]" data-testid="workflow-allow-writes" checked={draft.allowWrites ?? false} onChange={(event) => onChange({ allowWrites: event.target.checked })} /><span>Allow file changes<span className="mt-1 block text-label leading-relaxed text-muted-foreground">Custom write tasks keep changes isolated until you review and apply them.</span></span></label>

      <details data-testid="workflow-script-settings" className="border-t border-border/50">
        <summary className={SUMMARY}>Workflow script<span className="ml-2 text-label font-normal text-muted-foreground">{draft.source === WORKFLOW_STARTER ? "Read-only starter" : "Custom"}</span></summary>
        <div className="space-y-3 pb-2 pt-1">
          <p className="text-label leading-relaxed text-muted-foreground">The starter reviews three areas in parallel, then asks for a summary. Edit the script to choose tasks, dependencies and routes.</p>
          <label className="block space-y-1.5"><Eyebrow>Saved script</Eyebrow><select aria-label="Saved workflow script" data-testid="workflow-saved-script" className={SELECT} value="" onChange={(event) => { const script = scripts.find((item) => item.id === event.target.value); if (script) onChange({ source: script.source, title: script.title }); }}><option value="">Load a saved script…</option>{scripts.map((script) => <option key={script.id} value={script.id}>{script.title}</option>)}</select></label>
          <label className="block space-y-1.5"><Eyebrow>JavaScript</Eyebrow><Textarea aria-label="Workflow JavaScript" data-testid="workflow-script" value={draft.source} onChange={(event) => onChange({ source: event.target.value })} spellCheck={false} rows={10} className="min-h-56 resize-y whitespace-pre font-mono text-label leading-relaxed" maxLength={128000} /></label>
          <div className="flex flex-wrap items-center gap-2"><Button variant="outline" size="xs" data-testid="workflow-save-script" disabled={!draft.title.trim() || !draft.source.trim() || saving} onClick={onSave}><Save />{saving ? "Saving…" : "Save script"}</Button><Button variant="ghost" size="xs" onClick={() => onChange({ source: WORKFLOW_STARTER })}><RotateCw />Use starter</Button>{saved && <span role="status" className="text-label text-status-open">Saved</span>}</div>
          <details className="text-body-sm"><summary className={SUMMARY}>Script reference</summary><p className="mt-1 text-label leading-relaxed text-muted-foreground">Use <code>agent(prompt, options)</code> for a task and its result, <code>parallel(items, fn)</code> to fan out, and <code>workflow.addTask(spec)</code> / <code>workflow.wait(id)</code> for dependencies. Read <code>goal</code>, <code>routes</code> and <code>args</code>. Await workflow operations before returning.</p><p className="mt-2 text-label leading-relaxed text-muted-foreground">Use <code>workflow.replace</code>, <code>workflow.cancelTask</code> and <code>workflow.message</code> to adapt the work. Scripts have no imports, network, files or timers. The starter is read-only; custom write tasks also require Allow file changes.</p></details>
        </div>
      </details>
    </div>
    <div className="sticky bottom-0 space-y-2 border-t border-border/60 bg-background px-3.5 py-3">
      <div className="flex flex-wrap items-center gap-2"><Button size="sm" data-testid="workflow-live-run" disabled={!canLive} onClick={() => onLaunch("live")}><Play />{pending ? "Starting…" : "Run with agents"}</Button><Button size="sm" variant="outline" data-testid="workflow-dry-run" disabled={!canStart} onClick={() => onLaunch("dry_run")}><Code2 />Dry run</Button></div>
      <p className="break-words text-label leading-relaxed text-muted-foreground">Dry run uses no models.{draft.routes.length === 0 ? " Choose at least one provider." : unavailable.length > 0 ? ` Live execution is unavailable for ${unavailable.join(", ")}. Choose live providers or use Dry run.` : configurationHints.length > 0 ? ` ${configurationHints.join(" ")}` : " Run with agents uses the selected providers."}</p>
    </div>
  </div>;
}

function RouteSettings({ draft, capabilities, onChange }: { draft: WorkflowDraft; capabilities: WorkflowCapability[]; onChange: (patch: Partial<WorkflowDraft>) => void }) {
  const update = (index: number, patch: Partial<WorkflowDraft["routes"][number]>) => onChange({ routes: draft.routes.map((route, i) => i === index ? { ...route, ...patch } : route) });
  return <div className="space-y-3 pb-2 pt-1">
    {draft.routes.map((route, index) => {
      const standard = route.id === route.provider;
      const capability = capabilities.find((item) => item.provider === route.provider);
      const invalidModel = !!capability?.requires_model && route.provider === "opencode" && !!route.model?.trim() && !validOpenCodeModel(route.model.trim());
      const modelHint = invalidModel ? "Use provider/model with no spaces in the provider or model." : capability?.requires_model ? route.provider === "opencode" ? "Model required for live execution. Use provider/model." : "Model required for live execution." : null;
      const effortHint = capability?.effort_supported === false ? `${LABELS[route.provider]} uses its default effort. Leave effort empty.` : null;
      return <div key={index} className="space-y-2 border-b border-border/40 pb-3">
        <div className="flex min-w-0 items-center gap-2"><span className="min-w-0 flex-1 truncate text-body-sm font-medium">{LABELS[route.provider]}{!standard && <span className="ml-1.5 font-mono text-label font-normal text-muted-foreground">{route.id}</span>}</span>{!standard && <Button variant="ghost" size="icon-xs" aria-label={`Remove route ${route.id}`} onClick={() => onChange({ routes: draft.routes.filter((_, i) => i !== index) })}><X /></Button>}</div>
        {!standard && <div className="space-y-2"><Input aria-label={`Route ${index + 1} ID`} value={route.id} placeholder="Route ID" className="h-8 font-mono text-label" onChange={(event) => update(index, { id: event.target.value })} /><select aria-label={`Provider for ${route.id}`} className={SELECT} value={route.provider} onChange={(event) => update(index, { provider: event.target.value as AgentChatProviderKind, model: null, effort: null })}>{PROVIDERS.map((provider) => <option key={provider} value={provider}>{LABELS[provider]}</option>)}</select></div>}
        <div className="grid min-w-0 grid-cols-2 gap-2"><Input aria-label={standard ? `${LABELS[route.provider]} model` : `Model for ${route.id}`} aria-invalid={invalidModel || undefined} aria-describedby={modelHint ? `workflow-model-hint-${index}` : undefined} data-testid={`workflow-model-${route.id}`} placeholder={capability?.requires_model ? route.provider === "opencode" ? "provider/model" : "Required model" : "Default model"} value={route.model ?? ""} className="h-8 min-w-0 text-label" onChange={(event) => update(index, { model: event.target.value || null })} /><Input aria-label={standard ? `${LABELS[route.provider]} effort` : `Effort for ${route.id}`} aria-describedby={effortHint ? `workflow-effort-hint-${index}` : undefined} placeholder={effortHint ? "Leave empty" : "Default effort"} value={route.effort ?? ""} className="h-8 min-w-0 text-label" onChange={(event) => update(index, { effort: event.target.value || null })} /></div>
        {modelHint && <p id={`workflow-model-hint-${index}`} className="break-words text-label leading-relaxed text-muted-foreground">{modelHint}</p>}
        {effortHint && <p id={`workflow-effort-hint-${index}`} className="break-words text-label leading-relaxed text-muted-foreground">{effortHint}</p>}
        {capability?.reason && <p className="break-words text-label leading-relaxed text-muted-foreground">{capability.reason}</p>}
      </div>;
    })}
    <Button variant="outline" size="xs" data-testid="workflow-add-route" onClick={() => { let count = 1; while (draft.routes.some((route) => route.id === `route-${count}`)) count += 1; onChange({ routes: [...draft.routes, { id: `route-${count}`, provider: "claude" }] }); }}><Plus />Add route</Button>
    <p className="text-label leading-relaxed text-muted-foreground">Use more than one model from a provider by adding routes. Scripts select route IDs; an omitted ID uses the first route.</p>
  </div>;
}

function LimitSettings({ draft, onChange }: { draft: WorkflowDraft; onChange: (patch: Partial<WorkflowDraft>) => void }) {
  const update = (key: keyof WorkflowDraft["limits"], value: number | null) => onChange({ limits: { ...draft.limits, [key]: value } });
  return <details data-testid="workflow-limit-settings" className="border-t border-border/50">
    <summary className={SUMMARY}>Limits<span className="ml-2 text-label font-normal text-muted-foreground">{draft.limits.concurrency === 0 ? "Auto workers" : `${draft.limits.concurrency} workers`}</span></summary>
    <div className="grid grid-cols-2 gap-3 pb-2 pt-1">
      <label className="space-y-1 text-label text-muted-foreground">Worker ceiling<select aria-label="Worker ceiling" data-testid="workflow-concurrency" className={SELECT} value={draft.limits.concurrency} onChange={(event) => update("concurrency", Number(event.target.value))}><option value={0}>Auto (bounded)</option>{[1, 2, 4, 8, 16, 32, 64, 128, 256].map((count) => <option key={count} value={count}>{count} workers</option>)}</select></label>
      <NumberLimit label="Task budget" name="max_tasks" value={draft.limits.max_tasks} min={1} max={10000} onChange={update} />
      <NumberLimit label="Attempt budget" name="max_attempts" value={draft.limits.max_attempts} min={1} max={50000} onChange={update} />
      <NumberLimit label="Nesting depth" name="max_depth" value={draft.limits.max_depth} min={0} max={32} onChange={update} />
      <NumberLimit label="Output limit (bytes)" name="max_output_bytes" value={draft.limits.max_output_bytes} min={1024} max={10485760} onChange={update} />
      <label className="space-y-1 text-label text-muted-foreground">Token budget<Input type="number" aria-label="Token budget" min={1} placeholder="No token cap" value={draft.limits.token_budget ?? ""} className="h-8" onChange={(event) => update("token_budget", event.target.value ? Math.max(1, Number(event.target.value)) : null)} /></label>
      <label className="space-y-1 text-label text-muted-foreground">Time limit (minutes)<Input type="number" aria-label="Time limit in minutes" min={1} max={1440} value={draft.limits.wall_time_ms / 60000} className="h-8" onChange={(event) => update("wall_time_ms", Math.min(1440, Math.max(1, Number(event.target.value))) * 60000)} /></label>
    </div>
    <p className="pb-2 text-label leading-relaxed text-muted-foreground">Auto uses a bounded host ceiling. Retries share the attempt budget. Token limits control admission, not a hard dollar cap.</p>
  </details>;
}

function NumberLimit({ label, name, value, min, max, onChange }: { label: string; name: keyof WorkflowDraft["limits"]; value: number; min: number; max: number; onChange: (key: keyof WorkflowDraft["limits"], value: number) => void }) {
  return <label className="space-y-1 text-label text-muted-foreground">{label}<Input type="number" aria-label={label} data-testid={`workflow-limit-${name}`} value={value} min={min} max={max} className="h-8" onChange={(event) => onChange(name, Math.min(max, Math.max(min, Number(event.target.value))))} /></label>;
}
