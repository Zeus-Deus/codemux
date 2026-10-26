import { useEffect, useId, useLayoutEffect, useRef, useState, type KeyboardEvent } from "react";
import { flushSync } from "react-dom";
import { ChevronLeft, Plus, RefreshCw, TriangleAlert } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Toggle } from "@/components/ui/toggle";
import { SubsectionHeader } from "@/components/settings/settings-primitives";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useUIStore } from "@/stores/ui-store";
import type { PluginInstallation } from "./contract";
import {
  isPluginRecordReadOnly,
  isPluginUnavailable,
  pluginDisplayVersion,
  pluginFailure,
  pluginReviewSubject,
  pluginStatusPresentation,
} from "./model";
import {
  pluginDuplicateHints,
  pluginMatchesQuery,
  pluginNeedsAttention,
  pluginRowSummary,
  sortPluginInstallations,
} from "./plugin-browser";
import { prunePluginSettingsDrafts, usePluginSettingsDrafts } from "./plugin-settings-drafts";
import {
  platformSetEnabled,
  platformStagePrevious,
  refreshPlatformCatalogue,
  usePlatformCatalogue,
} from "./platform-adapter";
import {
  InstallPlatformPluginDialog,
  PlatformPluginDetailsDialog,
  PROCEED,
  RemovePlatformPluginDialog,
  ReviewPlatformPluginDialog,
  UsePreviousPlatformPluginDialog,
  usePluginEventBacklog,
} from "./PlatformPluginDialogs";
import { PlatformPluginEditor, TONE_DOT, type PluginDialog } from "./PlatformPluginEditor";
import { COMPACT_SWITCH } from "./PlatformPluginSettingsForm";
import {
  pluginStartPageInstallationId,
  readPluginStartPage,
  writePluginStartPage,
  type PluginStartPage,
} from "./start-page";

/** Below this section width the list and the selected plugin take turns. */
const TWO_PANE_MIN_PX = 640;

const failureText = (error: unknown) => pluginFailure(error).message;

/** The section's width once laid out; 0 until measured (and in tests), which reads as wide. */
function useMeasuredWidth() {
  const ref = useRef<HTMLDivElement>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const element = ref.current;
    if (!element) return;
    const measure = () => setWidth(element.getBoundingClientRect().width);
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  return [ref, width] as const;
}

/**
 * Settings → Plugins, isolated section: plugins from `codemux-plugin.json`
 * packages that run in their own worker process, with grants over the
 * Codemux APIs they may call (not an OS sandbox). Separate from the
 * renderer plugins above, which keep their own management.
 */
export function PlatformPluginsSection() {
  const { catalogue, loaded, error } = usePlatformCatalogue();
  const sectionRef = useRef<HTMLElement>(null);
  const requestedInstallationId = useUIStore((state) => state.pendingPluginSettingsSelection);
  const [dialog, setDialog] = useState<PluginDialog | null>(null);
  const [startPage, setStartPage] = useState<PluginStartPage | null>(() => readPluginStartPage());
  // Set when the user picks a plugin; until then the first one is shown.
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // On a narrow screen the list and the selected plugin take turns.
  const [detailOpen, setDetailOpen] = useState(false);
  const installations = catalogue?.installations ?? [];
  const executionDisabled = catalogue?.executionDisabled === true;
  const recoverySupported = catalogue?.recoverySupported === true;
  const find = (installationId: string) => installations.find((entry) => entry.installationId === installationId);
  // An older preference names only the plugin id; it counts only while that id is unambiguous.
  const startInstallationId = pluginStartPageInstallationId(startPage, installations);
  const chooseStartPage = (next: PluginStartPage | null) => {
    writePluginStartPage(next);
    setStartPage(next);
  };
  const close = () => setDialog(null);
  const select = (installationId: string) => {
    setSelectedId(installationId);
    setDetailOpen(true);
  };
  useEffect(() => {
    if (typeof requestedInstallationId !== "string" || requestedInstallationId === "") return;
    setSelectedId(requestedInstallationId);
    setDetailOpen(true);
    useUIStore.setState({ pendingPluginSettingsSelection: null });
    sectionRef.current?.scrollIntoView?.({ block: "start" });
  }, [requestedInstallationId]);
  const dialogInstallation =
    dialog === null || dialog.kind === "install"
      ? undefined
      : (find(dialog.installationId) ?? (dialog.kind === "review" ? dialog.installed : undefined));

  // Unsaved settings belong to one set of files; a removed or replaced plugin's go.
  useEffect(() => {
    if (catalogue) prunePluginSettingsDrafts(catalogue.installations);
  }, [catalogue]);

  // The platform isn't in this build or its store failed: core work is unaffected.
  const unavailable = catalogue?.unavailable?.message ?? (catalogue === null && loaded ? error : null);

  return (
    <section ref={sectionRef} className="mt-9" data-platform-plugins>
      <SubsectionHeader
        title="Isolated plugins"
        className="mb-1.5 flex-wrap [&_h3]:whitespace-nowrap"
        action={
          <div className="flex items-center gap-1">
            <Button size="sm" variant="ghost" className="text-muted-foreground" onClick={() => void refreshPlatformCatalogue()}>
              <RefreshCw />
              Refresh
            </Button>
            <Button size="sm" variant="outline" disabled={catalogue === null} onClick={() => setDialog({ kind: "install" })}>
              <Plus />
              Add plugin…
            </Button>
          </div>
        }
      />
      <p className="mb-4 max-w-prose text-body-sm leading-relaxed text-muted-foreground/80">
        Each runs in its own background process and reaches Codemux only through the permissions you grant. It is
        still trusted code running as your user, with your access to files and the network.
      </p>
      {unavailable ? (
        <p
          className="mb-4 rounded-md border border-hairline bg-surface-1 px-4 py-3 text-body-sm text-muted-foreground"
          role="status"
          data-platform-plugins-unavailable
        >
          Isolated plugins are unavailable: {unavailable}
        </p>
      ) : null}
      {executionDisabled ? (
        <p
          className="mb-4 rounded-md border border-hairline bg-surface-1 px-4 py-3 text-body-sm text-muted-foreground"
          role="status"
          data-platform-plugins-paused
        >
          Plugin code is turned off for this launch. You can still review, turn off or remove plugins; your choices
          apply when plugin code runs again.
        </p>
      ) : null}
      {!loaded ? (
        <div className="flex items-center gap-2 text-body-sm text-muted-foreground">
          <Spinner /> Loading plugins…
        </div>
      ) : installations.length === 0 ? (
        catalogue === null ? null : (
          <p className="text-body-sm text-muted-foreground">
            No isolated plugins yet. Add one from a folder on this computer or an https Git URL.
          </p>
        )
      ) : (
        <PluginBrowser
          installations={installations}
          executionDisabled={executionDisabled}
          recoverySupported={recoverySupported}
          selectedId={selectedId}
          detailOpen={detailOpen}
          startInstallationId={startInstallationId}
          startViewId={startPage?.viewId ?? null}
          onStartPage={chooseStartPage}
          onSelect={select}
          onBack={() => setDetailOpen(false)}
          onDialog={setDialog}
        />
      )}
      {dialog?.kind === "install" ? (
        <InstallPlatformPluginDialog
          onClose={close}
          onInstalled={(installed) => {
            select(installed.installationId);
            setDialog({ kind: "review", installationId: installed.installationId, installed });
          }}
        />
      ) : null}
      {dialog?.kind === "review" && dialogInstallation ? (
        <ReviewPlatformPluginDialog
          key={pluginReviewSubject(dialogInstallation).digest}
          installation={dialogInstallation}
          recoverySupported={recoverySupported}
          onClose={close}
        />
      ) : null}
      {dialog?.kind === "remove" && dialogInstallation ? (
        <RemovePlatformPluginDialog
          installation={dialogInstallation}
          onClose={close}
          onRemoved={() => {
            close();
            // The next plugin in the list takes its place.
            setSelectedId(null);
            setDetailOpen(false);
          }}
        />
      ) : null}
      {dialog?.kind === "details" && dialogInstallation ? (
        <PlatformPluginDetailsDialog installation={dialogInstallation} onClose={close} />
      ) : null}
      {dialog?.kind === "use-previous" && dialogInstallation?.recovery?.previous ? (
        <UsePreviousPlatformPluginDialog
          installation={dialogInstallation}
          previousVersion={dialogInstallation.recovery.previous.version}
          onClose={close}
          onConfirm={async () => {
            const previous = dialogInstallation.recovery?.previous;
            if (!previous) return;
            try {
              await platformStagePrevious({
                installationId: dialogInstallation.installationId,
                expectedDigest: previous.digest,
              });
              setDialog({ kind: "review", installationId: dialogInstallation.installationId });
            } catch (cause) {
              toast.error(`Couldn't prepare ${dialogInstallation.manifest.name} v${previous.version}`, {
                description: failureText(cause),
              });
            }
          }}
        />
      ) : null}
    </section>
  );
}

/** Turning a plugin on asks about missed runs first; one prompt serves the whole list. */
function usePluginToggle(recoverySupported: boolean, onReview: (installationId: string) => void) {
  const { resolveBacklog, backlogDialog } = usePluginEventBacklog(recoverySupported);
  const [busy, setBusy] = useState<ReadonlySet<string>>(() => new Set());
  const mark = (installationId: string, on: boolean) =>
    setBusy((current) => {
      const next = new Set(current);
      if (on) next.add(installationId);
      else next.delete(installationId);
      return next;
    });
  const toggle = async (installation: PluginInstallation, enabled: boolean) => {
    const { installationId, manifest } = installation;
    if (enabled && pluginStatusPresentation(installation).primary === "review") {
      onReview(installationId);
      return;
    }
    mark(installationId, true);
    try {
      const backlog = enabled ? await resolveBacklog(installation) : PROCEED;
      if (!backlog.proceed) return;
      await platformSetEnabled({
        installationId,
        enabled,
        ...(backlog.eventBacklog ? { eventBacklog: backlog.eventBacklog } : {}),
      });
    } catch (cause) {
      toast.error(`Couldn't turn ${enabled ? "on" : "off"} ${manifest.name}`, { description: failureText(cause) });
    } finally {
      mark(installationId, false);
    }
  };
  return { toggle, busy, backlogDialog };
}

function PluginBrowser({
  installations,
  executionDisabled,
  recoverySupported,
  selectedId,
  detailOpen,
  startInstallationId,
  startViewId,
  onStartPage,
  onSelect,
  onBack,
  onDialog,
}: {
  installations: ReadonlyArray<PluginInstallation>;
  executionDisabled: boolean;
  recoverySupported: boolean;
  selectedId: string | null;
  detailOpen: boolean;
  startInstallationId: string | null;
  startViewId: string | null;
  onStartPage: (next: PluginStartPage | null) => void;
  onSelect: (installationId: string) => void;
  onBack: () => void;
  onDialog: (dialog: PluginDialog) => void;
}) {
  const idPrefix = useId();
  const [containerRef, width] = useMeasuredWidth();
  const narrow = width > 0 && width < TWO_PANE_MIN_PX;
  const listRef = useRef<HTMLUListElement>(null);
  const headingRef = useRef<HTMLHeadingElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);
  const [query, setQuery] = useState("");
  const [attentionOnly, setAttentionOnly] = useState(false);
  const drafts = usePluginSettingsDrafts((state) => state.drafts);
  const { toggle, busy, backlogDialog } = usePluginToggle(recoverySupported, (installationId) =>
    onDialog({ kind: "review", installationId }),
  );

  const sorted = sortPluginInstallations(installations);
  const hints = pluginDuplicateHints(sorted);
  const entries = sorted.map((installation) => {
    const status = pluginStatusPresentation(installation, { executionDisabled });
    return { installation, status, attention: pluginNeedsAttention(installation, status) };
  });
  const attentionCount = entries.filter((entry) => entry.attention).length;
  const filterAttention = attentionOnly && attentionCount > 0;
  const needle = query.trim().toLowerCase();
  const visible = entries.filter(
    (entry) => (!filterAttention || entry.attention) && pluginMatchesQuery(entry.installation, needle),
  );
  const filtering = needle !== "" || filterAttention;
  const selected =
    selectedId === null
      ? (entries[0] ?? null)
      : (entries.find((entry) => entry.installation.installationId === selectedId) ?? null);
  const rowId = (installationId: string) => `${idPrefix}-row-${installationId}`;
  const showList = !narrow || !detailOpen;
  const showEditor = !narrow || detailOpen;

  const select = (installationId: string) => {
    flushSync(() => onSelect(installationId));
    // On a narrow screen the list just went away; take focus to what replaced it.
    if (narrow) headingRef.current?.focus();
  };
  const back = () => {
    flushSync(onBack);
    const row = selected === null ? null : document.getElementById(rowId(selected.installation.installationId));
    // A search or the attention filter may have hidden the row since it was opened.
    if (row) row.focus();
    else searchRef.current?.focus();
  };
  const clearFilters = () => {
    setQuery("");
    setAttentionOnly(false);
    searchRef.current?.focus();
  };
  // Arrow keys move between rows, so a long list is not a long run of tab stops.
  const moveFocus = (event: KeyboardEvent<HTMLUListElement>) => {
    const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
    if (!keys.includes(event.key)) return;
    const rows = [...(listRef.current?.querySelectorAll<HTMLButtonElement>("[data-plugin-select]") ?? [])];
    const index = rows.indexOf(document.activeElement as HTMLButtonElement);
    if (index === -1 || rows.length === 0) return;
    event.preventDefault();
    const next =
      event.key === "Home"
        ? 0
        : event.key === "End"
          ? rows.length - 1
          : Math.min(rows.length - 1, Math.max(0, index + (event.key === "ArrowDown" ? 1 : -1)));
    rows[next]?.focus();
  };

  return (
    <div ref={containerRef} data-platform-plugin-browser={narrow ? "narrow" : "wide"}>
      <div
        className={cn(
          "overflow-hidden rounded-lg border border-border bg-card/40",
          !narrow && "grid h-[min(42rem,calc(100dvh-12rem))] min-h-[26rem] grid-cols-[15rem_minmax(0,1fr)]",
        )}
      >
        {showList ? (
          <div className={cn("min-w-0", !narrow && "flex min-h-0 flex-col border-r border-border")}>
            <div className="grid gap-2 border-b border-hairline p-2">
              <Input
                ref={searchRef}
                type="search"
                aria-label="Search plugins"
                placeholder="Search plugins"
                className="h-8"
                value={query}
                onChange={(event) => setQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Escape" && query !== "") {
                    event.preventDefault();
                    setQuery("");
                  }
                }}
              />
              <div className="flex min-h-7 flex-wrap items-center justify-between gap-2 px-0.5">
                <span className="text-label text-muted-foreground" aria-live="polite">
                  {filtering
                    ? `${visible.length} of ${installations.length}`
                    : `${installations.length} ${installations.length === 1 ? "plugin" : "plugins"}`}
                </span>
                {attentionCount > 0 ? (
                  <Toggle size="sm" variant="outline" pressed={filterAttention} onPressedChange={setAttentionOnly}>
                    <TriangleAlert aria-hidden />
                    Needs attention ({attentionCount})
                  </Toggle>
                ) : null}
              </div>
            </div>
            <ul
              ref={listRef}
              aria-label="Isolated plugins"
              className={cn("grid min-w-0 grid-cols-1 content-start gap-px p-1.5", !narrow && "min-h-0 flex-1 overflow-y-auto")}
              onKeyDown={moveFocus}
            >
              {visible.map(({ installation, status }) => (
                <PluginListRow
                  key={installation.installationId}
                  id={rowId(installation.installationId)}
                  installation={installation}
                  summary={pluginRowSummary(installation, status, {
                    unsavedSettings: drafts[installation.installationId]?.digest === installation.digest,
                  })}
                  tone={status.tone}
                  hint={hints.get(installation.installationId) ?? null}
                  selected={installation.installationId === selected?.installation.installationId}
                  busy={busy.has(installation.installationId)}
                  onSelect={select}
                  onToggle={(enabled) => void toggle(installation, enabled)}
                />
              ))}
              {visible.length === 0 ? (
                <li className="flex flex-wrap items-center gap-x-3 gap-y-2 px-2.5 py-3 text-body-sm text-muted-foreground">
                  <span>No plugins match.</span>
                  <Button size="sm" variant="outline" onClick={clearFilters}>
                    Clear filters
                  </Button>
                </li>
              ) : null}
            </ul>
          </div>
        ) : null}
        {showEditor ? (
          <div className={cn("min-w-0", !narrow && "min-h-0 overflow-y-auto")}>
            {narrow ? (
              <div className="px-3 pt-2">
                <Button size="sm" variant="ghost" className="-ms-1 text-muted-foreground" onClick={back}>
                  <ChevronLeft />
                  All plugins
                </Button>
              </div>
            ) : null}
            {selected ? (
              <PlatformPluginEditor
                key={selected.installation.installationId}
                installation={selected.installation}
                status={selected.status}
                executionDisabled={executionDisabled}
                recoverySupported={recoverySupported}
                startViewId={startInstallationId === selected.installation.installationId ? startViewId : null}
                headingRef={headingRef}
                showSwitch={narrow}
                toggleBusy={busy.has(selected.installation.installationId)}
                onToggle={(installation, enabled) => void toggle(installation, enabled)}
                onStartPage={onStartPage}
                onDialog={onDialog}
              />
            ) : (
              <h4
                ref={headingRef}
                tabIndex={-1}
                className="px-5 py-4 text-body-sm font-normal text-muted-foreground outline-none"
              >
                This plugin is no longer installed.
              </h4>
            )}
          </div>
        ) : null}
      </div>
      {backlogDialog}
    </div>
  );
}

/** Selecting a row only shows its plugin; the switch beside it is the one control that acts. */
function PluginListRow({
  id,
  installation,
  summary,
  tone,
  hint,
  selected,
  busy,
  onSelect,
  onToggle,
}: {
  id: string;
  installation: PluginInstallation;
  summary: string;
  tone: keyof typeof TONE_DOT;
  /** Tells a same-named plugin apart. */
  hint: string | null;
  selected: boolean;
  busy: boolean;
  onSelect: (installationId: string) => void;
  onToggle: (enabled: boolean) => void;
}) {
  const { manifest } = installation;
  const version = pluginDisplayVersion(installation);
  const readOnly = isPluginRecordReadOnly(installation);
  const unavailable = isPluginUnavailable(installation);
  const name = hint ? `${manifest.name} (${hint})` : manifest.name;
  return (
    <li
      className={cn(
        "flex min-w-0 items-center gap-2 rounded-md pr-2.5 transition-colors duration-100",
        selected ? "bg-surface-3" : "hover:bg-surface-2",
      )}
      data-platform-plugin-row={installation.installationId}
      data-platform-plugin-status={installation.status}
    >
      <button
        id={id}
        type="button"
        data-plugin-select
        aria-current={selected ? "true" : undefined}
        aria-label={name}
        aria-describedby={`${id}-summary`}
        className="flex min-w-0 flex-1 flex-col rounded-md py-2 pl-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
        onClick={() => onSelect(installation.installationId)}
      >
        <span className="flex min-w-0 items-baseline gap-1.5">
          <span
            className={cn(
              "truncate text-body font-medium",
              installation.enabled || selected ? "text-foreground" : "text-muted-foreground",
            )}
          >
            {manifest.name}
          </span>
          {version ? (
            <span className="max-w-20 shrink-0 truncate font-mono text-label text-muted-foreground/70">v{version}</span>
          ) : null}
        </span>
        {hint ? (
          <span className="truncate font-mono text-label text-muted-foreground/70" title={hint}>
            {hint}
          </span>
        ) : null}
        <span id={`${id}-summary`} className="mt-0.5 flex min-w-0 items-start gap-1.5 text-label text-muted-foreground">
          <span aria-hidden className="flex h-[1.4em] shrink-0 items-center">
            <span className={cn("size-1.5 rounded-full", TONE_DOT[tone])} />
          </span>
          <span className="line-clamp-2 [overflow-wrap:anywhere]">{summary}</span>
        </span>
      </button>
      <Switch
        size="sm"
        className={COMPACT_SWITCH}
        checked={installation.enabled}
        // Turning off always works; turning on needs a host that can run it.
        disabled={busy || readOnly || (!installation.enabled && unavailable)}
        aria-label={`${installation.enabled ? "Turn off" : "Turn on"} ${name}`}
        onCheckedChange={onToggle}
      />
    </li>
  );
}
