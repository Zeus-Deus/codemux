import { useEffect, useMemo, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { ArrowUpRight, Check, CircleAlert, Laptop, Monitor, RefreshCw } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";

import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { useHosts, useHostsStore } from "@/stores/hosts-store";
import { useLocalDeviceName } from "@/stores/local-device-store";
import type { ProviderUpdateReport } from "@/stores/provider-update-store";
import { useUpdateStatusStore } from "@/stores/update-status-store";
import {
  LOCAL,
  PROVIDER_LABELS,
  buildPlan,
  helperIsOutdated,
  hostKey,
  itemKey,
  useUpdateAll,
  type ItemId,
  type ItemState,
  type MachineKey,
  type MachineState,
  type UpdatePlan,
} from "@/stores/update-all-store";
import { SectionHeader, SettingsCard, SubsectionHeader } from "./settings-primitives";

const RELEASES_URL = "https://github.com/Zeus-Deus/codemux/releases";

const GUIDES: Partial<Record<string, string>> = {
  claude: "https://code.claude.com/docs/en/setup",
  codex: "https://developers.openai.com/codex/cli/",
  cursor: "https://cursor.com/docs/cli/installation",
  grok: "https://www.npmjs.com/package/@xai-official/grok",
  hermes: "https://hermes-agent.nousresearch.com/docs/getting-started/updating",
  opencode: "https://opencode.ai/docs/cli/",
};

/**
 * Settings → Updates. Codemux and every coding agent, on this computer and
 * on each device, with one button that updates everything through the
 * installer that owns each tool. The app updates last: installing it
 * restarts Codemux.
 */
export function UpdatesSection() {
  const remote = isRemoteClient();
  const hosts = useHosts();
  const localName = useLocalDeviceName();
  const machines = useUpdateAll((s) => s.machines);
  const running = useUpdateAll((s) => s.running);
  const summary = useUpdateAll((s) => s.summary);
  const refreshAll = useUpdateAll((s) => s.refreshAll);
  const runPlan = useUpdateAll((s) => s.runPlan);
  const app = useAppUpdate();
  const [confirming, setConfirming] = useState<UpdatePlan | null>(null);

  const hostIds = useMemo(() => hosts.map((h) => h.id), [hosts]);
  useEffect(() => {
    void useHostsStore.getState().init();
  }, []);
  useEffect(() => {
    void refreshAll(hostIds);
  }, [hostIds, refreshAll]);

  const names = useMemo(() => {
    const map: Record<string, string> = { [LOCAL]: localName ?? "This computer" };
    for (const h of hosts) map[hostKey(h.id)] = h.name;
    return map;
  }, [hosts, localName]);

  const known = Object.fromEntries(
    Object.entries(machines).filter(([key]) => key === LOCAL || hostIds.includes(Number(key.slice(5)))),
  ) as Record<string, MachineState>;
  const plan = buildPlan(known, names);
  if (app.newer && !app.available) {
    plan.skipped.push({ machine: LOCAL, label: "Codemux", reason: "Installed by a package manager. Update it there." });
  }
  const loading = Object.values(known).some((m) => m.loading) || Object.keys(known).length === 0;
  const appPending = app.available;
  const total = plan.items.length + (appPending ? 1 : 0);

  const start = async (approved: UpdatePlan) => {
    setConfirming(null);
    const result = approved.items.length ? await runPlan(approved) : { updated: 0, failed: [] };
    const label = (n: number) => `${n} update${n === 1 ? "" : "s"}`;
    if (result.failed.length) {
      toast.warning(`${result.failed.length} of ${approved.items.length} updates failed`, {
        description: result.failed.map((f) => `${names[f.machine] ?? f.machine} · ${f.label}: ${f.reason}`).join("\n"),
      });
    } else if (result.updated) {
      toast.success(`${label(result.updated)} installed`, {
        description: "Running chats weren't interrupted. Restart Codemux when it suits you so every agent uses its new version.",
      });
    }
    // Last, because it restarts the app.
    if (appPending) app.advance();
  };

  return (
    <div>
      <SectionHeader
        title="Updates"
        description="Codemux and your coding agents, on this computer and your devices. Each one updates the way it was installed, including mise on Omarchy, so nothing ends up with a second copy."
        action={
          <>
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground"
              disabled={loading || running}
              onClick={() => void refreshAll(hostIds, true)}
              aria-label="Check for updates"
            >
              {loading ? <Spinner className="size-3.5" aria-hidden /> : <RefreshCw className="size-3.5" />}
              Check
            </Button>
            <Button
              size="sm"
              disabled={running || loading || total === 0}
              onClick={() => setConfirming(plan)}
            >
              {running && <Spinner className="size-3.5" aria-hidden />}
              {running ? "Updating…" : total ? `Update all (${total})` : "Up to date"}
            </Button>
          </>
        }
      />

      {summary && !running && (
        <p role="status" className="-mt-3 mb-6 text-body-sm text-muted-foreground">
          {summary.failed.length
            ? `${summary.updated} updated, ${summary.failed.length} failed. Failed items show why below.`
            : `${summary.updated} updated. Running chats weren't interrupted; restart Codemux when it suits you to use the new versions.`}
        </p>
      )}

      <SubsectionHeader title={names[LOCAL]} />
      <SettingsCard className="p-0">
        <ul className="divide-y divide-border/40">
          <AppRow app={app} remote={remote} />
          <ProviderRows machine={LOCAL} state={known[LOCAL]} />
        </ul>
      </SettingsCard>

      {hosts.map((host) => {
        const key = hostKey(host.id);
        const state = known[key];
        return (
          <div key={host.id} className="mt-8">
            <SubsectionHeader title={host.name} />
            <SettingsCard className="p-0">
              <ul className="divide-y divide-border/40">
                <HelperRow machine={key} state={state} />
                {state?.helperVersion && <ProviderRows machine={key} state={state} />}
              </ul>
            </SettingsCard>
          </div>
        );
      })}

      <AlertDialog open={confirming !== null} onOpenChange={(open) => !open && setConfirming(null)}>
        <AlertDialogContent onEscapeKeyDown={(event) => event.stopPropagation()}>
          <AlertDialogHeader>
            <AlertDialogTitle>Update {total === 1 ? "1 item" : `${total} items`}?</AlertDialogTitle>
            <AlertDialogDescription asChild>
              <div className="space-y-3 text-body-sm">
                <ConfirmList plan={confirming} names={names} app={appPending ? app.version : null} />
                <p>Running chats aren't interrupted. Each agent updates the way it was installed, one at a time per machine.</p>
              </div>
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => confirming && void start(confirming)}>Update all</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function ConfirmList({ plan, names, app }: { plan: UpdatePlan | null; names: Record<string, string>; app: string | null }) {
  if (!plan) return null;
  const grouped = new Map<string, string[]>();
  for (const item of plan.items) {
    const name = names[item.machine] ?? item.machine;
    grouped.set(name, [...(grouped.get(name) ?? []), item.label]);
  }
  return (
    <>
      <ul className="space-y-1">
        {[...grouped.entries()].map(([name, labels]) => (
          <li key={name}>
            <span className="font-medium text-foreground">{name}</span>: {labels.join(", ")}
          </li>
        ))}
        {app && (
          <li>
            <span className="font-medium text-foreground">Codemux {app}</span>: last, then Codemux restarts
          </li>
        )}
      </ul>
      {plan.skipped.length > 0 && (
        <div>
          <p className="font-medium text-foreground">Skipped</p>
          <ul className="mt-1 space-y-1">
            {plan.skipped.map((s) => (
              <li key={`${s.machine}/${s.label}`}>
                {names[s.machine] ?? s.machine} · {s.label}: {s.reason}
              </li>
            ))}
          </ul>
        </div>
      )}
    </>
  );
}

function Row({
  icon,
  title,
  detail,
  tone = "muted",
  children,
}: {
  icon: React.ReactNode;
  title: string;
  detail: React.ReactNode;
  tone?: "muted" | "available" | "error" | "done";
  children?: React.ReactNode;
}) {
  return (
    <li className="flex items-center gap-3 px-4 py-3" data-settings-row>
      <div className="flex size-7 shrink-0 items-center justify-center rounded-md border border-border bg-muted/40">{icon}</div>
      <div className="min-w-0 flex-1">
        <p className="truncate text-body font-medium text-foreground">{title}</p>
        <p
          title={tone === "error" && typeof detail === "string" ? detail : undefined}
          className={cn(
            "text-label",
            tone === "error" ? "line-clamp-2 break-words" : "truncate",
            tone === "available" && "text-foreground",
            tone === "error" && "text-destructive",
            tone === "done" && "text-success",
            tone === "muted" && "text-muted-foreground/80",
          )}
        >
          {detail}
        </p>
      </div>
      {children && <div className="flex shrink-0 items-center gap-2">{children}</div>}
    </li>
  );
}

function ProgressBadge({ item }: { item: ItemState | undefined }) {
  if (!item) return null;
  if (item.phase === "queued") return <span className="text-label text-muted-foreground">Waiting</span>;
  if (item.phase === "updating")
    return (
      <span className="inline-flex items-center gap-1.5 text-label text-muted-foreground">
        <Spinner className="size-3" aria-hidden />
        Updating
      </span>
    );
  if (item.phase === "updated")
    return (
      <span className="inline-flex items-center gap-1 text-label text-success">
        <Check className="size-3.5" /> Updated
      </span>
    );
  return null;
}

function ProviderRows({ machine, state }: { machine: MachineKey; state: MachineState | undefined }) {
  const items = useUpdateAll((s) => s.items);
  const updateOne = useUpdateAll((s) => s.updateOne);
  const running = useUpdateAll((s) => s.running);
  if (!state || (state.loading && state.providers.length === 0)) {
    return (
      <li className="flex items-center gap-2 px-4 py-3 text-body-sm text-muted-foreground">
        <Spinner className="size-3.5" aria-hidden /> Checking coding agents…
      </li>
    );
  }
  if (state.providers.length === 0) {
    return (
      <li className="px-4 py-3 text-body-sm text-muted-foreground/80">
        {state.error && machine === LOCAL ? state.error : "No coding agents installed."}
      </li>
    );
  }
  return (
    <>
      {state.providers.map((report) => (
        <ProviderRow
          key={report.provider}
          report={report}
          item={items[itemKey(machine, report.provider)]}
          busy={running}
          onUpdate={() => void updateOne(machine, report.provider)}
        />
      ))}
    </>
  );
}

function providerDetail(report: ProviderUpdateReport, item: ItemState | undefined): { text: string; tone: "muted" | "available" | "error" | "done" } {
  if (item?.phase === "failed") return { text: item.message ?? "Update failed", tone: "error" };
  const version = report.installed_version ?? "Unknown version";
  const how = report.manager ? ` · ${report.manager}` : "";
  if (item?.phase === "updated") return { text: `${version}${how}`, tone: "done" };
  if (!report.installed_version && report.message) return { text: report.message, tone: "muted" };
  // Hermes reports its configured channel rather than a version number.
  const latest = report.latest_version && /\d/.test(report.latest_version) ? report.latest_version : null;
  if (report.available) {
    return { text: latest ? `${version} → ${latest}${how}` : `${version} · update available${how}`, tone: "available" };
  }
  return { text: `${version}${how} · up to date`, tone: "muted" };
}

function ProviderRow({
  report,
  item,
  busy,
  onUpdate,
}: {
  report: ProviderUpdateReport;
  item: ItemState | undefined;
  busy: boolean;
  onUpdate: () => void;
}) {
  const label = PROVIDER_LABELS[report.provider];
  const detail = providerDetail(report, item);
  const inFlight = item?.phase === "updating" || item?.phase === "queued";
  return (
    <Row icon={<ProviderLogo provider={report.provider} className="size-4" />} title={label} detail={detail.text} tone={detail.tone}>
      {inFlight || item?.phase === "updated" ? (
        <ProgressBadge item={item} />
      ) : report.available && report.can_update ? (
        <Button variant="outline" size="xs" disabled={busy} onClick={onUpdate}>
          {item?.phase === "failed" ? "Try again" : "Update"}
        </Button>
      ) : report.available ? (
        <Button
          variant="ghost"
          size="xs"
          className="text-muted-foreground"
          title={report.message ?? undefined}
          onClick={() =>
            void openUrl(report.manager === "Omarchy system package" ? "https://omarchy.org/" : GUIDES[report.provider] ?? "").catch(console.error)
          }
        >
          Update guide
          <ArrowUpRight className="size-3" />
        </Button>
      ) : null}
    </Row>
  );
}

function HelperRow({ machine, state }: { machine: MachineKey; state: MachineState | undefined }) {
  const item = useUpdateAll((s) => s.items[itemKey(machine, "helper" as ItemId)]);
  const updateOne = useUpdateAll((s) => s.updateOne);
  const running = useUpdateAll((s) => s.running);
  const icon = <Laptop className="size-4 text-muted-foreground" aria-hidden />;
  if (!state || (state.loading && !state.checkedAt)) {
    return <Row icon={icon} title="Codemux" detail="Checking…" />;
  }
  if (!state.helperVersion) {
    return (
      <Row
        icon={<CircleAlert className="size-4 text-muted-foreground" aria-hidden />}
        title="Codemux"
        detail={state.error ?? "Couldn't reach this device"}
        tone="muted"
      />
    );
  }
  const outdated = helperIsOutdated(state);
  const detail =
    item?.phase === "failed"
      ? { text: item.message ?? "Update failed", tone: "error" as const }
      : item?.phase === "updated"
        ? { text: `v${state.helperVersion}`, tone: "done" as const }
        : outdated
          ? { text: `v${state.helperVersion} → v${state.appVersion}`, tone: "available" as const }
          : { text: `v${state.helperVersion} · up to date`, tone: "muted" as const };
  return (
    <Row icon={icon} title="Codemux" detail={detail.text} tone={detail.tone}>
      {item && item.phase !== "failed" ? (
        <ProgressBadge item={item} />
      ) : outdated ? (
        <Button variant="outline" size="xs" disabled={running} onClick={() => void updateOne(machine, "helper")}>
          {item?.phase === "failed" ? "Try again" : "Update"}
        </Button>
      ) : null}
    </Row>
  );
}

interface AppUpdate {
  version: string | null;
  current: string | null;
  /** A newer release exists. */
  newer: boolean;
  /** …and Codemux can install it itself (AppImage, Windows installer, or
   *  the desktop when asked from a paired browser). */
  available: boolean;
  state: string;
  progress: number;
  /** Move the app's own update one step: download, then restart. */
  advance: () => void;
  canAutoUpdate: boolean;
}

/** The desktop app's own update, driven by the single update checker. */
function useAppUpdate(): AppUpdate {
  const state = useUpdateStatusStore((s) => s.state);
  const version = useUpdateStatusStore((s) => s.updateVersion);
  const progress = useUpdateStatusStore((s) => s.downloadProgress);
  const canAutoUpdate = useUpdateStatusStore((s) => s.canAutoUpdate);
  const isRemote = useUpdateStatusStore((s) => s.isRemote);
  const startDownload = useUpdateStatusStore((s) => s.startDownload);
  const installAndRestart = useUpdateStatusStore((s) => s.installAndRestart);
  const requestDesktopUpdate = useUpdateStatusStore((s) => s.requestDesktopUpdate);
  const [current, setCurrent] = useState<string | null>(null);
  useEffect(() => {
    getVersion().then(setCurrent).catch(() => setCurrent(null));
  }, []);
  const newer = state === "update-available" || state === "ready";
  const available = newer && (canAutoUpdate || isRemote);
  return {
    version,
    current,
    newer,
    available,
    state,
    progress,
    canAutoUpdate,
    advance: () => {
      if (isRemote) requestDesktopUpdate?.();
      else if (state === "ready") installAndRestart?.();
      else startDownload?.();
    },
  };
}

function AppRow({ app, remote }: { app: AppUpdate; remote: boolean }) {
  const icon = <Monitor className="size-4 text-muted-foreground" aria-hidden />;
  const current = app.current ? `v${app.current}` : "Codemux";
  let detail: { text: string; tone: "muted" | "available" | "error" | "done" } = { text: `${current} · up to date`, tone: "muted" };
  if (app.state === "downloading") detail = { text: `Downloading v${app.version} · ${app.progress}%`, tone: "available" };
  else if (app.state === "ready") detail = { text: `v${app.version} is ready. Restart to finish.`, tone: "available" };
  else if (app.state === "update-available") detail = { text: `${current} → v${app.version}`, tone: "available" };
  else if (app.state === "checking") detail = { text: "Checking…", tone: "muted" };
  return (
    <Row icon={icon} title="Codemux" detail={detail.text} tone={detail.tone}>
      {app.available ? (
        <Button variant="outline" size="xs" disabled={app.state === "downloading"} onClick={app.advance}>
          {remote ? "Update desktop" : app.state === "ready" ? "Restart" : "Update"}
        </Button>
      ) : app.newer ? (
        <Button
          variant="ghost"
          size="xs"
          className="text-muted-foreground"
          onClick={() => void openUrl(`${RELEASES_URL}/tag/v${app.version}`).catch(console.error)}
        >
          Release page
          <ArrowUpRight className="size-3" />
        </Button>
      ) : null}
    </Row>
  );
}
