import { Code2, Play, RotateCw, Save } from "lucide-react";
import { NativeSelect } from "@/components/ui/native-select";
import { Button } from "@/components/ui/button";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { WORKFLOW_SIZES, type WorkflowRoutePlan, type WorkflowSettings } from "@/lib/workflows/workflow-settings";
import type { WorkflowDraft } from "@/stores/workflow-ui-store";
import type { WorkflowScript } from "@/tauri/workflows";
import { WORKFLOW_STARTER } from "./workflow-starter";


export interface WorkflowScriptEditorProps {
  draft: WorkflowDraft;
  scripts: WorkflowScript[];
  settings: WorkflowSettings;
  /** Live routes from Settings → Workflows, or why none can run. */
  live: WorkflowRoutePlan;
  pending: boolean;
  saving: boolean;
  saved: boolean;
  onChange: (patch: Partial<WorkflowDraft>) => void;
  onLaunch: (mode: "dry_run" | "live") => void;
  onSave: () => void;
  onOpenSettings: () => void;
}

/**
 * Power-user path: run a hand-written workflow program. Who runs it, file
 * access and limits come from Settings → Workflows, so this stays a short
 * form. Everyday workflows start from `/workflow <goal>` in a chat instead.
 */
export function WorkflowScriptEditor({ draft, scripts, settings, live, pending, saving, saved, onChange, onLaunch, onOpenSettings, onSave }: WorkflowScriptEditorProps) {
  const canStart = !!draft.title.trim() && !!draft.goal.trim() && !!draft.source.trim() && !pending;
  const canLive = canStart && !live.problem && live.routes.length > 0;
  const size = WORKFLOW_SIZES[settings.size];
  const routeNames = live.routes.map((route) => live.labels[route.id] ?? route.id);

  return <div className="min-w-0" data-testid="workflow-script-editor">
    <div className="space-y-4 px-3.5 pb-4 pt-3.5">
      <div>
        <h2 className="text-body-lg font-semibold">Custom workflow script</h2>
        <p className="mt-1 text-body-sm leading-relaxed text-muted-foreground">Run your own JavaScript program. For everyday work, type <code className="font-mono text-label text-foreground">/workflow</code> and a goal in any chat instead.</p>
      </div>
      <label className="block space-y-1.5"><Eyebrow tone="strong">Name</Eyebrow><Input aria-label="Workflow name" data-testid="workflow-title" value={draft.title} onChange={(event) => onChange({ title: event.target.value })} maxLength={160} /></label>
      <label className="block space-y-1.5"><Eyebrow tone="strong">Goal</Eyebrow><Textarea aria-label="Workflow goal" data-testid="workflow-goal" value={draft.goal} onChange={(event) => onChange({ goal: event.target.value })} rows={3} className="min-h-20 resize-y" maxLength={32000} /></label>
      <div className="space-y-1.5">
        <div className="flex items-center justify-between gap-2">
          <Eyebrow tone="strong">Script</Eyebrow>
          {scripts.length > 0 && <NativeSelect size="sm" aria-label="Saved workflow script" data-testid="workflow-saved-script" className="w-auto max-w-[60%]" value="" onChange={(event) => { const script = scripts.find((item) => item.id === event.target.value); if (script) onChange({ source: script.source, title: script.title }); }}><option value="">Load saved…</option>{scripts.map((script) => <option key={script.id} value={script.id}>{script.title}</option>)}</NativeSelect>}
        </div>
        <Textarea aria-label="Workflow JavaScript" data-testid="workflow-script" value={draft.source} onChange={(event) => onChange({ source: event.target.value })} spellCheck={false} rows={12} className="min-h-64 resize-y whitespace-pre font-mono text-label leading-relaxed" maxLength={128000} />
        <div className="flex flex-wrap items-center gap-1.5">
          <Button variant="outline" size="xs" data-testid="workflow-save-script" disabled={!draft.title.trim() || !draft.source.trim() || saving} onClick={onSave}><Save />{saving ? "Saving…" : "Save script"}</Button>
          <Button variant="ghost" size="xs" onClick={() => onChange({ source: WORKFLOW_STARTER })}><RotateCw />Reset to starter</Button>
          {saved && <span role="status" className="text-label text-status-open">Saved</span>}
        </div>
        <details className="text-body-sm"><summary className="cursor-pointer py-1 text-label font-medium text-muted-foreground">Script reference</summary><p className="mt-1 text-label leading-relaxed text-muted-foreground">Use <code>agent(prompt, options)</code> for a task and its result, <code>parallel(items, fn)</code> to fan out, and <code>workflow.addTask(spec)</code> / <code>workflow.wait(id)</code> for dependencies. Read <code>goal</code>, <code>routes</code> and <code>args</code>; pick a model with <code>route_id</code>. Await workflow operations before returning. Scripts have no imports, network, files or timers.</p></details>
      </div>
    </div>
    <div className="sticky bottom-0 space-y-2 border-t border-border/60 bg-background px-3.5 py-3">
      <p className="break-words text-label leading-relaxed text-muted-foreground" data-testid="workflow-script-routes">
        {live.problem ?? `Runs on ${routeNames.join(", ")}`} · {settings.allowWrites ? "edits isolated for review" : "read-only"} · {size.label.toLowerCase()} ({size.concurrency} at once, {size.minutes} min).{" "}
        <button type="button" className="text-foreground underline-offset-2 hover:underline" onClick={onOpenSettings}>Change in Settings</button>
      </p>
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" data-testid="workflow-live-run" disabled={!canLive} onClick={() => onLaunch("live")}><Play />{pending ? "Starting…" : "Run"}</Button>
        <Button size="sm" variant="outline" data-testid="workflow-dry-run" disabled={!canStart} onClick={() => onLaunch("dry_run")} title="Exercise the script without calling any model"><Code2 />Dry run</Button>
      </div>
    </div>
  </div>;
}
