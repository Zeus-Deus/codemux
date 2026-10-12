import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CheckIcon, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { cn } from "@/lib/utils";
import { useHermes, hermesProfileKey, hermesModelUnavailable, pickHermesProfile, type HermesProfile } from "@/stores/hermes-store";

/** One selectable model row, on the same row recipe as the provider picker's model list. */
function HermesModelRow({ title, subtitle, selected, disabled, tooltip, onClick }: {
  title: string; subtitle: string; selected: boolean; disabled?: boolean; tooltip?: string; onClick: () => void;
}) {
  return <button type="button" title={tooltip} disabled={disabled} aria-current={selected || undefined} onClick={onClick}
    className={cn("flex w-full items-start gap-2 rounded-md px-2 py-2 text-left transition-colors duration-100 hover:bg-surface-2 disabled:opacity-40", selected && "bg-surface-3")}>
    <span className="min-w-0 flex-1">
      <span className="block truncate font-medium">{title}</span>
      <span className="mt-0.5 block truncate text-muted-foreground/70">{subtitle}</span>
    </span>
    {selected && <CheckIcon aria-hidden className="mt-0.5 size-3.5 shrink-0" />}
  </button>;
}

/** Lives inside the existing provider picker; only native layout metadata is displayed. */
export function HermesProfileModels({ threadId, projectPath, model, onSelect, onProfileChange }: {
  threadId?: string | null; projectPath?: string | null; model: string | null; onSelect: (model: string) => void; onProfileChange?: () => void;
}) {
  const [profiles, setProfiles] = useState<HermesProfile[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const selection = useHermes(s => threadId ? s.selections[threadId] : undefined);
  const mode = useHermes(s => threadId ? s.modes[threadId] : undefined);
  const fixed = useHermes(s => threadId ? s.fixed[threadId] : false);
  const slot = useHermes(s => selection ? s.catalogs[hermesProfileKey(selection)] : undefined);
  const refresh = useHermes(s => s.refresh);
  useEffect(() => {
    let disposed = false;
    setLoading(true);
    void (async () => {
      try {
        if (threadId) await useHermes.getState().restore(threadId);
        const values = await invoke<HermesProfile[]>("hermes_profiles");
        if (disposed) return;
        setProfiles(values);
        const project = projectPath ?? "home";
        const picked = threadId && !useHermes.getState().selections[threadId] ? pickHermesProfile(values, project) : null;
        if (picked && threadId) useHermes.getState().select(threadId, project, picked, false);
      } catch (e) { if (!disposed) setError(String(e)); }
      finally { if (!disposed) setLoading(false); }
    })();
    return () => { disposed = true; };
  }, [threadId, projectPath]);
  useEffect(() => {
    if (!selection) return;
    const cached = useHermes.getState().catalogs[hermesProfileKey(selection)];
    if (!cached?.value && !cached?.loading) void refresh(selection);
  }, [selection, refresh]);
  const missing = selection && !loading && !profiles.some(p => hermesProfileKey(p) === hermesProfileKey(selection));
  const models = slot?.value?.session.models;
  const groups = new Map<string, NonNullable<typeof models>["availableModels"]>();
  for (const entry of models?.availableModels ?? []) {
    const service = entry._meta?.provider ?? "Models";
    groups.set(service, [...(groups.get(service) ?? []), entry]);
  }
  const unavailable = missing || slot?.value?.state === "unsupported";
  const modes = slot?.value?.session.modes;
  // `model` is null unless Hermes is the active provider, so no row checks then.
  const usingDefault = model === "profile_default";
  return <div className="flex min-h-0 flex-1 flex-col p-3 text-label" data-testid="hermes-profile-models">
    <div className="mb-3 flex items-center justify-between"><strong className="text-body">Hermes</strong><span className="text-muted-foreground">Experimental</span></div>
    <label className="mb-1 text-muted-foreground" htmlFor="hermes-profile">Profile {fixed ? "· fixed for this chat" : ""}</label>
    <Select disabled={fixed || loading || !threadId} value={selection ? hermesProfileKey(selection) : ""} onValueChange={value => {
      const p = profiles.find(p => hermesProfileKey(p) === value);
      if (p && threadId) { useHermes.getState().select(threadId, projectPath ?? "home", p); onProfileChange?.(); }
    }}>
      <SelectTrigger id="hermes-profile" aria-label="Hermes profile" size="sm" className="mb-2 w-full text-label">
        <SelectValue placeholder={loading ? "Finding profiles…" : "Choose a profile"} />
      </SelectTrigger>
      <SelectContent position="popper">
        {missing && selection && <SelectItem value={hermesProfileKey(selection)}>{selection.id} · repair required</SelectItem>}
        {profiles.map(p => <SelectItem key={hermesProfileKey(p)} value={hermesProfileKey(p)}>{p.id}</SelectItem>)}
      </SelectContent>
    </Select>
    <p className="mb-2 text-caption text-muted-foreground">Reasoning control unavailable. Configure identity, credentials and skills in Hermes.</p>
    {(error || slot?.error || missing || unavailable) && <p role="alert" className="mb-2 rounded-md border border-destructive/30 p-2 text-destructive">{error ?? slot?.error ?? (missing ? "Profile missing or replaced. Restore it in Hermes; this chat will not switch profiles." : slot?.value?.message ?? "This profile cannot resume durable chats on the installed adapter.")}</p>}
    {selection && <Button type="button" variant="ghost" size="xs" className="mb-2 self-end text-muted-foreground" disabled={slot?.loading} onClick={() => void refresh(selection)}>
      <RefreshCw aria-hidden className={cn(slot?.loading && "motion-safe:animate-spin")} />
      {slot?.loading ? "Loading models…" : "Refresh models"}
    </Button>}
    {modes && <div className="mb-2">
      <label className="mb-1 block text-caption text-muted-foreground" htmlFor="hermes-edit-policy">File edit policy · not a terminal sandbox</label>
      <Select value={mode ?? modes.currentModeId} onValueChange={value => {
        if (!threadId) return;
        void (async () => {
          try {
            if (fixed) await invoke("agent_chat_set_permission_mode", {provider:"hermes", threadId, mode:value});
            useHermes.setState(s => ({modes:{...s.modes,[threadId]:value}}));
          } catch (e) {setError(String(e));}
        })();
      }}>
        <SelectTrigger id="hermes-edit-policy" aria-label="Hermes file edit policy" size="sm" className="w-full text-label">
          <SelectValue />
        </SelectTrigger>
        <SelectContent position="popper">
          {modes.availableModes.map(m => <SelectItem key={m.id} value={m.id}>{m.description ? `${m.name} — ${m.description}` : m.name}</SelectItem>)}
        </SelectContent>
      </Select>
    </div>}
    <div className="min-h-0 flex-1 overflow-y-auto">
      {models && !unavailable && <HermesModelRow title="Use profile default" subtitle={models.currentModelId} selected={usingDefault} disabled={slot?.loading || fixed} onClick={() => onSelect("profile_default")} />}
      {Array.from(groups, ([service, entries]) => <section key={service}>
        <Eyebrow asChild><h3 className="block px-2 pb-1 pt-3">{service}</h3></Eyebrow>
        {entries.map(m => <HermesModelRow key={m.modelId} title={m.name} subtitle={m.description ?? m.modelId} selected={model === m.modelId}
          tooltip={hermesModelUnavailable(m.modelId) ? "Unsupported native restart route" : m.description}
          disabled={unavailable || slot?.loading || hermesModelUnavailable(m.modelId)} onClick={() => onSelect(m.modelId)} />)}
      </section>)}
      {models?.availableModels.length === 0 && <p className="p-2 text-muted-foreground">No models advertised. Configure this profile in Hermes and refresh.</p>}
      {model && model !== "profile_default" && models && !models.availableModels.some(m => m.modelId === model) && <p role="status" className="p-2 text-muted-foreground">Selected model unavailable: {model}</p>}
    </div>
  </div>;
}
