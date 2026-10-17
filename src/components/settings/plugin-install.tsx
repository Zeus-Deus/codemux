/**
 * Installing plugins: the GitHub URL prompt, the install/update dialog, and
 * the catalog list. The dialog always shows what will be installed (name,
 * source, exact commit, label) before anything is written, and a new install
 * stays off unless the user picks "Install and turn on".
 */
import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Badge } from "@/components/ui/badge";
import { Spinner } from "@/components/ui/spinner";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { toast } from "@/lib/toast";
import {
  pluginsCatalog,
  pluginsInstall,
  pluginsPreviewInstall,
  type CatalogView,
  type InstallPreview,
  type InstallSource,
} from "@/contrib/ipc";
import { beginInstallationChange, usePluginsStore, type PluginTier } from "@/contrib/plugins-store";
import { SubsectionHeader } from "./settings-primitives";

export const FULL_ACCESS_WARNING =
  "This plugin runs with full access to Codemux, your projects and your files. Only install plugins you trust.";

const TIER_LABEL: Record<PluginTier, string> = { official: "Official", unverified: "Unverified", local: "Local" };

function shortSha(sha: string): string {
  return sha.slice(0, 12);
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

type Stage =
  | { kind: "closed" }
  | { kind: "url" }
  | { kind: "resolving"; source: InstallSource }
  | { kind: "review"; source: InstallSource; preview: InstallPreview }
  | { kind: "installing"; source: InstallSource; preview: InstallPreview; enable: boolean }
  | { kind: "failed"; source: InstallSource | null; message: string };

/** One dialog for every install path: URL entry → resolve → review → install. */
export function usePluginInstaller() {
  const [stage, setStage] = useState<Stage>({ kind: "closed" });

  const resolve = useCallback(async (source: InstallSource) => {
    setStage({ kind: "resolving", source });
    try {
      const preview = await pluginsPreviewInstall(source);
      setStage({ kind: "review", source, preview });
    } catch (error) {
      setStage({ kind: "failed", source, message: errorText(error) });
    }
  }, []);

  const install = useCallback(
    async (source: InstallSource, preview: InstallPreview, enable: boolean) => {
      setStage({ kind: "installing", source, preview, enable });
      try {
        // A fresh install is a new installation: nothing a removed copy with
        // this id chose (or was still saving) may decide whether it runs.
        const change = preview.replaces ? null : await beginInstallationChange(preview.id);
        try {
          await pluginsInstall(source, preview.sha, enable);
        } catch (error) {
          change?.rollback();
          throw error;
        }
        change?.commit();
        const { reloadPlugins } = await import("@/contrib/boot");
        await reloadPlugins();
        setStage({ kind: "closed" });
        toast.success(
          preview.replaces
            ? `${preview.name} updated to ${shortSha(preview.sha)}`
            : enable
              ? `${preview.name} installed and turned on`
              : `${preview.name} installed. It's off until you turn it on.`,
        );
      } catch (error) {
        setStage({ kind: "failed", source, message: errorText(error) });
      }
    },
    [],
  );

  const dialog = (
    <InstallDialog
      stage={stage}
      onClose={() => setStage({ kind: "closed" })}
      onResolve={resolve}
      onInstall={install}
    />
  );
  return {
    dialog,
    promptUrl: () => setStage({ kind: "url" }),
    installFrom: (source: InstallSource) => void resolve(source),
  };
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[6.5rem_1fr] gap-3 text-body-sm">
      <span className="text-muted-foreground">{label}</span>
      <span className="min-w-0 break-words text-foreground">{children}</span>
    </div>
  );
}

function InstallDialog({
  stage,
  onClose,
  onResolve,
  onInstall,
}: {
  stage: Stage;
  onClose: () => void;
  onResolve: (source: InstallSource) => void;
  onInstall: (source: InstallSource, preview: InstallPreview, enable: boolean) => void;
}) {
  const [url, setUrl] = useState("");
  const open = stage.kind !== "closed";
  const busy = stage.kind === "resolving" || stage.kind === "installing";
  const preview = stage.kind === "review" || stage.kind === "installing" ? stage.preview : null;
  const update = !!preview?.replaces;
  const tier: PluginTier = preview?.tier ?? "unverified";

  return (
    <Dialog open={open} onOpenChange={(next) => !next && !busy && onClose()}>
      <DialogContent className="sm:max-w-lg" data-plugin-install-dialog={stage.kind}>
        <DialogHeader>
          <DialogTitle>
            {stage.kind === "url"
              ? "Install from GitHub"
              : preview
                ? `${update ? "Update" : "Install"} ${preview.name}?`
                : stage.kind === "failed"
                  ? "Couldn’t install the plugin"
                  : "Looking up the plugin…"}
          </DialogTitle>
          {stage.kind === "url" && (
            <DialogDescription>
              A public GitHub repository with a plugin.js at its root or in codemux/. A /tree/&lt;branch&gt;/&lt;folder&gt;
              link works too.
            </DialogDescription>
          )}
        </DialogHeader>

        {stage.kind === "url" && (
          <form
            id="plugin-install-url"
            onSubmit={(e) => {
              e.preventDefault();
              if (url.trim()) onResolve({ kind: "git", url: url.trim() });
            }}
          >
            <Input
              autoFocus
              aria-label="GitHub repository URL"
              placeholder="https://github.com/owner/repo"
              value={url}
              onChange={(e) => setUrl(e.target.value)}
              spellCheck={false}
            />
          </form>
        )}

        {stage.kind === "resolving" && (
          <p className="flex items-center gap-2 text-body-sm text-muted-foreground">
            <Spinner /> Fetching the commit…
          </p>
        )}

        {preview && (
          <div className="space-y-2" data-plugin-install-review>
            <Field label="Name">
              {preview.name}
              {preview.version ? <span className="text-muted-foreground"> {preview.version}</span> : null}
            </Field>
            {preview.description && <Field label="Description">{preview.description}</Field>}
            <Field label="Source">
              {preview.source === "catalog" ? "Codemux catalog · " : ""}
              {preview.repo}
              {preview.subdir ? `/${preview.subdir}` : ""}
            </Field>
            <Field label="Commit">
              <code className="font-mono text-caption" data-plugin-install-sha={preview.sha}>
                {shortSha(preview.sha)}
              </code>
              {preview.replaces && (
                <span className="text-muted-foreground"> (installed: {shortSha(preview.replaces)})</span>
              )}
            </Field>
            <Field label="Label">
              <Badge variant={tier === "official" ? "secondary" : "outline"}>{TIER_LABEL[tier]}</Badge>
            </Field>
            {tier !== "official" && (
              <p className="rounded-md border border-hairline bg-surface-1 px-3 py-2 text-body-sm text-foreground" role="note">
                {FULL_ACCESS_WARNING}
              </p>
            )}
          </div>
        )}

        {stage.kind === "failed" && (
          <p className="break-words text-body-sm text-destructive" role="alert">
            {stage.message}
          </p>
        )}

        <DialogFooter>
          <Button variant="ghost" disabled={busy} onClick={onClose}>
            Cancel
          </Button>
          {stage.kind === "url" && (
            <Button type="submit" form="plugin-install-url" disabled={!url.trim()}>
              Continue
            </Button>
          )}
          {stage.kind === "failed" && stage.source && (
            <Button variant="outline" onClick={() => onResolve(stage.source!)}>
              Try again
            </Button>
          )}
          {preview && update && (
            <Button disabled={busy} onClick={() => stage.kind === "review" && onInstall(stage.source, preview, false)}>
              {stage.kind === "installing" ? <Spinner /> : null}
              Update
            </Button>
          )}
          {preview && !update && (
            <>
              <Button
                variant="outline"
                disabled={busy}
                onClick={() => stage.kind === "review" && onInstall(stage.source, preview, false)}
              >
                {stage.kind === "installing" && !stage.enable ? <Spinner /> : null}
                Install
              </Button>
              <Button
                disabled={busy}
                onClick={() => stage.kind === "review" && onInstall(stage.source, preview, true)}
              >
                {stage.kind === "installing" && stage.enable ? <Spinner /> : null}
                Install and turn on
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

/** Settings → Plugins → Browse: the catalog with labels and Install. */
export function PluginCatalog({ onInstall }: { onInstall: (source: InstallSource) => void }) {
  const [view, setView] = useState<CatalogView | null>(null);
  const [loading, setLoading] = useState(true);
  const records = usePluginsStore((s) => s.records);

  useEffect(() => {
    let live = true;
    pluginsCatalog()
      .then((v) => live && setView(v))
      .catch((error) => live && setView({ catalog: { schemaVersion: 1, revision: 0, plugins: [], blocked: [] }, fetchedAt: null, error: errorText(error) }))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
  }, []);

  const plugins = view?.catalog.plugins ?? [];
  return (
    <section className="mt-8" data-plugin-catalog>
      <SubsectionHeader title="Browse" description="Plugins listed in the Codemux catalog." />
      {loading ? (
        <p className="flex items-center gap-2 text-body-sm text-muted-foreground">
          <Spinner /> Loading the catalog…
        </p>
      ) : plugins.length === 0 ? (
        <p className="text-body-sm text-muted-foreground">
          {view?.error ? `The catalog is unavailable: ${view.error}` : "The catalog has no plugins yet."}
        </p>
      ) : (
        <div className="divide-y divide-hairline rounded-md border border-hairline">
          {plugins.map((p) => {
            const installed = !!records[p.id];
            return (
              <article key={p.id} className="flex items-start gap-3 px-4 py-3" data-catalog-row={p.id}>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2">
                    <h3 className="truncate font-medium text-foreground">{p.name}</h3>
                    <Badge variant={p.tier === "official" ? "secondary" : "outline"}>{TIER_LABEL[p.tier]}</Badge>
                  </div>
                  {p.description && <p className="mt-0.5 text-body-sm text-muted-foreground">{p.description}</p>}
                </div>
                <Button
                  size="sm"
                  variant="outline"
                  disabled={installed}
                  onClick={() => onInstall({ kind: "catalog", id: p.id })}
                >
                  {installed ? "Installed" : "Install"}
                </Button>
              </article>
            );
          })}
        </div>
      )}
    </section>
  );
}
