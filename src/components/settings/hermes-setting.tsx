import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSettingsStore } from "@/stores/settings-store";
import { useHermes, hermesProfileKey, type HermesProfile } from "@/stores/hermes-store";

export function HermesSetting() {
  const settings = useSettingsStore(s => s.settings);
  const set = useSettingsStore(s => s.set);
  const [installation, setInstallation] = useState(settings["hermes.installation"] ?? "hermes");
  const [root, setRoot] = useState(settings["hermes.root"] ?? "");
  const [profiles, setProfiles] = useState<HermesProfile[]>([]);
  const [status, setStatus] = useState("");
  return <section className="space-y-3 rounded-lg border border-border p-4" aria-label="Hermes setup">
    <h3 className="text-sm font-medium">Hermes · experimental ACP integration</h3>
    <p className="text-xs text-muted-foreground">Use existing profiles. Create profiles, configure credentials and install ACP dependencies in Hermes. Use one active interface per profile.</p>
    <label className="block text-xs">Hermes executable<input className="mt-1 block w-full rounded border border-border bg-background p-2" value={installation} onChange={e => setInstallation(e.target.value)} /></label>
    <label className="block text-xs">Hermes root<input className="mt-1 block w-full rounded border border-border bg-background p-2" placeholder="Default Hermes root" value={root} onChange={e => setRoot(e.target.value)} /></label>
    <button className="rounded border border-border px-3 py-1 text-xs" onClick={() => void (async () => {
      try {
        await set("hermes.installation", installation);
        await set("hermes.root", root);
        useHermes.setState({catalogs:{}});
        const values = await invoke<HermesProfile[]>("hermes_profiles");
        setProfiles(values); setStatus(`${values.length} profiles found. Select a profile in chat to check ACP and model compatibility.`);
      } catch (e) { setStatus(String(e)); }
    })()}>Save and refresh</button>
    <p role="status" className="text-xs text-muted-foreground">{status}</p>
    {profiles.map(p => <div key={p.home} className="flex items-center justify-between text-xs"><span>{p.id}</span><button className="rounded px-2 py-1 hover:bg-muted" onClick={() => void (async () => {
      await useHermes.getState().refresh(p);
      const slot = useHermes.getState().catalogs[hermesProfileKey(p)];
      setStatus(slot.error ?? slot.value?.message ?? `${p.id}: ${slot.value?.state ?? "unknown"}. External authentication and compressed-history recovery are not certified.`);
    })()}>Check runtime</button><button className="rounded px-2 py-1 hover:bg-muted" onClick={() => void invoke("hermes_disconnect", { profile: p }).then(() => setStatus("Disconnected. Background learning completion is not guaranteed; worktrees are retained."), e => setStatus(String(e)))}>Disconnect</button></div>)}
  </section>;
}
