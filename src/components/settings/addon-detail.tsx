import { useEffect, useRef, useState } from "react";
import { ArrowLeft, RefreshCw, ShieldCheck } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import { addonInvoke } from "@/lib/addons/bridge";
import { refreshAddons } from "@/lib/addons/platform";
import {
  addonMessage,
  type AddonCredentialState,
  type AddonDiagnostics,
  type AddonInstallation,
} from "@/lib/addons/types";
import { Capabilities, ProblemAlert } from "./addon-parts";
import {
  CREDENTIAL_STATES,
  TIER_LABELS,
  count,
  platformLabel,
  sourceIdentity,
} from "./addon-presentation";

function Credentials({
  installation,
  states,
}: {
  installation: AddonInstallation;
  states: Record<string, AddonCredentialState> | undefined;
}) {
  const [values, setValues] = useState<Record<string, string>>({});
  const [session, setSession] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState("");
  const [notice, setNotice] = useState("");
  if (!installation.manifest.credentials.length) return null;
  const run = async (action: () => Promise<void>) => {
    setBusy(true);
    setFailure("");
    setNotice("");
    try {
      await action();
    } catch (e) {
      setFailure(addonMessage(e));
    } finally {
      setBusy(false);
      // The host also announces the change; refresh so the state is current
      // even if that event is still in flight.
      void refreshAddons();
    }
  };
  return (
    <section className="space-y-3 border-t pt-5">
      <h3 className="font-medium">Credentials</h3>
      <p className="text-body text-muted-foreground">
        Values stay in CodeMux’s credential store and are attached only to the
        declared service. Add-ons cannot read them.
      </p>
      {installation.manifest.credentials.map((field) => {
        const state = states?.[field.id];
        const described = state ? CREDENTIAL_STATES[state] : null;
        return (
          <div key={field.id} className="space-y-2">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <label className="text-body" htmlFor={`credential-${field.id}`}>
                {field.label}
              </label>
              {described && (
                <Badge
                  variant={state === "not-configured" ? "outline" : "secondary"}
                  data-state={state}
                >
                  {described.label}
                </Badge>
              )}
            </div>
            <div className="flex gap-2">
              <Input
                id={`credential-${field.id}`}
                type="password"
                autoComplete="off"
                value={values[field.id] ?? ""}
                placeholder="Enter a new value"
                aria-describedby={`credential-${field.id}-state`}
                onChange={(e) => {
                  const value = e.target.value;
                  setValues((all) => ({ ...all, [field.id]: value }));
                }}
              />
              <Button
                disabled={busy || !values[field.id]}
                onClick={() =>
                  void run(async () => {
                    const sent = values[field.id];
                    await addonInvoke("addon_credential_set", {
                      id: installation.manifest.id,
                      credentialId: field.id,
                      value: sent,
                      sessionOnly: session,
                    });
                    // The fields stay editable during the save: clear only
                    // the value that was stored and keep anything typed since.
                    setValues((all) =>
                      all[field.id] === sent ? { ...all, [field.id]: "" } : all,
                    );
                  })
                }
              >
                Save
              </Button>
              {(state === "saved" || state === "session-only") && (
                <Button
                  variant="outline"
                  disabled={busy}
                  aria-label={`Clear ${field.label}`}
                  onClick={() =>
                    void run(async () => {
                      const warnings = await addonInvoke<string[]>(
                        "addon_credential_clear",
                        {
                          id: installation.manifest.id,
                          credentialId: field.id,
                        },
                      );
                      if (warnings?.length) setNotice(warnings.join(" "));
                    })
                  }
                >
                  Clear
                </Button>
              )}
            </div>
            <p
              id={`credential-${field.id}-state`}
              className="text-label text-muted-foreground"
            >
              {described ? described.detail(field.origin) : field.origin}
            </p>
          </div>
        );
      })}
      <label className="flex items-center gap-2 text-body">
        <input
          type="checkbox"
          checked={session}
          onChange={(e) => setSession(e.target.checked)}
        />
        Store new values for this session only
      </label>
      {notice && (
        <p role="status" className="text-body text-muted-foreground">
          {notice}
        </p>
      )}
      <ProblemAlert problem={failure} />
    </section>
  );
}

function Compatibility({ installation }: { installation: AddonInstallation }) {
  const compatibility = installation.compatibility;
  if (!compatibility) return null;
  const platforms = compatibility.platforms.map(platformLabel).join(", ");
  return (
    <section className="space-y-2 border-t pt-5 text-body">
      <h3 className="font-medium">Compatibility</h3>
      <ul className="space-y-1 text-muted-foreground">
        <li>
          Requires add-on API {compatibility.api}; this CodeMux provides{" "}
          {compatibility.hostApi}.
        </li>
        <li>
          Built for {platforms}; this device is{" "}
          {compatibility.platform
            ? platformLabel(compatibility.platform)
            : "not a supported add-on platform"}
          .
        </li>
      </ul>
      {compatibility.compatible ? (
        <p>Compatible with this device.</p>
      ) : (
        <p className="text-destructive">
          Not compatible: {compatibility.reason}
        </p>
      )}
    </section>
  );
}

const LEVELS = ["error", "warn", "info", "log", "debug"] as const;
const LEVEL_LABELS: Record<(typeof LEVELS)[number], [string, string]> = {
  error: ["error", "errors"],
  warn: ["warning", "warnings"],
  info: ["info", "info"],
  log: ["log", "logs"],
  debug: ["debug", "debug"],
};
const time = (at: number) => new Date(at).toLocaleTimeString();
/** Bounded log activity: counts, levels, sizes and times, never text. */
function Diagnostics({ id }: { id: string }) {
  const [value, setValue] = useState<AddonDiagnostics | null>(null);
  const [failure, setFailure] = useState("");
  const [loading, setLoading] = useState(true);
  const request = useRef(0);
  const load = async () => {
    const current = ++request.current;
    setLoading(true);
    setFailure("");
    try {
      const next = await addonInvoke<AddonDiagnostics>("addon_diagnostics", {
        id,
      });
      if (current === request.current) setValue(next);
    } catch (e) {
      if (current === request.current) setFailure(addonMessage(e));
    } finally {
      if (current === request.current) setLoading(false);
    }
  };
  useEffect(() => {
    void load();
    return () => {
      request.current += 1;
    };
  }, [id]);
  const logs = value?.logs ?? [];
  const counts = LEVELS.map(
    (level) => [level, logs.filter((l) => l.level === level).length] as const,
  ).filter(([, count]) => count > 0);
  const recent = logs.slice(-8).reverse();
  return (
    <section className="space-y-3 border-t pt-5 text-body">
      <div className="flex items-center justify-between gap-3">
        <h3 className="font-medium">Diagnostics</h3>
        <Button
          size="sm"
          variant="outline"
          disabled={loading}
          onClick={() => void load()}
        >
          <RefreshCw className="size-3" />
          Refresh diagnostics
        </Button>
      </div>
      <p className="text-muted-foreground">
        Log activity from this session. CodeMux records only the time, level
        and size of each entry, never its text.
      </p>
      <ProblemAlert problem={failure} />
      {loading && !value ? (
        <p role="status" className="text-muted-foreground">
          Loading diagnostics…
        </p>
      ) : value && value.received === 0 ? (
        <p>No log activity this session.</p>
      ) : value ? (
        <>
          <p>
            {count(value.received, "log entry", "log entries")} this session
            {value.received > logs.length &&
              ` · the latest ${logs.length} are kept`}
            {logs.length > 0 && ` · last at ${time(logs[logs.length - 1].at)}`}
          </p>
          {counts.length > 0 && (
            <p className="text-muted-foreground">
              {counts
                .map(([level, n]) => count(n, ...LEVEL_LABELS[level]))
                .join(" · ")}
            </p>
          )}
          <ol aria-label="Recent log entries" className="space-y-1 text-label">
            {recent.map((entry, index) => (
              <li
                key={`${entry.at}/${index}`}
                className="flex gap-3 text-muted-foreground"
              >
                <span className="tabular-nums">{time(entry.at)}</span>
                <span
                  className={
                    entry.level === "error"
                      ? "text-destructive"
                      : entry.level === "warn"
                        ? "text-warning"
                        : undefined
                  }
                >
                  {entry.level}
                </span>
                <span className="tabular-nums">{entry.bytes} bytes</span>
              </li>
            ))}
          </ol>
        </>
      ) : null}
    </section>
  );
}

function Identity({ installation }: { installation: AddonInstallation }) {
  const { source, catalog } = installation;
  if (source.kind === "local")
    return (
      <>
        <span>Local / unverified</span>
        <span className="text-label text-muted-foreground">
          Imported from a local package; not reviewed by the CodeMux catalog.
        </span>
      </>
    );
  return (
    <>
      <span className="flex flex-wrap items-center gap-1.5">
        {catalog?.tier === "official" && (
          <ShieldCheck className="size-4 text-muted-foreground" aria-hidden />
        )}
        {sourceIdentity(source, catalog)}
        {catalog?.tier && (
          <Badge variant="outline">{TIER_LABELS[catalog.tier]}</Badge>
        )}
      </span>
      <span className="text-label text-muted-foreground">
        {catalog?.listed === false
          ? "No longer listed in the reviewed catalog."
          : catalog?.listed === true
            ? "Listed in the reviewed catalog."
            : "Catalog listing unknown until the catalog is refreshed."}
      </span>
    </>
  );
}

export function AddonDetail({
  installation,
  credentialStates,
  back,
  onError,
  onRollback,
}: {
  installation: AddonInstallation;
  credentialStates: Record<string, AddonCredentialState> | undefined;
  back: () => void;
  onError: (message: string) => void;
  onRollback: (installation: AddonInstallation) => void;
}) {
  const [settings, setSettings] = useState<Record<string, unknown>>({});
  const [configuration, setConfiguration] = useState<
    "loading" | "ready" | "failed"
  >("loading");
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    let live = true;
    void addonInvoke<Record<string, unknown>>("addon_settings_get", {
      id: installation.manifest.id,
    })
      .then((value) => {
        if (!live) return;
        setSettings(value);
        setConfiguration("ready");
      })
      .catch((e) => {
        if (!live) return;
        setConfiguration("failed");
        onError(addonMessage(e));
      });
    return () => {
      live = false;
    };
  }, [installation.installationId]);
  const { manifest } = installation;
  return (
    <div className="space-y-6">
      <Button variant="ghost" onClick={back}>
        <ArrowLeft className="size-4" /> Installed
      </Button>
      <div>
        <h2 className="text-xl font-semibold">{manifest.name}</h2>
        <p className="mt-1 text-body text-muted-foreground">
          {manifest.description}
        </p>
      </div>
      <div className="grid gap-1 text-body">
        <Identity installation={installation} />
        <span className="text-label text-muted-foreground">
          Author (as stated by the package): {manifest.author.name}
        </span>
        <span>
          Version {manifest.version}
          {installation.updateAvailable &&
            ` · version ${installation.updateAvailable} is available`}
        </span>
        <span className="break-all text-muted-foreground">
          {manifest.repository}
        </span>
        <code className="break-all text-label text-muted-foreground">
          SHA-256 {installation.digest}
        </code>
      </div>
      {installation.failure && (
        <div
          role="alert"
          className="rounded-lg border border-destructive/30 p-3 text-body"
        >
          {installation.failure}
        </div>
      )}
      <Capabilities manifest={manifest} />
      {manifest.settings.length > 0 && (
        <form
          className="space-y-4 border-t pt-5"
          onSubmit={async (e) => {
            e.preventDefault();
            if (configuration !== "ready" || busy) return;
            setBusy(true);
            setSaved(false);
            try {
              await addonInvoke("addon_settings_set", {
                id: manifest.id,
                settings,
              });
              setSaved(true);
            } catch (error) {
              onError(addonMessage(error));
            } finally {
              setBusy(false);
            }
          }}
        >
          <h3 className="font-medium">Configuration</h3>
          {configuration === "loading" && (
            <p role="status" className="text-body text-muted-foreground">
              Loading configuration…
            </p>
          )}
          {manifest.settings.map((field) => (
            <label key={field.id} className="grid gap-2 text-body">
              {field.label}
              {field.type === "boolean" ? (
                <Switch
                  disabled={configuration !== "ready" || busy}
                  aria-label={field.label}
                  checked={settings[field.id] === true}
                  onCheckedChange={(value) =>
                    setSettings({ ...settings, [field.id]: value })
                  }
                />
              ) : field.type === "enum" ? (
                <select
                  disabled={configuration !== "ready" || busy}
                  className="rounded-md border bg-background p-2"
                  value={String(settings[field.id] ?? field.default)}
                  onChange={(e) =>
                    setSettings({ ...settings, [field.id]: e.target.value })
                  }
                >
                  {field.values.map((value) => (
                    <option key={value}>{value}</option>
                  ))}
                </select>
              ) : (
                <Input
                  disabled={configuration !== "ready" || busy}
                  type={field.type === "integer" ? "number" : "text"}
                  min={field.type === "integer" ? field.min : undefined}
                  max={field.type === "integer" ? field.max : undefined}
                  value={String(settings[field.id] ?? field.default)}
                  onChange={(e) =>
                    setSettings({
                      ...settings,
                      [field.id]:
                        field.type === "integer"
                          ? Number(e.target.value)
                          : e.target.value,
                    })
                  }
                />
              )}
            </label>
          ))}
          <div className="flex items-center gap-3">
            <Button disabled={configuration !== "ready" || busy}>
              Save settings
            </Button>
            {saved && (
              <span role="status" className="text-body text-muted-foreground">
                Saved
              </span>
            )}
          </div>
        </form>
      )}
      <Credentials installation={installation} states={credentialStates} />
      <Compatibility installation={installation} />
      <Diagnostics id={manifest.id} />
      {installation.previous && (
        <section className="space-y-3 border-t pt-5 text-body">
          <h3 className="font-medium">Rollback</h3>
          <p className="text-muted-foreground">
            Restore version {installation.previous.manifest.version} and the
            private data snapshot taken before this update.
          </p>
          <Button
            size="sm"
            variant="outline"
            onClick={() => onRollback(installation)}
          >
            <RefreshCw className="size-3" />
            Rollback
          </Button>
        </section>
      )}
    </div>
  );
}
