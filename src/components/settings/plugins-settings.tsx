import { useMemo, useState, useSyncExternalStore } from "react";
import { FolderOpen, Github, MoreHorizontal, RefreshCw, Settings2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { reportFailure } from "@/lib/toast";
import { pluginsRemove, pluginsRevealRoot, type InstallSource } from "@/contrib/ipc";
import { useConfirm } from "@/components/ui/confirm-dialog";
import { useUIStore } from "@/stores/ui-store";
import { PluginCatalog, usePluginInstaller } from "./plugin-install";
import {
  setPluginEnabled,
  beginInstallationChange,
  usePluginsStore,
  type PluginRecord,
  type PluginTier,
} from "@/contrib/plugins-store";
import { SectionHeader, SubsectionHeader } from "./settings-primitives";
import { Spinner } from "@/components/ui/spinner";
import { useContributions } from "@/contrib/react/use-contributions";
import { ContribBoundary, ContribRender } from "@/contrib/react/boundary";
import { SETTINGS_PAGES_AREA } from "@/sdk/areas";
import type { PluginSettingsPage, PluginSetupStatus } from "@/contrib/plugin";
import { cn } from "@/lib/utils";

const TIER_LABEL: Record<PluginTier, string> = {
  official: "Official",
  unverified: "Unverified",
  local: "Local",
};

function TierBadge({ tier }: { tier: PluginTier }) {
  return (
    <Badge variant={tier === "official" ? "secondary" : "outline"} data-plugin-tier={tier}>
      {TIER_LABEL[tier]}
    </Badge>
  );
}

function statusText(record: PluginRecord): string | null {
  if (record.status === "loading") return "Loading…";
  return null;
}

/** Settings pages a plugin registered, keyed by plugin id. */
export function usePluginSettingsPages() {
  const pages = useContributions(SETTINGS_PAGES_AREA);
  return useMemo(() => {
    const byPlugin = new Map<string, typeof pages>();
    for (const page of pages) {
      const pluginId = (page.data as { pluginId?: string } | undefined)?.pluginId;
      if (!pluginId) continue;
      byPlugin.set(pluginId, [...(byPlugin.get(pluginId) ?? []), page]);
    }
    return byPlugin;
  }, [pages]);
}

type SetupSource = NonNullable<PluginSettingsPage["status"]>;
const NO_SETUP: SetupSource = { get: () => null, subscribe: () => () => {} };

/** The first setup status any of the plugin's settings pages reports. */
function setupSourceOf(pages: readonly { data?: unknown }[]): SetupSource {
  for (const page of pages) {
    const status = (page.data as { status?: SetupSource } | undefined)?.status;
    if (status) return status;
  }
  return NO_SETUP;
}

function SetupStatus({ source }: { source: SetupSource }) {
  const status: PluginSetupStatus | null = useSyncExternalStore(source.subscribe, source.get, source.get);
  if (!status) return null;
  return (
    <p className="mt-1 flex items-center gap-1.5 text-body-sm text-muted-foreground" data-plugin-setup={status.tone}>
      <span
        aria-hidden
        className={cn(
          "size-1.5 shrink-0 rounded-full",
          status.tone === "ready" ? "bg-status-open" : status.tone === "attention" ? "bg-warning" : "bg-muted-foreground",
        )}
      />
      {status.label}
    </p>
  );
}

/** Settings → <plugin>: every settings page the plugin registered. */
export function PluginSettingsPageView({ pluginId }: { pluginId: string }) {
  const record = usePluginsStore((s) => s.records[pluginId]);
  const pages = usePluginSettingsPages().get(pluginId) ?? [];
  return (
    <div data-plugin-settings={pluginId}>
      <SectionHeader title={record?.name ?? pluginId} description={record?.description} />
      <div className="space-y-8">
        {pages.map((page) => (
          <section key={page.id}>
            {pages.length > 1 && page.title && <SubsectionHeader title={page.title} />}
            {page.render && (
              <ContribBoundary id={page.id} variant="block">
                <ContribRender render={page.render} />
              </ContribBoundary>
            )}
          </section>
        ))}
      </div>
    </div>
  );
}

/** The plugins-folder name behind a row. A shadowed disk copy's row id
 *  carries a suffix; error rows (blocked, bad name) keep the folder name. */
function folderIdOf(record: PluginRecord): string {
  return record.id.replace(/:disk-shadowed$/, "");
}

interface RowActions {
  openSettings: (id: string) => void;
  update: (source: InstallSource) => void;
  remove: (record: PluginRecord) => void;
  hasSettings: boolean;
  setup: SetupSource;
}

function PluginRow({ record, actions }: { record: PluginRecord; actions: RowActions }) {
  const decisions = usePluginsStore((s) => s.decisions);
  const on = record.id in decisions ? decisions[record.id] : (record.defaultEnabled ?? false);
  const paused =
    usePluginsStore((s) => s.disabledByEnv) ||
    (usePluginsStore((s) => s.paused) && record.kind !== "bundled");
  const status = statusText(record);
  const saveError = usePluginsStore((s) => s.unsavedDecisions[record.id] ?? null);
  const changing = usePluginsStore((s) => !!s.changing[record.id]);
  return (
    <article
      className="flex items-start gap-3 px-4 py-3"
      data-plugin-row={record.id}
      data-plugin-status={record.status}
    >
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <h3 className="truncate font-medium text-foreground">{record.name}</h3>
          <TierBadge tier={record.tier} />
          {record.version && (
            <span className="text-caption text-muted-foreground">{record.version}</span>
          )}
        </div>
        {record.description && (
          <p className="mt-0.5 text-body-sm text-muted-foreground">{record.description}</p>
        )}
        {status && <p className="mt-0.5 text-body-sm text-muted-foreground">{status}</p>}
        {on && !paused && !record.inert && record.status !== "error" && <SetupStatus source={actions.setup} />}
        {record.status === "error" && record.error && (
          <p className="mt-1 break-words text-body-sm text-destructive" role="alert">
            {record.error}
          </p>
        )}
        {saveError && (
          <p className="mt-1 break-words text-body-sm text-destructive" role="alert" data-plugin-save-error>
            Couldn’t save this choice: {saveError}. It applies until Codemux restarts.
          </p>
        )}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        {actions.hasSettings && !record.inert && (
          <Button size="sm" variant="ghost" onClick={() => actions.openSettings(record.id)}>
            <Settings2 />
            Settings
          </Button>
        )}
        {record.file && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button size="icon-sm" variant="ghost" aria-label={`${record.name} actions`}>
                <MoreHorizontal />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {record.file && (
                <DropdownMenuItem
                  onSelect={() =>
                    void revealItemInDir(record.file!).catch(reportFailure("Couldn’t reveal plugin"))
                  }
                >
                  Reveal folder
                </DropdownMenuItem>
              )}
              {record.origin && folderIdOf(record) === record.id && (
                <DropdownMenuItem onSelect={() => actions.update({ kind: "update", id: record.id })}>
                  Update…
                </DropdownMenuItem>
              )}
              {record.kind !== "bundled" && record.file && (
                <>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem variant="destructive" onSelect={() => actions.remove(record)}>
                    Remove…
                  </DropdownMenuItem>
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
        {!record.inert && (
          <Switch
            checked={on && !paused && !changing}
            disabled={paused || changing}
            aria-label={`${on ? "Turn off" : "Turn on"} ${record.name}`}
            onCheckedChange={(next) =>
              void setPluginEnabled(record.id, next).catch(
                reportFailure(`Couldn’t ${next ? "turn on" : "turn off"} ${record.name}`),
              )
            }
          />
        )}
      </div>
    </article>
  );
}

export function PluginsSettings({ onOpenSettings }: { onOpenSettings?: (pluginId: string) => void } = {}) {
  const records = usePluginsStore((s) => s.records);
  const installer = usePluginInstaller();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const settingsPages = usePluginSettingsPages();
  const remove = async (record: PluginRecord) => {
    const ok = await confirm({
      title: `Remove ${record.name}?`,
      description:
        record.tier === "local"
          ? "This deletes the plugin’s folder from the plugins folder. A linked folder is only unlinked."
          : "This deletes the installed copy. You can install it again later.",
      confirmLabel: "Remove plugin",
      destructive: true,
    });
    if (!ok) return;
    try {
      const change = await beginInstallationChange(folderIdOf(record));
      try {
        await pluginsRemove(folderIdOf(record));
      } catch (error) {
        change.rollback();
        // The folder stays: bring the live state back in line with its switch.
        const { reloadPlugins } = await import("@/contrib/boot");
        void reloadPlugins();
        throw error;
      }
      change.commit();
      const prefix = `plugin:${folderIdOf(record)}:`;
      useUIStore.getState().forgetRightPanelPanes((pane) => pane.startsWith(prefix));
      const { reloadPlugins } = await import("@/contrib/boot");
      await reloadPlugins();
    } catch (error) {
      reportFailure(`Couldn’t remove ${record.name}`)(error);
    }
  };
  const rowActions = (record: PluginRecord): RowActions => ({
    openSettings: (id) => onOpenSettings?.(id),
    update: installer.installFrom,
    remove: (r) => void remove(r),
    hasSettings: settingsPages.has(record.id) && !!onOpenSettings,
    setup: setupSourceOf(settingsPages.get(record.id) ?? []),
  });
  const disabledByEnv = usePluginsStore((s) => s.disabledByEnv);
  const paused = usePluginsStore((s) => s.paused);
  const pausedReason = usePluginsStore((s) => s.pausedReason);
  const [reloading, setReloading] = useState(false);
  const rows = useMemo(
    () =>
      Object.values(records).sort(
        (a, b) =>
          Number(a.kind !== "bundled") - Number(b.kind !== "bundled") ||
          a.name.localeCompare(b.name),
      ),
    [records],
  );

  const reload = async () => {
    setReloading(true);
    try {
      const { reloadPlugins } = await import("@/contrib/boot");
      await reloadPlugins();
    } catch (error) {
      reportFailure("Couldn’t reload plugins")(error);
    } finally {
      setReloading(false);
    }
  };

  return (
    <div>
      <SectionHeader
        title="Plugins"
        description="Plugins add features to Codemux. They run with full access to Codemux, your projects and your files. Each one can be turned on or off; off means none of its code runs."
        className="mb-4"
      />
      <div className="mb-7 flex flex-wrap items-center gap-2" data-plugins-actions>
        <Button size="sm" variant="outline" disabled={disabledByEnv} onClick={installer.promptUrl}>
          <Github />
          Install from GitHub URL…
        </Button>
        <Button
          size="sm"
          variant="outline"
          onClick={() => void pluginsRevealRoot().catch(reportFailure("Couldn’t open the plugins folder"))}
        >
          <FolderOpen />
          Open plugins folder
        </Button>
        <Button size="sm" variant="outline" disabled={reloading || disabledByEnv} onClick={() => void reload()}>
          {reloading ? <Spinner /> : <RefreshCw />}
          Reload plugins
        </Button>
      </div>
      {disabledByEnv && (
        <p className="mb-4 rounded-md border border-hairline bg-surface-1 px-4 py-3 text-body-sm text-muted-foreground" role="status">
          Plugins disabled by environment (CODEMUX_DISABLE_PLUGINS).
        </p>
      )}
      {paused && !disabledByEnv && (
        <div
          className="mb-4 flex items-center justify-between gap-4 rounded-md border border-hairline bg-surface-1 px-4 py-3"
          role="status"
          data-plugins-safe-mode
        >
          <p className="text-body-sm text-muted-foreground">
            {pausedReason ?? "Plugins you installed are paused."}
          </p>
          <Button
            size="sm"
            variant="outline"
            onClick={() =>
              void import("@/contrib/boot")
                .then(({ resumePlugins }) => resumePlugins())
                .catch(reportFailure("Couldn’t resume plugins"))
            }
          >
            Resume plugins
          </Button>
        </div>
      )}
      <SubsectionHeader title="Installed" />
      {rows.length === 0 ? (
        <p className="text-body-sm text-muted-foreground">
          No plugins yet. Drop a folder with a plugin.js into the plugins folder and it appears here as Local.
        </p>
      ) : (
        <div className="divide-y divide-hairline rounded-md border border-hairline">
          {rows.map((record) => (
            <PluginRow key={record.id} record={record} actions={rowActions(record)} />
          ))}
        </div>
      )}
      {!disabledByEnv && <PluginCatalog onInstall={installer.installFrom} />}
      {installer.dialog}
      {confirmDialog}
    </div>
  );
}
