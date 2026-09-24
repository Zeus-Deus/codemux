import { useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { PackagePlus, Pause, Puzzle, RefreshCw, ShieldCheck } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Switch } from "@/components/ui/switch";
import {
  Dialog,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { AddonCatalog } from "@/components/addons/addon-catalog";
import { beginAddonRevocation, useAddonsStore } from "@/stores/addons-store";
import { addonInvoke, resubscribeAddons } from "@/lib/addons/bridge";
import { activeAddonWorkspace, refreshAddons } from "@/lib/addons/platform";
import {
  addonMessage,
  type AddonInstallation,
  type AddonReview,
  type AddonUpdateCheck,
} from "@/lib/addons/types";
import { AddonDetail } from "./addon-detail";
import { AddonDialogContent, ProblemAlert } from "./addon-parts";
import {
  TIER_LABELS,
  addonProblem,
  sourceIdentity,
  type AddonProblem,
} from "./addon-presentation";
import { AddonReviewDialog } from "./addon-review";

/** The outcome of the last "Check for update" on one release of a row. */
type UpdateCheck = { release: string } & (
  | { upToDate: true }
  | { problem: AddonProblem }
);
const releaseKey = (item: AddonInstallation) =>
  `${item.installationId}/${item.digest}`;
/** The catalog or the app forbids enabling these; Enable could never work. */
const UNAVAILABLE = ["blocked-disabled", "incompatible-disabled", "removing"];

function names(values: string[]) {
  return values.length < 2
    ? values.join("")
    : `${values.slice(0, -1).join(", ")} and ${values[values.length - 1]}`;
}

function InstalledRow({
  item,
  busy,
  check,
  onConfigure,
  onCheck,
  onEnable,
  onDisable,
  onRollback,
  onRemove,
}: {
  item: AddonInstallation;
  busy: boolean;
  check: UpdateCheck | undefined;
  onConfigure: () => void;
  onCheck: () => void;
  onEnable: () => void;
  onDisable: () => void;
  onRollback: () => void;
  onRemove: () => void;
}) {
  const current = check?.release === releaseKey(item) ? check : undefined;
  const reason =
    item.failure ??
    (item.compatibility && !item.compatibility.compatible
      ? item.compatibility.reason
      : null);
  return (
    <article className="space-y-3 p-4">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <h3 className="font-medium">
            {item.manifest.name}{" "}
            <span className="font-normal text-muted-foreground">
              {item.manifest.version}
            </span>
          </h3>
          <p className="mt-1 text-label text-muted-foreground">
            {item.source.kind === "local"
              ? `${item.manifest.author.name} · Local / unverified`
              : sourceIdentity(item.source, item.catalog)}
            {item.catalog?.tier && ` · ${TIER_LABELS[item.catalog.tier]}`}
            {item.catalog?.listed === false && " · No longer listed"} ·{" "}
            {item.status.replace(/-/g, " ")}
          </p>
        </div>
        <div className="flex shrink-0 flex-wrap items-center justify-end gap-2">
          {item.updateAvailable && (
            <Badge variant="secondary">
              Update {item.updateAvailable} available
            </Badge>
          )}
          {item.catalog?.tier === "official" && (
            <ShieldCheck
              className="size-4 text-muted-foreground"
              role="img"
              aria-label="Official"
            />
          )}
        </div>
      </div>
      <p className="text-body text-muted-foreground">
        {item.manifest.description}
      </p>
      {reason && <p className="text-body text-destructive">{reason}</p>}
      <div className="flex flex-wrap items-center gap-2">
        <Button size="sm" variant="outline" onClick={onConfigure}>
          Configure / Permissions
        </Button>
        {item.source.kind === "catalog" && (
          <Button size="sm" variant="outline" disabled={busy} onClick={onCheck}>
            Check for update
          </Button>
        )}
        {item.status === "failed-disabled" ? (
          <>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={onEnable}
            >
              Retry
            </Button>
            <Button
              size="sm"
              variant="outline"
              disabled={busy}
              onClick={onDisable}
            >
              Disable
            </Button>
          </>
        ) : item.status === "incompatible-disabled" && item.desiredEnabled ? (
          // It starts again by itself once CodeMux is compatible; let the
          // user turn it off before that.
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={onDisable}
          >
            Disable
          </Button>
        ) : UNAVAILABLE.includes(item.status) ? null : (
          <Button
            size="sm"
            variant="outline"
            disabled={busy}
            onClick={item.desiredEnabled ? onDisable : onEnable}
          >
            {item.desiredEnabled ? "Disable" : "Enable"}
          </Button>
        )}
        {item.previous && (
          <Button size="sm" variant="outline" onClick={onRollback}>
            <RefreshCw className="size-3" />
            Rollback
          </Button>
        )}
        <Button size="sm" variant="ghost" onClick={onRemove}>
          Remove
        </Button>
        {current && "upToDate" in current && (
          <span role="status" className="text-label text-muted-foreground">
            Up to date
          </span>
        )}
      </div>
      {current && "problem" in current && (
        <ProblemAlert problem={current.problem} />
      )}
    </article>
  );
}

export function AddonsSettings() {
  const state = useAddonsStore();
  const [selected, setSelected] = useState<string | null>(null);
  const [review, setReview] = useState<AddonReview | null>(null);
  // The token of a review whose accept failed. The host removes a review as
  // it accepts it, so the same review can never be accepted again.
  const [endedReview, setEndedReview] = useState<string | null>(null);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  // Failures of the open dialog's operation render inside that dialog; the
  // page behind a modal is hidden from assistive technology.
  const [dialogProblem, setDialogProblem] = useState<AddonProblem | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [remove, setRemove] = useState<AddonInstallation | null>(null);
  const [keep, setKeep] = useState(false);
  const [rollback, setRollback] = useState<AddonInstallation | null>(null);
  const [resetOpen, setResetOpen] = useState(false);
  const [checks, setChecks] = useState<Record<string, UpdateCheck>>({});
  const [tab, setTab] = useState<"installed" | "browse">("installed");
  const [target, setTarget] = useState("");
  const [linkOpen, setLinkOpen] = useState(false);
  const linkOpener = useRef<HTMLButtonElement>(null);
  // Where focus returns when the review closes, when not to the element
  // focused as it opened: a review found from a link replaces the link
  // dialog, and what was focused there is gone by then.
  const [reviewReturn, setReviewReturn] = useState<HTMLElement | null>(null);
  const showReview = (
    value: AddonReview,
    returnFocus: HTMLElement | null = null,
  ) => {
    setDialogProblem(null);
    setReviewReturn(returnFocus);
    setReview(value);
  };
  useEffect(() => {
    void refreshAddons();
  }, []);
  // A watched rebuild's review arrives on its own. It waits in the store until
  // the open review and any operation have finished, so it never replaces
  // the review under the user's pointer; the store keeps only the newest
  // one, and the host cancels the one it replaces.
  useEffect(() => {
    if (state.developmentReview && !review && !busy) {
      showReview(state.developmentReview);
      useAddonsStore.setState({ developmentReview: null });
    }
  }, [state.developmentReview, review, busy]);
  const perform = async (
    action: () => Promise<unknown>,
    options: { revoke?: string; dialog?: boolean } = {},
  ) => {
    const release = options.revoke
      ? beginAddonRevocation(options.revoke)
      : undefined;
    setBusy(true);
    // Each operation replaces the previous outcome, including one a dialog
    // operation leaves behind on the page (a registry reset or a removal).
    setError("");
    setNotice("");
    if (options.dialog) setDialogProblem(null);
    try {
      await action();
    } catch (cause) {
      if (options.dialog) setDialogProblem(addonProblem(cause));
      else setError(addonMessage(cause));
    } finally {
      await refreshAddons();
      release?.();
      setBusy(false);
    }
  };
  const openDialog = (show: () => void) => {
    setDialogProblem(null);
    show();
  };
  const setCheck = (id: string, value: UpdateCheck | null) =>
    setChecks((all) => {
      const next = { ...all };
      if (value) next[id] = value;
      else delete next[id];
      return next;
    });
  const checkForUpdate = (item: AddonInstallation) =>
    void perform(async () => {
      const id = item.manifest.id;
      const release = releaseKey(item);
      setCheck(id, null);
      try {
        const result = await addonInvoke<AddonUpdateCheck>(
          "addon_check_update",
          { id },
        );
        if (result.review && !result.upToDate) showReview(result.review);
        else setCheck(id, { release, upToDate: true });
      } catch (cause) {
        setCheck(id, { release, problem: addonProblem(cause) });
      }
    });
  const installation = state.installed.find((i) => i.manifest.id === selected);
  const interrupted = (state.interruptedActivations ?? []).map(
    (id) => state.installed.find((i) => i.manifest.id === id)?.manifest.name ?? id,
  );
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
        {/* An unreadable registry cannot be paused or resumed; the registry
            panel offers the reset that recovers it instead. */}
        {!state.registryError && (
          <Button
            variant="outline"
            disabled={busy}
            onClick={() => {
              void perform(
                async () => {
                  if (!state.paused) return addonInvoke("addon_pause_all");
                  // Resuming also retries a manager that failed to open, whose
                  // event stream then has to be opened again.
                  await addonInvoke("addon_resume");
                  await resubscribeAddons(activeAddonWorkspace);
                },
                { revoke: state.paused ? undefined : "*" },
              );
            }}
          >
            <Pause className="size-4" />
            {state.paused ? "Resume add-ons" : "Pause all add-ons"}
          </Button>
        )}
      </header>
      <ProblemAlert
        problem={
          // The registry panel below already shows the open failure.
          state.registryError ? error || null : error || state.error
        }
      />
      {notice && (
        <p role="status" className="rounded-lg border bg-muted/40 p-3 text-body">
          {notice}
        </p>
      )}
      {state.registryError && (
        <div
          role="alert"
          className="space-y-3 rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-body"
        >
          <p className="font-medium">The add-on registry could not be opened</p>
          <p>{state.error}</p>
          <p className="text-muted-foreground">
            Add-ons stay off until it is repaired. The rest of CodeMux is not
            affected. Restarting CodeMux tries to open it again; resetting
            starts with no add-ons and keeps the current files as a backup.
          </p>
          <Button
            size="sm"
            disabled={busy}
            onClick={() => openDialog(() => setResetOpen(true))}
          >
            Reset add-on registry
          </Button>
        </div>
      )}
      {state.paused &&
        !state.error &&
        (interrupted.length ? (
          <p role="status" className="rounded-lg border bg-muted/40 p-3 text-body">
            CodeMux closed while {names(interrupted)}{" "}
            {interrupted.length > 1 ? "were" : "was"} starting. All add-ons
            are paused so you can review{" "}
            {interrupted.length > 1 ? "them" : "it"} first; choose Resume
            add-ons when you are ready.
          </p>
        ) : (
          <p role="status" className="rounded-lg border bg-muted/40 p-3 text-body">
            All add-ons are paused. Your installed packages and settings are
            kept.
          </p>
        ))}
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
      {state.registryError ? null : installation ? (
        <AddonDetail
          key={`${installation.installationId}/${installation.digest}/${installation.dataGeneration}`}
          installation={installation}
          credentialStates={state.credentialStates?.[installation.manifest.id]}
          back={() => setSelected(null)}
          onError={setError}
          onRollback={(item) => openDialog(() => setRollback(item))}
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
                ref={linkOpener}
                variant="outline"
                disabled={busy}
                onClick={() => openDialog(() => setLinkOpen(true))}
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
                    if (typeof path === "string")
                      showReview(
                        await addonInvoke<AddonReview>("addon_import_review", {
                          path,
                        }),
                      );
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
                <InstalledRow
                  key={item.installationId}
                  item={item}
                  busy={busy}
                  check={checks[item.manifest.id]}
                  onConfigure={() => setSelected(item.manifest.id)}
                  onCheck={() => checkForUpdate(item)}
                  onEnable={() =>
                    void perform(() =>
                      addonInvoke("addon_enable", { id: item.manifest.id }),
                    )
                  }
                  onDisable={() =>
                    void perform(
                      () =>
                        addonInvoke("addon_disable", { id: item.manifest.id }),
                      { revoke: item.manifest.id },
                    )
                  }
                  onRollback={() => openDialog(() => setRollback(item))}
                  onRemove={() =>
                    openDialog(() => {
                      setKeep(false);
                      setRemove(item);
                    })
                  }
                />
              ))}
            </div>
          )}
        </>
      )}
      {!installation && !state.registryError && (
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
      <Dialog
        open={linkOpen}
        onOpenChange={(value) => {
          setLinkOpen(value);
          if (!value) setDialogProblem(null);
        }}
      >
        <AddonDialogContent>
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
              void perform(
                async () => {
                  const found = await addonInvoke<AddonReview>(
                    "addon_catalog_review",
                    { target: target.trim() },
                  );
                  setLinkOpen(false);
                  showReview(found, linkOpener.current);
                },
                { dialog: true },
              );
            }}
          >
            <Input
              aria-label="Add-on install link or ID"
              value={target}
              onChange={(e) => setTarget(e.target.value)}
              placeholder="codemux.project-brief"
            />
            <ProblemAlert problem={dialogProblem} />
            <DialogFooter>
              <Button type="submit" disabled={busy || !target.trim()}>
                Find release
              </Button>
            </DialogFooter>
          </form>
        </AddonDialogContent>
      </Dialog>
      <AddonReviewDialog
        review={review}
        busy={busy}
        paused={state.paused}
        problem={dialogProblem}
        ended={review !== null && review.token === endedReview}
        returnFocus={reviewReturn}
        onDismiss={() => {
          if (review)
            void addonInvoke("addon_cancel_review", {
              token: review.token,
            }).catch(() => {});
          setReview(null);
          setDialogProblem(null);
        }}
        onAccept={(choice) => {
          if (!review) return;
          void perform(
            async () => {
              try {
                await addonInvoke("addon_accept_review", {
                  token: review.token,
                  ...choice,
                });
              } catch (cause) {
                setEndedReview(review.token);
                throw cause;
              }
              setReview(null);
            },
            { dialog: true },
          );
        }}
      />
      <Dialog
        open={remove !== null}
        onOpenChange={(value) => {
          if (!value && !busy) setRemove(null);
        }}
      >
        <AddonDialogContent>
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
          <ProblemAlert problem={dialogProblem} />
          <DialogFooter>
            <Button
              variant="destructive"
              disabled={busy}
              onClick={() =>
                void perform(
                  async () => {
                    const warnings = await addonInvoke<string[]>(
                      "addon_remove",
                      { id: remove!.manifest.id, keepData: keep },
                    );
                    setRemove(null);
                    if (selected === remove!.manifest.id) setSelected(null);
                    if (warnings?.length) setError(warnings.join(" "));
                  },
                  { revoke: remove!.manifest.id, dialog: true },
                )
              }
            >
              Remove add-on
            </Button>
          </DialogFooter>
        </AddonDialogContent>
      </Dialog>
      <Dialog
        open={rollback !== null}
        onOpenChange={(value) => {
          if (!value && !busy) setRollback(null);
        }}
      >
        <AddonDialogContent>
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
          <ProblemAlert problem={dialogProblem} />
          <DialogFooter>
            <Button
              disabled={busy}
              onClick={() =>
                void perform(
                  async () => {
                    await addonInvoke("addon_rollback", {
                      id: rollback!.manifest.id,
                    });
                    setRollback(null);
                  },
                  { dialog: true },
                )
              }
            >
              Restore previous release
            </Button>
          </DialogFooter>
        </AddonDialogContent>
      </Dialog>
      <Dialog
        open={resetOpen}
        onOpenChange={(value) => {
          if (!value && !busy) setResetOpen(false);
        }}
      >
        <AddonDialogContent>
          <DialogHeader>
            <DialogTitle>Reset the add-on registry?</DialogTitle>
            <DialogDescription>
              CodeMux moves the current add-on folder aside as a backup and
              starts with no add-ons. Nothing is deleted; install the add-ons
              you use again afterwards.
            </DialogDescription>
          </DialogHeader>
          {state.registryError && (
            <code className="break-all text-label text-muted-foreground">
              {state.registryError.path}
            </code>
          )}
          <ProblemAlert problem={dialogProblem} />
          <DialogFooter>
            <Button
              disabled={busy}
              onClick={() =>
                void perform(
                  async () => {
                    const backup = await addonInvoke<string>(
                      "addon_registry_reset",
                    );
                    setResetOpen(false);
                    setNotice(
                      `The add-on registry was reset. The previous files were moved to ${backup}.`,
                    );
                    // The first subscription failed with the registry.
                    await resubscribeAddons(activeAddonWorkspace).catch(
                      (cause) => setError(addonMessage(cause)),
                    );
                  },
                  { dialog: true },
                )
              }
            >
              Reset registry
            </Button>
          </DialogFooter>
        </AddonDialogContent>
      </Dialog>
    </div>
  );
}
