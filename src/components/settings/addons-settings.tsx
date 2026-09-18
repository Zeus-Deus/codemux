import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  ArrowLeft,
  PackagePlus,
  Pause,
  Puzzle,
  RefreshCw,
  ShieldCheck,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { AddonCatalog } from "@/components/addons/addon-catalog";
import { beginAddonRevocation, useAddonsStore } from "@/stores/addons-store";
import { addonInvoke } from "@/lib/addons/bridge";
import { refreshAddons } from "@/lib/addons/platform";
import {
  addonMessage,
  type AddonInstallation,
  type AddonManifest,
  type AddonReview,
} from "@/lib/addons/types";
function Capabilities({ manifest }: { manifest: AddonManifest }) {
  return (
    <div className="space-y-3 text-body">
      <h4 className="font-medium">Requested access</h4>
      {!manifest.permissions.length && !manifest.http.length ? (
        <p className="text-muted-foreground">
          Private settings, storage, and declared UI only.
        </p>
      ) : (
        <ul className="list-inside list-disc space-y-1">
          {manifest.permissions.map((p) => (
            <li key={p}>
              {(
                {
                  "workspace.read": "Read local project metadata",
                  "git.read": "Read a bounded Git summary",
                  "composer.append": "Append text after your interaction",
                  "external.open": "Open HTTPS links after your interaction",
                } as Record<string, string>
              )[p] ?? p}
            </li>
          ))}
          {manifest.http.map((grant) => (
            <li key={grant.origin}>
              {grant.methods.join(", ")} {grant.origin}
              {grant.credential ? " · host-managed credential" : ""}
            </li>
          ))}
        </ul>
      )}
      <p className="text-muted-foreground">
        {manifest.contributes.commands.length} commands ·{" "}
        {manifest.contributes.panels.length} panels ·{" "}
        {manifest.contributes.composerActions.length} composer actions
      </p>
      {manifest.http.some((grant) =>
        grant.methods.some((method) => method !== "GET"),
      ) && (
        <p className="text-muted-foreground">
          This add-on can write to the listed external services using the
          declared methods.
        </p>
      )}
      {manifest.http.length > 0 && (
        <p className="text-muted-foreground">
          Data sent to an external service cannot be recalled by removing the
          add-on.
        </p>
      )}
    </div>
  );
}
function Credentials({
  installation,
  onError,
}: {
  installation: AddonInstallation;
  onError: (message: string) => void;
}) {
  const [values, setValues] = useState<Record<string, string>>({});
  const [session, setSession] = useState(false);
  const [busy, setBusy] = useState(false);
  if (!installation.manifest.credentials.length) return null;
  return (
    <section className="space-y-3 border-t pt-5">
      <h3 className="font-medium">Credentials</h3>
      <p className="text-body text-muted-foreground">
        Values stay in CodeMux’s credential store and are attached only to the
        declared service. Add-ons cannot read them.
      </p>
      {installation.manifest.credentials.map((field) => (
        <div key={field.id} className="space-y-2">
          <label className="text-body" htmlFor={`credential-${field.id}`}>
            {field.label}
          </label>
          <div className="flex gap-2">
            <Input
              id={`credential-${field.id}`}
              type="password"
              autoComplete="off"
              value={values[field.id] ?? ""}
              placeholder="Enter a new value"
              onChange={(e) =>
                setValues({ ...values, [field.id]: e.target.value })
              }
            />
            <Button
              disabled={busy || !values[field.id]}
              onClick={async () => {
                setBusy(true);
                try {
                  await addonInvoke("addon_credential_set", {
                    id: installation.manifest.id,
                    credentialId: field.id,
                    value: values[field.id],
                    sessionOnly: session,
                  });
                  setValues({ ...values, [field.id]: "" });
                } catch (e) {
                  onError(addonMessage(e));
                } finally {
                  setBusy(false);
                }
              }}
            >
              Save
            </Button>
          </div>
          <p className="text-label text-muted-foreground">{field.origin}</p>
        </div>
      ))}
      <label className="flex items-center gap-2 text-body">
        <input
          type="checkbox"
          checked={session}
          onChange={(e) => setSession(e.target.checked)}
        />
        Store new values for this session only
      </label>
    </section>
  );
}
function Configure({
  installation,
  back,
  onError,
}: {
  installation: AddonInstallation;
  back: () => void;
  onError: (message: string) => void;
}) {
  const [settings, setSettings] = useState<Record<string, unknown>>({});
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(false);
  useEffect(() => {
    void addonInvoke<Record<string, unknown>>("addon_settings_get", {
      id: installation.manifest.id,
    })
      .then(setSettings)
      .catch((e) => onError(addonMessage(e)));
  }, [installation.installationId]);
  return (
    <div className="space-y-6">
      <Button variant="ghost" onClick={back}>
        <ArrowLeft className="size-4" /> Installed
      </Button>
      <div>
        <h2 className="text-xl font-semibold">{installation.manifest.name}</h2>
        <p className="mt-1 text-body text-muted-foreground">
          {installation.manifest.description}
        </p>
      </div>
      <div className="grid gap-1 text-body">
        <span>
          {installation.source.kind === "catalog"
            ? "Catalog source"
            : "Local / unverified"}{" "}
          · {installation.manifest.version} · API {installation.manifest.api}
        </span>
        <span className="break-all text-muted-foreground">
          {installation.manifest.repository}
        </span>
        <code className="break-all text-label text-muted-foreground">
          SHA-256 {installation.digest}
        </code>
      </div>
      <Capabilities manifest={installation.manifest} />
      {installation.manifest.settings.length > 0 && (
        <form
          className="space-y-4 border-t pt-5"
          onSubmit={async (e) => {
            e.preventDefault();
            setBusy(true);
            setSaved(false);
            try {
              await addonInvoke("addon_settings_set", {
                id: installation.manifest.id,
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
          {installation.manifest.settings.map((field) => (
            <label key={field.id} className="grid gap-2 text-body">
              {field.label}
              {field.type === "boolean" ? (
                <Switch
                  aria-label={field.label}
                  checked={settings[field.id] === true}
                  onCheckedChange={(value) =>
                    setSettings({ ...settings, [field.id]: value })
                  }
                />
              ) : field.type === "enum" ? (
                <select
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
            <Button disabled={busy}>Save settings</Button>
            {saved && (
              <span role="status" className="text-body text-muted-foreground">
                Saved
              </span>
            )}
          </div>
        </form>
      )}
      <Credentials installation={installation} onError={onError} />
      {installation.failure && (
        <div
          role="alert"
          className="rounded-lg border border-destructive/30 p-3 text-body"
        >
          {installation.failure}
        </div>
      )}
    </div>
  );
}
export function AddonsSettings() {
  const state = useAddonsStore();
  const [selected, setSelected] = useState<string | null>(null);
  const [review, setReview] = useState<AddonReview | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [replace, setReplace] = useState(false);
  const [enable, setEnable] = useState(true);
  const [remove, setRemove] = useState<AddonInstallation | null>(null);
  const [keep, setKeep] = useState(false);
  const [restoreData, setRestoreData] = useState(false);
  const [rollback, setRollback] = useState<AddonInstallation | null>(null);
  const [tab, setTab] = useState<"installed" | "browse">("installed");
  const [target, setTarget] = useState("");
  const [linkOpen, setLinkOpen] = useState(false);
  const showReview = (value: AddonReview) => {
    setRestoreData(false);
    setReplace(false);
    setEnable(true);
    setReview(value);
  };
  useEffect(() => {
    void refreshAddons();
  }, []);
  useEffect(() => {
    if (state.developmentReview) {
      showReview(state.developmentReview);
      useAddonsStore.setState({ developmentReview: null });
    }
  }, [state.developmentReview]);
  const perform = async (action: () => Promise<unknown>, revoke?: string) => {
    const release = revoke ? beginAddonRevocation(revoke) : undefined;
    setBusy(true);
    setError("");
    try {
      await action();
    } catch (cause) {
      setError(addonMessage(cause));
    } finally {
      await refreshAddons();
      release?.();
      setBusy(false);
    }
  };
  const installation = state.installed.find((i) => i.manifest.id === selected);
  if (isRemoteClient())
    return (
      <div className="space-y-3">
        <h2 className="text-xl font-semibold">Add-ons</h2>
        <p className="text-body text-muted-foreground">
          Add-ons run in the local desktop app. Installation and plugin
          operations are unavailable in a remote browser.
        </p>
      </div>
    );
  return (
    <div className="max-w-3xl space-y-6">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h2 className="text-xl font-semibold">Add-ons</h2>
          <p className="mt-1 text-body text-muted-foreground">
            Optional tools for your projects and conversations.
          </p>
        </div>
        <Button
          variant="outline"
          disabled={busy}
          onClick={() => {
            void perform(
              () =>
                addonInvoke(state.paused ? "addon_resume" : "addon_pause_all"),
              state.paused ? undefined : "*",
            );
          }}
        >
          <Pause className="size-4" />
          {state.paused ? "Resume add-ons" : "Pause all add-ons"}
        </Button>
      </header>
      {(error || state.error) && (
        <p
          role="alert"
          className="rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-body"
        >
          {error || state.error}
        </p>
      )}
      {state.paused && (
        <p role="status" className="rounded-lg border bg-muted/40 p-3 text-body">
          All add-ons are paused. Your installed packages and settings are kept.
        </p>
      )}
      {!!state.warnings?.length && (
        <div role="status" className="space-y-2 rounded-lg border p-3 text-body">
          {state.warnings.map((warning) => (
            <p key={warning}>{warning}</p>
          ))}
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={() =>
              void perform(() => addonInvoke("addon_retry_cleanup"))
            }
          >
            Retry cleanup
          </Button>
        </div>
      )}
      {installation ? (
        <Configure
          key={installation.installationId}
          installation={installation}
          back={() => setSelected(null)}
          onError={setError}
        />
      ) : (
        <>
          <div className="flex flex-wrap items-center justify-between gap-3 border-b pb-3">
            <div className="flex gap-1" aria-label="Add-on manager pages">
              <Button
                size="sm"
                variant={tab === "installed" ? "secondary" : "ghost"}
                aria-pressed={tab === "installed"}
                onClick={() => setTab("installed")}
              >
                Installed{" "}
                <span className="text-muted-foreground">
                  {state.installed.length}
                </span>
              </Button>
              <Button
                size="sm"
                variant={tab === "browse" ? "secondary" : "ghost"}
                aria-pressed={tab === "browse"}
                onClick={() => setTab("browse")}
              >
                Browse
              </Button>
            </div>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="outline"
                disabled={busy}
                onClick={() => setLinkOpen(true)}
              >
                Install from link / ID
              </Button>
              <Button
                variant="outline"
                disabled={busy}
                onClick={() =>
                  void perform(async () => {
                    const path = await open({
                      multiple: false,
                      filters: [
                        {
                          name: "CodeMux feature plugin",
                          extensions: ["cmxaddon"],
                        },
                      ],
                    });
                    if (typeof path === "string") {
                      setReplace(false);
                      setEnable(true);
                      setReview(
                        await addonInvoke<AddonReview>("addon_import_review", {
                          path,
                        }),
                      );
                    }
                  })
                }
              >
                <PackagePlus className="size-4" />
                Import package
              </Button>
            </div>
          </div>
          {tab === "browse" ? (
            <AddonCatalog onReview={showReview} busy={busy} perform={perform} />
          ) : !state.loaded ? (
            <p role="status">Loading add-ons…</p>
          ) : !state.installed.length ? (
            <div className="rounded-xl border border-dashed px-6 py-12 text-center">
              <Puzzle className="mx-auto size-7 text-muted-foreground" />
              <h3 className="mt-4 font-medium">Make room for your workflow</h3>
              <p className="mx-auto mt-2 max-w-md text-body text-muted-foreground">
                Add-ons can add project panels, commands, and composer actions.
                Browse reviewed releases or import a package to review its
                access before enabling it.
              </p>
              <Button
                className="mt-5"
                variant="outline"
                onClick={() => setTab("browse")}
              >
                Browse add-ons
              </Button>
            </div>
          ) : (
            <div className="divide-y rounded-lg border">
              {state.installed.map((item) => (
                <article key={item.installationId} className="space-y-3 p-4">
                  <div className="flex items-start justify-between gap-3">
                    <div>
                      <h3 className="font-medium">
                        {item.manifest.name}{" "}
                        <span className="font-normal text-muted-foreground">
                          {item.manifest.version}
                        </span>
                      </h3>
                      <p className="mt-1 text-label text-muted-foreground">
                        {item.manifest.author.name} ·{" "}
                        {item.source.kind === "local"
                          ? "Local / unverified"
                          : "Catalog source"}{" "}
                        · {item.status.replace(/-/g, " ")}
                      </p>
                    </div>
                    {item.source.kind === "catalog" && (
                      <ShieldCheck className="size-4 text-muted-foreground" />
                    )}
                  </div>
                  <p className="text-body text-muted-foreground">
                    {item.manifest.description}
                  </p>
                  {item.failure && (
                    <p className="text-body text-destructive">{item.failure}</p>
                  )}
                  <div className="flex flex-wrap gap-2">
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => setSelected(item.manifest.id)}
                    >
                      Configure / Permissions
                    </Button>
                    {item.source.kind === "catalog" && (
                      <Button
                        size="sm"
                        variant="outline"
                        disabled={busy}
                        onClick={() =>
                          void perform(async () =>
                            showReview(
                              await addonInvoke<AddonReview>(
                                "addon_catalog_review",
                                { target: item.manifest.id },
                              ),
                            ),
                          )
                        }
                      >
                        Check for update
                      </Button>
                    )}
                    <Button
                      size="sm"
                      variant="outline"
                      disabled={busy}
                      onClick={() =>
                        void perform(
                          () =>
                            addonInvoke(
                              item.desiredEnabled &&
                                item.status !== "failed-disabled"
                                ? "addon_disable"
                                : "addon_enable",
                              { id: item.manifest.id },
                            ),
                          item.desiredEnabled ? item.manifest.id : undefined,
                        )
                      }
                    >
                      {item.status === "failed-disabled"
                        ? "Retry"
                        : item.desiredEnabled
                          ? "Disable"
                          : "Enable"}
                    </Button>
                    {item.previous && (
                      <Button
                        size="sm"
                        variant="outline"
                        onClick={() => setRollback(item)}
                      >
                        <RefreshCw className="size-3" />
                        Rollback
                      </Button>
                    )}
                    <Button
                      size="sm"
                      variant="ghost"
                      onClick={() => {
                        setKeep(false);
                        setRemove(item);
                      }}
                    >
                      Remove
                    </Button>
                  </div>
                </article>
              ))}
            </div>
          )}
        </>
      )}
      {!installation && (
        <section className="space-y-3 border-t pt-5">
          <div className="flex items-center justify-between gap-4">
            <div>
              <h3 className="text-body font-medium">Developer mode</h3>
              <p className="mt-1 text-label text-muted-foreground">
                Watch one package you select. Off by default each time CodeMux
                starts.
              </p>
            </div>
            <Switch
              aria-label="Developer mode"
              checked={state.developerMode ?? false}
              disabled={busy}
              onCheckedChange={(enabled) =>
                void perform(() =>
                  addonInvoke("addon_developer_mode", { enabled }),
                )
              }
            />
          </div>
          {state.developerMode && (
            <>
              <p className="text-label text-muted-foreground">
                Validated local builds reload with the access you accepted.
                Changed access needs a new review. CodeMux does not run package
                scripts or discover source folders.
              </p>
              <div className="flex flex-wrap gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={busy}
                  onClick={() =>
                    void perform(async () => {
                      const path = await open({
                        multiple: false,
                        filters: [
                          {
                            name: "Development feature plugin",
                            extensions: ["cmxaddon"],
                          },
                        ],
                      });
                      if (typeof path === "string")
                        showReview(
                          await addonInvoke<AddonReview>(
                            "addon_development_review",
                            { path },
                          ),
                        );
                    })
                  }
                >
                  Select development package
                </Button>
                {state.developmentPackage && (
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() =>
                      void perform(() =>
                        addonInvoke("addon_development_reload"),
                      )
                    }
                  >
                    Reload {state.developmentPackage}
                  </Button>
                )}
              </div>
            </>
          )}
        </section>
      )}
      <Dialog open={linkOpen} onOpenChange={setLinkOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Install from link or ID</DialogTitle>
            <DialogDescription>
              Paste a codemux.org add-on link or a catalog ID. You will review
              the exact package before installing.
            </DialogDescription>
          </DialogHeader>
          <form
            className="space-y-4"
            onSubmit={(e) => {
              e.preventDefault();
              void perform(async () => {
                showReview(
                  await addonInvoke<AddonReview>("addon_catalog_review", {
                    target: target.trim(),
                  }),
                );
                setLinkOpen(false);
              });
            }}
          >
            <Input
              aria-label="Add-on install link or ID"
              autoFocus
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              placeholder="codemux.project-brief"
            />
            <DialogFooter>
              <Button type="submit" disabled={busy || !target.trim()}>
                Find release
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
      <Dialog
        open={review !== null}
        onOpenChange={(value) => {
          if (!value && !busy && review) {
            void addonInvoke("addon_cancel_review", { token: review.token });
            setReview(null);
          }
        }}
      >
        <DialogContent className="max-h-[85vh] overflow-auto">
          <DialogHeader>
            <DialogTitle>Review {review?.manifest.name}</DialogTitle>
            <DialogDescription>
              {review?.manifest.version} ·{" "}
              {review?.source.kind === "local"
                ? "Local / unverified"
                : "Catalog source"}
            </DialogDescription>
          </DialogHeader>
          {review && (
            <>
              <p className="text-body">{review.manifest.description}</p>
              {review.development && (
                <p className="rounded-sm border p-3 text-body">
                  Development package. Once enabled, CodeMux watches this
                  selected file for validated local rebuilds. Permission changes
                  still need review.
                </p>
              )}
              <p className="text-label text-muted-foreground">
                Author: {review.manifest.author.name} · License:{" "}
                {review.manifest.license}
              </p>
              <Capabilities manifest={review.manifest} />
              <code className="break-all text-label">SHA-256 {review.digest}</code>
              {review.replacesSource && (
                <label className="flex items-start gap-2 text-body">
                  <input
                    type="checkbox"
                    checked={replace}
                    onChange={(e) => setReplace(e.target.checked)}
                  />
                  Replace the existing source with this package. Its previous
                  grants and private data will not carry over.
                </label>
              )}
              <label className="flex items-center gap-2 text-body">
                <input
                  type="checkbox"
                  checked={enable}
                  onChange={(e) => setEnable(e.target.checked)}
                />
                Enable after installation
              </label>
              {review.retainedData && (
                <label className="flex items-start gap-2 text-body">
                  <input
                    type="checkbox"
                    checked={restoreData}
                    onChange={(e) => setRestoreData(e.target.checked)}
                  />
                  Restore private data retained from version{" "}
                  {review.retainedData.version} of this same source. Credentials
                  are not restored.
                </label>
              )}
              <DialogFooter>
                <Button
                  disabled={busy || (review.replacesSource && !replace)}
                  onClick={() =>
                    void perform(async () => {
                      await addonInvoke("addon_accept_review", {
                        token: review.token,
                        enable,
                        replaceSource: replace,
                        restoreData,
                      });
                      setReview(null);
                    })
                  }
                >
                  Accept and install
                </Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>
      <Dialog
        open={remove !== null}
        onOpenChange={(value) => {
          if (!value && !busy) setRemove(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Remove {remove?.manifest.name}?</DialogTitle>
            <DialogDescription>
              The add-on stops immediately. Credentials are deleted even if you
              keep private data.
            </DialogDescription>
          </DialogHeader>
          <label className="flex items-center gap-2 text-body">
            <input
              type="checkbox"
              checked={keep}
              onChange={(e) => setKeep(e.target.checked)}
            />
            Keep data for reinstall from the same source
          </label>
          <DialogFooter>
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  const warnings = await addonInvoke<string[]>("addon_remove", {
                    id: remove!.manifest.id,
                    keepData: keep,
                  });
                  setRemove(null);
                  if (warnings.length) setError(warnings.join(" "));
                }, remove!.manifest.id)
              }
            >
              Remove add-on
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
      <Dialog
        open={rollback !== null}
        onOpenChange={(value) => {
          if (!value && !busy) setRollback(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>
              Restore {rollback?.previous?.manifest.version}?
            </DialogTitle>
            <DialogDescription>
              This restores the previous release and its private data snapshot.
              Private changes since the update will be lost. Completed external
              actions are not undone.
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button
              disabled={busy}
              onClick={() =>
                void perform(async () => {
                  await addonInvoke("addon_rollback", {
                    id: rollback!.manifest.id,
                  });
                  setRollback(null);
                })
              }
            >
              Restore previous release
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}
