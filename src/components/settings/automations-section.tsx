import { useCallback, useEffect, useMemo, useState } from "react";

import {
  CalendarClock,
  Check,
  Loader2,
  Pause,
  Pencil,
  Play,
  Plus,
  Trash2,
  X,
} from "lucide-react";

import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { cn } from "@/lib/utils";
import {
  automationsCreate,
  automationsDelete,
  automationsList,
  automationsRuns,
  automationsSetEnabled,
  automationsUpdate,
  hostsList,
  type AutomationInput,
  type AutomationRunView,
  type AutomationView,
  type HostView,
} from "@/tauri/commands";

/**
 * Settings → Automations.
 *
 * Scheduled agent runs: a named prompt + agent + recurrence that fires
 * on a chosen host. Mirrors the sidebar-list + detail-pane shape of the
 * Hosts section. The schedule is stored as a complete RFC 5545
 * iCalendar block; the builder below composes the common
 * hourly/daily/weekly cases, and a raw field handles anything else.
 */

type Frequency = "HOURLY" | "DAILY" | "WEEKLY";

const WEEKDAYS: Array<{ value: string; label: string }> = [
  { value: "MO", label: "Monday" },
  { value: "TU", label: "Tuesday" },
  { value: "WE", label: "Wednesday" },
  { value: "TH", label: "Thursday" },
  { value: "FR", label: "Friday" },
  { value: "SA", label: "Saturday" },
  { value: "SU", label: "Sunday" },
];

const AGENTS: Array<{ value: string; label: string }> = [
  { value: "claude", label: "Claude Code" },
  { value: "codex", label: "Codex" },
];

function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** Build a complete RFC 5545 schedule from the friendly builder fields. */
function composeSchedule(opts: {
  frequency: Frequency;
  hour: number;
  minute: number;
  weekday: string;
  timezone: string;
}): string {
  const now = new Date();
  const datestamp = `${now.getFullYear()}${pad2(now.getMonth() + 1)}${pad2(
    now.getDate(),
  )}`;
  if (opts.frequency === "HOURLY") {
    return `DTSTART:${datestamp}T000000Z\nRRULE:FREQ=HOURLY;BYMINUTE=${opts.minute};BYSECOND=0`;
  }
  const time = `${pad2(opts.hour)}${pad2(opts.minute)}00`;
  const dtstart = `DTSTART;TZID=${opts.timezone}:${datestamp}T${time}`;
  if (opts.frequency === "WEEKLY") {
    return `${dtstart}\nRRULE:FREQ=WEEKLY;BYDAY=${opts.weekday}`;
  }
  return `${dtstart}\nRRULE:FREQ=DAILY`;
}

interface BuilderState {
  frequency: Frequency;
  hour: number;
  minute: number;
  weekday: string;
}

/** Best-effort reverse of `composeSchedule` so editing an automation
 *  re-opens the builder pre-filled. Returns null for any schedule the
 *  builder cannot represent — the form then falls back to a raw field. */
function parseSchedule(schedule: string): BuilderState | null {
  const freq = schedule.match(/FREQ=(HOURLY|DAILY|WEEKLY)/);
  if (!freq) return null;
  const frequency = freq[1] as Frequency;
  // Reject anything the builder does not emit (intervals, bounds).
  if (/INTERVAL=|COUNT=|UNTIL=/.test(schedule)) return null;

  if (frequency === "HOURLY") {
    const minute = schedule.match(/BYMINUTE=(\d{1,2})/);
    return {
      frequency,
      hour: 0,
      minute: minute ? Number(minute[1]) : 0,
      weekday: "MO",
    };
  }
  const time = schedule.match(/:\d{8}T(\d{2})(\d{2})\d{2}/);
  if (!time) return null;
  const hour = Number(time[1]);
  const minute = Number(time[2]);
  if (frequency === "WEEKLY") {
    const byday = schedule.match(/BYDAY=([A-Z]{2})\b/);
    if (!byday) return null;
    return { frequency, hour, minute, weekday: byday[1] };
  }
  return { frequency, hour, minute, weekday: "MO" };
}

/** Human one-liner for the detail pane and list. */
function describeSchedule(schedule: string): string {
  const parsed = parseSchedule(schedule);
  if (!parsed) return "Custom schedule";
  const time = `${pad2(parsed.hour)}:${pad2(parsed.minute)}`;
  if (parsed.frequency === "HOURLY") {
    return `Hourly at :${pad2(parsed.minute)}`;
  }
  if (parsed.frequency === "DAILY") return `Daily at ${time}`;
  const day = WEEKDAYS.find((d) => d.value === parsed.weekday)?.label ?? parsed.weekday;
  return `Weekly on ${day} at ${time}`;
}

function byNameInsensitive(a: AutomationView, b: AutomationView): number {
  return a.name.toLowerCase().localeCompare(b.name.toLowerCase());
}

const browserTimezone = (): string => {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC";
  } catch {
    return "UTC";
  }
};

/** Draft state for the create/edit form. */
interface FormDraft {
  name: string;
  prompt: string;
  agent: string;
  timezone: string;
  hostId: number | null;
  projectPath: string;
  retentionLimit: number;
  builder: BuilderState;
  /** Raw schedule string — used when `rawMode` is on. */
  rawSchedule: string;
  rawMode: boolean;
}

function emptyDraft(): FormDraft {
  return {
    name: "",
    prompt: "",
    agent: "claude",
    timezone: browserTimezone(),
    hostId: null,
    projectPath: "",
    retentionLimit: 10,
    builder: { frequency: "DAILY", hour: 9, minute: 0, weekday: "MO" },
    rawSchedule: "",
    rawMode: false,
  };
}

function draftFromAutomation(a: AutomationView): FormDraft {
  const parsed = parseSchedule(a.schedule);
  return {
    name: a.name,
    prompt: a.prompt,
    agent: a.agent,
    timezone: a.timezone,
    hostId: a.host_id,
    projectPath: a.project_path ?? "",
    retentionLimit: a.retention_limit,
    builder: parsed ?? { frequency: "DAILY", hour: 9, minute: 0, weekday: "MO" },
    rawSchedule: a.schedule,
    rawMode: parsed === null,
  };
}

function draftToInput(draft: FormDraft): AutomationInput {
  const schedule = draft.rawMode
    ? draft.rawSchedule.trim()
    : composeSchedule({ ...draft.builder, timezone: draft.timezone });
  return {
    name: draft.name.trim(),
    prompt: draft.prompt,
    agent: draft.agent,
    schedule,
    timezone: draft.timezone.trim(),
    host_id: draft.hostId,
    project_path: draft.projectPath.trim() || null,
    retention_limit: draft.retentionLimit,
  };
}

export function AutomationsSection() {
  const [automations, setAutomations] = useState<AutomationView[]>([]);
  const [hosts, setHosts] = useState<HostView[]>([]);
  const [loading, setLoading] = useState(true);
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // `null` = not editing. A draft with no matching automation = create.
  const [draft, setDraft] = useState<FormDraft | null>(null);
  const [creating, setCreating] = useState(false);

  const reload = useCallback(async () => {
    setError(null);
    try {
      const [fresh, freshHosts] = await Promise.all([
        automationsList(),
        hostsList(),
      ]);
      setAutomations(fresh);
      setHosts(freshHosts);
      if (fresh.length > 0 && selectedId === null) {
        setSelectedId(fresh[0].id);
      } else if (fresh.length === 0) {
        setSelectedId(null);
      } else if (selectedId !== null && !fresh.find((a) => a.id === selectedId)) {
        setSelectedId(fresh[0]?.id ?? null);
      }
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setLoading(false);
    }
  }, [selectedId]);

  useEffect(() => {
    void reload();
    // Mount-only — `reload` closes over `selectedId` but we only want
    // the initial fetch here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const selected = useMemo(
    () => automations.find((a) => a.id === selectedId) ?? null,
    [automations, selectedId],
  );

  const startCreate = useCallback(() => {
    setCreating(true);
    setDraft(emptyDraft());
    setError(null);
  }, []);

  const startEdit = useCallback((automation: AutomationView) => {
    setCreating(false);
    setDraft(draftFromAutomation(automation));
    setError(null);
  }, []);

  const cancelForm = useCallback(() => {
    setDraft(null);
    setCreating(false);
    setError(null);
  }, []);

  const handleSave = useCallback(async () => {
    if (!draft) return;
    setBusy(true);
    setError(null);
    try {
      const input = draftToInput(draft);
      const saved =
        creating || selected === null
          ? await automationsCreate(input)
          : await automationsUpdate(selected.id, input);
      setAutomations((prev) => {
        const without = prev.filter((a) => a.id !== saved.id);
        return [...without, saved].sort(byNameInsensitive);
      });
      setSelectedId(saved.id);
      setDraft(null);
      setCreating(false);
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setBusy(false);
    }
  }, [draft, creating, selected]);

  const handleToggleEnabled = useCallback(async (automation: AutomationView) => {
    setBusy(true);
    setError(null);
    try {
      const updated = await automationsSetEnabled(
        automation.id,
        !automation.enabled,
      );
      setAutomations((prev) =>
        prev.map((a) => (a.id === updated.id ? updated : a)),
      );
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setBusy(false);
    }
  }, []);

  const handleDelete = useCallback(async (automation: AutomationView) => {
    if (
      !window.confirm(
        `Delete the automation "${automation.name}"? Its run history is kept.`,
      )
    ) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await automationsDelete(automation.id);
      setAutomations((prev) => prev.filter((a) => a.id !== automation.id));
      if (selectedId === automation.id) setSelectedId(null);
    } catch (err) {
      setError(typeof err === "string" ? err : String(err));
    } finally {
      setBusy(false);
    }
  }, [selectedId]);

  if (loading) {
    return (
      <div className="flex items-center justify-center py-12 text-sm text-muted-foreground">
        <Loader2 className="mr-2 size-4 animate-spin" />
        Loading automations…
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-[460px] gap-6">
      {/* Sidebar */}
      <div className="w-56 shrink-0 border-r border-border/60 pr-5 flex flex-col">
        <div className="mb-3 flex items-end justify-between gap-2">
          <p className="text-[10px] font-semibold uppercase tracking-[0.14em] text-muted-foreground/60">
            Automations
          </p>
          <span className="text-[11px] text-muted-foreground/60 tabular-nums">
            {automations.length}
          </span>
        </div>

        {automations.length === 0 && draft === null && (
          <div className="rounded-lg border border-dashed border-border/60 p-3 text-[12px] text-muted-foreground/80 leading-relaxed">
            No automations yet. Create one to run an agent on a schedule.
          </div>
        )}

        <ul className="space-y-px">
          {automations.map((automation) => (
            <li key={automation.id}>
              <button
                type="button"
                onClick={() => {
                  setSelectedId(automation.id);
                  setDraft(null);
                }}
                className={cn(
                  "group/row flex w-full items-center gap-2.5 rounded-md px-2.5 h-8 text-left text-[13px] transition-colors",
                  selectedId === automation.id && draft === null
                    ? "bg-muted text-foreground"
                    : "text-muted-foreground hover:bg-muted/40 hover:text-foreground",
                )}
              >
                <span
                  aria-hidden
                  title={automation.enabled ? "Enabled" : "Paused"}
                  className={cn(
                    "size-1.5 shrink-0 rounded-full transition-colors",
                    automation.enabled ? "bg-success" : "bg-muted-foreground/40",
                  )}
                />
                <span className="min-w-0 flex-1 truncate">{automation.name}</span>
                {automation.dirty && (
                  <span
                    title="Pending sync"
                    className="size-1.5 shrink-0 rounded-full bg-warning"
                  />
                )}
              </button>
            </li>
          ))}
        </ul>

        <div className="mt-4 pt-4 border-t border-border/40">
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="w-full justify-start gap-2 h-8 px-2.5 text-[13px] text-muted-foreground hover:text-foreground hover:bg-muted/40 border border-dashed border-border/60"
            onClick={startCreate}
          >
            <Plus className="size-3.5" />
            New automation
          </Button>
        </div>
      </div>

      {/* Detail */}
      <div className="flex-1 min-w-0">
        {error && (
          <div className="mb-4 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-[12px] text-destructive leading-relaxed">
            {error}
          </div>
        )}

        {draft !== null ? (
          <AutomationForm
            draft={draft}
            setDraft={setDraft}
            hosts={hosts}
            busy={busy}
            creating={creating || selected === null}
            onSave={handleSave}
            onCancel={cancelForm}
          />
        ) : !selected ? (
          <div className="flex h-full items-center justify-center text-center">
            <div className="space-y-3">
              <div className="mx-auto size-12 rounded-full bg-muted/40 border border-border/40 flex items-center justify-center">
                <CalendarClock className="size-5 text-muted-foreground/60" />
              </div>
              <p className="text-[13px] text-muted-foreground/80">
                Select an automation, or create a new one.
              </p>
            </div>
          </div>
        ) : (
          <AutomationDetail
            automation={selected}
            hosts={hosts}
            busy={busy}
            onEdit={() => startEdit(selected)}
            onToggleEnabled={() => void handleToggleEnabled(selected)}
            onDelete={() => void handleDelete(selected)}
          />
        )}
      </div>
    </div>
  );
}

// ── Detail view ──

function AutomationDetail({
  automation,
  hosts,
  busy,
  onEdit,
  onToggleEnabled,
  onDelete,
}: {
  automation: AutomationView;
  hosts: HostView[];
  busy: boolean;
  onEdit: () => void;
  onToggleEnabled: () => void;
  onDelete: () => void;
}) {
  const hostLabel =
    automation.host_id === null
      ? "This machine"
      : (hosts.find((h) => h.id === automation.host_id)?.name ??
        "Removed host");

  return (
    <div className="space-y-6">
      <div>
        <div className="mb-1 flex items-center gap-2">
          <h3 className="text-[15px] font-semibold tracking-tight text-foreground">
            {automation.name}
          </h3>
          <span
            className={cn(
              "rounded-full px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider border",
              automation.enabled
                ? "bg-success/15 border-success/30 text-success"
                : "bg-muted border-border/50 text-muted-foreground",
            )}
          >
            {automation.enabled ? "Enabled" : "Paused"}
          </span>
          {automation.dirty && (
            <span className="rounded-full bg-warning/15 border border-warning/30 px-2 py-0.5 text-[10px] font-medium uppercase tracking-wider text-warning">
              Pending sync
            </span>
          )}
        </div>
        <p className="text-[12px] text-muted-foreground/85">
          {describeSchedule(automation.schedule)} · {automation.agent} ·{" "}
          {hostLabel}
        </p>
      </div>

      <div className="grid grid-cols-2 gap-3">
        <FactCard label="Next run" value={formatStamp(automation.next_run_at)} />
        <FactCard label="Last run" value={formatStamp(automation.last_run_at)} />
      </div>

      <div className="space-y-1.5">
        <p className="text-[11px] font-semibold uppercase tracking-[0.12em] text-muted-foreground/60">
          Prompt
        </p>
        <div className="rounded-lg border border-border/60 bg-muted/30 p-3 text-[12.5px] text-foreground/90 leading-relaxed whitespace-pre-wrap max-h-40 overflow-y-auto">
          {automation.prompt}
        </div>
      </div>

      <RunHistory automationId={automation.id} />

      <div className="flex items-center justify-between pt-4 border-t border-border/40">
        <div className="flex gap-1.5">
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-[12px]"
            disabled={busy}
            onClick={onEdit}
          >
            <Pencil className="size-3.5" />
            Edit
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-8 gap-1.5 text-[12px]"
            disabled={busy}
            onClick={onToggleEnabled}
          >
            {automation.enabled ? (
              <>
                <Pause className="size-3.5" />
                Pause
              </>
            ) : (
              <>
                <Play className="size-3.5" />
                Resume
              </>
            )}
          </Button>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-8 gap-1.5 text-[12px] text-destructive hover:bg-destructive/10 hover:text-destructive"
          disabled={busy}
          onClick={onDelete}
        >
          <Trash2 className="size-3.5" />
          Delete
        </Button>
      </div>
    </div>
  );
}

function FactCard({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-lg border border-border/60 bg-muted/30 px-3 py-2.5">
      <p className="text-[10px] font-semibold uppercase tracking-[0.12em] text-muted-foreground/60">
        {label}
      </p>
      <p className="mt-1 text-[12.5px] text-foreground/90 tabular-nums">{value}</p>
    </div>
  );
}

function formatStamp(stamp: string | null): string {
  if (!stamp) return "—";
  const date = new Date(stamp);
  if (Number.isNaN(date.getTime())) return stamp;
  return date.toLocaleString();
}

// ── Run history ──

const RUN_STATUS_TONE: Record<string, string> = {
  scheduled: "bg-muted-foreground/40",
  running: "bg-warning",
  succeeded: "bg-success",
  failed: "bg-destructive",
  skipped_offline: "bg-muted-foreground/40",
  skipped_busy: "bg-muted-foreground/40",
};

function RunHistory({ automationId }: { automationId: number }) {
  const [runs, setRuns] = useState<AutomationRunView[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    automationsRuns(automationId, 20)
      .then((fresh) => {
        if (!cancelled) setRuns(fresh);
      })
      .catch(() => {
        if (!cancelled) setRuns([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [automationId]);

  return (
    <div className="space-y-1.5">
      <p className="text-[11px] font-semibold uppercase tracking-[0.12em] text-muted-foreground/60">
        Run history
      </p>
      {loading ? (
        <p className="text-[12px] text-muted-foreground/70 py-2">Loading…</p>
      ) : runs.length === 0 ? (
        <p className="text-[12px] text-muted-foreground/70 py-2">
          No runs yet. The next fire will appear here.
        </p>
      ) : (
        <ul className="rounded-lg border border-border/60 divide-y divide-border/40">
          {runs.map((run) => (
            <li
              key={run.id}
              className="flex items-center gap-2.5 px-3 py-2 text-[12px]"
            >
              <span
                aria-hidden
                className={cn(
                  "size-1.5 shrink-0 rounded-full",
                  RUN_STATUS_TONE[run.status] ?? "bg-muted-foreground/40",
                )}
              />
              <span className="font-mono text-[11.5px] text-muted-foreground/85 tabular-nums">
                {formatStamp(run.scheduled_for)}
              </span>
              <span className="ml-auto text-[11px] text-muted-foreground/75">
                {run.status.replace(/_/g, " ")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ── Create / edit form ──

function AutomationForm({
  draft,
  setDraft,
  hosts,
  busy,
  creating,
  onSave,
  onCancel,
}: {
  draft: FormDraft;
  setDraft: React.Dispatch<React.SetStateAction<FormDraft | null>>;
  hosts: HostView[];
  busy: boolean;
  creating: boolean;
  onSave: () => void;
  onCancel: () => void;
}) {
  const patch = (next: Partial<FormDraft>) =>
    setDraft((prev) => (prev ? { ...prev, ...next } : prev));
  const patchBuilder = (next: Partial<BuilderState>) =>
    setDraft((prev) =>
      prev ? { ...prev, builder: { ...prev.builder, ...next } } : prev,
    );

  return (
    <div className="space-y-5">
      <h3 className="text-[15px] font-semibold tracking-tight text-foreground">
        {creating ? "New automation" : "Edit automation"}
      </h3>

      <Field label="Name">
        <Input
          value={draft.name}
          onChange={(e) => patch({ name: e.target.value })}
          placeholder="Daily issue triage"
          className="h-9 text-[13px]"
          autoFocus
        />
      </Field>

      <Field label="Prompt" hint="What the agent does on every run.">
        <Textarea
          value={draft.prompt}
          onChange={(e) => patch({ prompt: e.target.value })}
          placeholder="Review newly opened issues and add labels…"
          className="min-h-24 text-[13px]"
        />
      </Field>

      <div className="grid grid-cols-2 gap-4">
        <Field label="Agent">
          <Select value={draft.agent} onValueChange={(v) => patch({ agent: v })}>
            <SelectTrigger className="h-9 text-[13px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              {AGENTS.map((agent) => (
                <SelectItem key={agent.value} value={agent.value}>
                  {agent.label}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>

        <Field label="Host">
          <Select
            value={draft.hostId === null ? "local" : String(draft.hostId)}
            onValueChange={(v) =>
              patch({ hostId: v === "local" ? null : Number(v) })
            }
          >
            <SelectTrigger className="h-9 text-[13px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="local">This machine</SelectItem>
              {hosts.map((host) => (
                <SelectItem key={host.id} value={String(host.id)}>
                  {host.name}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </Field>
      </div>

      <ScheduleField draft={draft} patch={patch} patchBuilder={patchBuilder} />

      <div className="grid grid-cols-2 gap-4">
        <Field label="Timezone">
          <Input
            value={draft.timezone}
            onChange={(e) => patch({ timezone: e.target.value })}
            className="h-9 text-[13px] font-mono"
          />
        </Field>
        <Field label="Keep last N runs" hint="Older run worktrees are pruned.">
          <Input
            type="number"
            min={1}
            max={1000}
            value={draft.retentionLimit}
            onChange={(e) =>
              patch({ retentionLimit: Number(e.target.value) || 10 })
            }
            className="h-9 text-[13px] tabular-nums"
          />
        </Field>
      </div>

      <Field label="Project path" hint="Repository the run operates in (optional).">
        <Input
          value={draft.projectPath}
          onChange={(e) => patch({ projectPath: e.target.value })}
          placeholder="/home/you/code/my-repo"
          className="h-9 text-[13px] font-mono"
        />
      </Field>

      <div className="flex justify-end gap-1.5 pt-2 border-t border-border/40">
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-8 gap-1.5 text-[12px]"
          disabled={busy}
          onClick={onCancel}
        >
          <X className="size-3.5" />
          Cancel
        </Button>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          className="h-8 gap-1.5 text-[12px]"
          disabled={busy}
          onClick={onSave}
        >
          {busy ? (
            <Loader2 className="size-3.5 animate-spin" />
          ) : (
            <Check className="size-3.5" />
          )}
          {creating ? "Create" : "Save"}
        </Button>
      </div>
    </div>
  );
}

function ScheduleField({
  draft,
  patch,
  patchBuilder,
}: {
  draft: FormDraft;
  patch: (next: Partial<FormDraft>) => void;
  patchBuilder: (next: Partial<BuilderState>) => void;
}) {
  const composed = draft.rawMode
    ? draft.rawSchedule
    : composeSchedule({ ...draft.builder, timezone: draft.timezone });

  return (
    <Field
      label="Schedule"
      hint={
        draft.rawMode
          ? "Raw RFC 5545 — a DTSTART line and one RRULE line."
          : undefined
      }
    >
      {draft.rawMode ? (
        <Textarea
          value={draft.rawSchedule}
          onChange={(e) => patch({ rawSchedule: e.target.value })}
          className="min-h-20 text-[12px] font-mono"
          placeholder={"DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY"}
        />
      ) : (
        <div className="flex flex-wrap items-center gap-2">
          <Select
            value={draft.builder.frequency}
            onValueChange={(v) => patchBuilder({ frequency: v as Frequency })}
          >
            <SelectTrigger className="h-9 w-32 text-[13px]">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value="HOURLY">Hourly</SelectItem>
              <SelectItem value="DAILY">Daily</SelectItem>
              <SelectItem value="WEEKLY">Weekly</SelectItem>
            </SelectContent>
          </Select>

          {draft.builder.frequency === "WEEKLY" && (
            <Select
              value={draft.builder.weekday}
              onValueChange={(v) => patchBuilder({ weekday: v })}
            >
              <SelectTrigger className="h-9 w-36 text-[13px]">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                {WEEKDAYS.map((day) => (
                  <SelectItem key={day.value} value={day.value}>
                    {day.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          )}

          <Input
            type="time"
            value={`${pad2(
              draft.builder.frequency === "HOURLY" ? 0 : draft.builder.hour,
            )}:${pad2(draft.builder.minute)}`}
            onChange={(e) => {
              const [h, m] = e.target.value.split(":");
              patchBuilder({ hour: Number(h) || 0, minute: Number(m) || 0 });
            }}
            className="h-9 w-28 text-[13px] tabular-nums"
          />
          {draft.builder.frequency === "HOURLY" && (
            <span className="text-[11.5px] text-muted-foreground/70">
              past each hour
            </span>
          )}
        </div>
      )}

      <div className="mt-1.5 flex items-center justify-between gap-3">
        <code className="min-w-0 truncate text-[10.5px] text-muted-foreground/60 font-mono">
          {composed.replace(/\n/g, " · ")}
        </code>
        <button
          type="button"
          className="shrink-0 text-[11px] text-muted-foreground/70 hover:text-foreground transition-colors"
          onClick={() => {
            if (draft.rawMode) {
              patch({ rawMode: false });
            } else {
              patch({ rawMode: true, rawSchedule: composed });
            }
          }}
        >
          {draft.rawMode ? "Use builder" : "Edit raw"}
        </button>
      </div>
    </Field>
  );
}

function Field({
  label,
  hint,
  children,
}: {
  label: string;
  hint?: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-1.5">
      <Label className="text-[13px] font-medium text-foreground">{label}</Label>
      {children}
      {hint && (
        <p className="text-[11px] text-muted-foreground/70 leading-relaxed">
          {hint}
        </p>
      )}
    </div>
  );
}
