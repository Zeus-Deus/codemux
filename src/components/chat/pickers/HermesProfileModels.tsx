import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useHermes, hermesProfileKey, hermesModelUnavailable, type HermesProfile } from "@/stores/hermes-store";

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
        const preferred = useHermes.getState().preferred[projectPath ?? "home"];
        if (threadId && !useHermes.getState().selections[threadId] && preferred && values.some(p => hermesProfileKey(p) === hermesProfileKey(preferred))) {
          useHermes.getState().select(threadId, projectPath ?? "home", preferred);
        }
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
  return <div className="flex min-h-0 flex-1 flex-col p-3 text-xs" data-testid="hermes-profile-models">
    <div className="mb-3 flex items-center justify-between"><strong className="text-sm">Hermes</strong><span className="text-muted-foreground">Experimental</span></div>
    <label className="mb-1 text-muted-foreground" htmlFor="hermes-profile">Profile {fixed ? "· fixed for this chat" : ""}</label>
    <select id="hermes-profile" aria-label="Hermes profile" className="mb-2 w-full rounded-md border border-border bg-background p-2" disabled={fixed || loading || !threadId} value={selection ? hermesProfileKey(selection) : ""} onChange={e => {
      const p = profiles.find(p => hermesProfileKey(p) === e.target.value);
      if (p && threadId) { useHermes.getState().select(threadId, projectPath ?? "home", p); onProfileChange?.(); }
    }}>
      <option value="">{loading ? "Finding profiles…" : "Choose a profile"}</option>
      {missing && selection && <option value={hermesProfileKey(selection)}>{selection.id} · repair required</option>}
      {profiles.map(p => <option key={hermesProfileKey(p)} value={hermesProfileKey(p)}>{p.id}</option>)}
    </select>
    <p className="mb-2 text-[11px] text-muted-foreground">Reasoning control unavailable. Configure identity, credentials and skills in Hermes.</p>
    {(error || slot?.error || missing || unavailable) && <p role="alert" className="mb-2 rounded border border-destructive/30 p-2 text-destructive">{error ?? slot?.error ?? (missing ? "Profile missing or replaced. Restore it in Hermes; this chat will not switch profiles." : slot?.value?.message ?? "This profile cannot resume durable chats on the installed adapter.")}</p>}
    {selection && <button type="button" className="mb-2 self-end text-muted-foreground hover:text-foreground" disabled={slot?.loading} onClick={() => void refresh(selection)}>{slot?.loading ? "Loading models…" : "Refresh models"}</button>}
    {slot?.value?.session.modes && <label className="mb-2 block text-[11px] text-muted-foreground">File edit policy · not a terminal sandbox
      <select aria-label="Hermes file edit policy" className="mt-1 block w-full rounded border border-border bg-background p-1 text-xs text-foreground" value={mode ?? slot.value.session.modes.currentModeId} onChange={e => {
        const value = e.target.value;
        if (!threadId) return;
        void (async () => {
          try {
            if (fixed) await invoke("agent_chat_set_permission_mode", {provider:"hermes", threadId, mode:value});
            useHermes.setState(s => ({modes:{...s.modes,[threadId]:value}}));
          } catch (e) {setError(String(e));}
        })();
      }}>{slot.value.session.modes.availableModes.map(m => <option key={m.id} value={m.id}>{m.name} — {m.description}</option>)}</select>
    </label>}
    <div className="min-h-0 flex-1 overflow-y-auto">
      {models && !unavailable && <button type="button" disabled={slot?.loading || fixed} className="mb-1 w-full rounded p-2 text-left hover:bg-muted disabled:opacity-40" onClick={() => onSelect("profile_default")}><span className="font-medium">Use profile default</span><span className="block truncate text-muted-foreground">{models.currentModelId}</span></button>}
      {Array.from(groups, ([service, entries]) => <section key={service}><h3 className="px-2 pb-1 pt-3 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">{service}</h3>{entries.map(m => <button type="button" key={m.modelId} title={hermesModelUnavailable(m.modelId) ? "Unsupported native restart route" : m.description} disabled={unavailable || slot?.loading || hermesModelUnavailable(m.modelId)} className="w-full rounded p-2 text-left hover:bg-muted disabled:opacity-40" onClick={() => onSelect(m.modelId)}><span className="block truncate">{m.name}</span><span className="block truncate text-[10px] text-muted-foreground">{m.description ?? m.modelId}</span></button>)}</section>)}
      {models?.availableModels.length === 0 && <p className="p-2 text-muted-foreground">No models advertised. Configure this profile in Hermes and refresh.</p>}
      {model && model !== "profile_default" && models && !models.availableModels.some(m => m.modelId === model) && <p role="status" className="p-2 text-muted-foreground">Selected model unavailable: {model}</p>}
    </div>
  </div>;
}
