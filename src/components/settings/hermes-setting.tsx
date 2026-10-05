import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { useSettingsStore } from "@/stores/settings-store";
import { useHermes, hermesProfileKey, HERMES_DEFAULT_PROFILE_SETTING } from "@/stores/hermes-store";

const AUTOMATIC = "__automatic__";

function useHermesProfiles() {
  const profiles = useHermes(s => s.profiles);
  useEffect(() => {
    // A failed listing stays unloaded so the next mount retries instead of reporting profiles as missing.
    if (!useHermes.getState().profiles) useHermes.getState().loadProfiles().catch(() => {});
  }, []);
  return profiles;
}

/** Which profile a new Hermes chat starts with; the chat picker can still switch before the first turn. */
export function HermesDefaultProfileSetting() {
  const id = useSettingsStore(s => s.settings[HERMES_DEFAULT_PROFILE_SETTING] ?? "");
  const set = useSettingsStore(s => s.set);
  const profiles = useHermesProfiles();
  const unlisted = id && !profiles?.some(p => p.id === id);
  return <Select value={id || AUTOMATIC} onValueChange={v => set(HERMES_DEFAULT_PROFILE_SETTING, v === AUTOMATIC ? "" : v)}>
    <SelectTrigger aria-label="Default Hermes profile" className="h-9 w-48">
      <SelectValue />
    </SelectTrigger>
    <SelectContent>
      <SelectItem value={AUTOMATIC}>Automatic</SelectItem>
      {unlisted && <SelectItem value={id}>{profiles ? `${id} · not found` : id}</SelectItem>}
      {profiles?.map(p => <SelectItem key={hermesProfileKey(p)} value={p.id}>{p.id}</SelectItem>)}
    </SelectContent>
  </Select>;
}

export function HermesSetting() {
  const settings = useSettingsStore(s => s.settings);
  const set = useSettingsStore(s => s.set);
  const [installation, setInstallation] = useState(settings["hermes.installation"] ?? "hermes");
  const [root, setRoot] = useState(settings["hermes.root"] ?? "");
  const profiles = useHermesProfiles();
  const [status, setStatus] = useState("");
  return <section className="space-y-3 rounded-lg border border-border p-4" aria-label="Hermes setup">
    <h3 className="text-body font-medium">Hermes · experimental ACP integration</h3>
    <p className="text-label text-muted-foreground">Use existing profiles. Create profiles, configure credentials and install ACP dependencies in Hermes. Use one active interface per profile.</p>
    <label className="block text-label">Hermes executable<Input className="mt-1" value={installation} onChange={e => setInstallation(e.target.value)} /></label>
    <label className="block text-label">Hermes root<Input className="mt-1" placeholder="Default Hermes root" value={root} onChange={e => setRoot(e.target.value)} /></label>
    <Button type="button" variant="outline" size="sm" onClick={() => void (async () => {
      try {
        await set("hermes.installation", installation);
        await set("hermes.root", root);
        useHermes.setState({catalogs:{}});
        const values = await useHermes.getState().loadProfiles();
        setStatus(`${values.length} profiles found.`);
      } catch (e) { setStatus(String(e)); }
    })()}>Save and refresh</Button>
    <p role="status" className="text-label text-muted-foreground">{status}</p>
    {profiles?.map(p => <div key={p.home} className="flex items-center justify-between text-label"><span>{p.id}</span><Button type="button" variant="ghost" size="xs" onClick={() => void (async () => {
      await useHermes.getState().refresh(p);
      const slot = useHermes.getState().catalogs[hermesProfileKey(p)];
      setStatus(slot.error ?? slot.value?.message ?? `${p.id}: ${slot.value?.state ?? "unknown"}. External authentication and compressed-history recovery are not certified.`);
    })()}>Check runtime</Button><Button type="button" variant="ghost" size="xs" onClick={() => void invoke("hermes_disconnect", { profile: p }).then(() => setStatus("Disconnected. Background learning completion is not guaranteed; worktrees are retained."), e => setStatus(String(e)))}>Disconnect</Button></div>)}
  </section>;
}
