import { useCallback, useEffect, useMemo, useState } from "react";

import { AlertTriangle, Loader2, RotateCw, Server } from "lucide-react";

import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/switch";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useMcpRuntime } from "@/hooks/use-mcp-runtime";
import { cn } from "@/lib/utils";
import { useMcpStore } from "@/stores/mcp-store";
import {
  listMcpServers,
  listMcpToolsWithCapInfo,
  MCP_CODEMUX_SELF_ID,
  MCP_STATUS_CHANGED_EVENT,
  primeMcpRuntime,
  restartMcpServerCmd,
  startMcpServerCmd,
  stopMcpServerCmd,
  type CappedTools,
  type McpConfigSource,
  type McpServerConfig,
  type McpServerRuntime,
} from "@/tauri/commands";
import { listen } from "@tauri-apps/api/event";

import { McpToolModal } from "./mcp-tool-modal";

/** Slow-start threshold (ms). Servers stuck in `starting` longer than
 *  this surface a "taking longer than usual" hint. 3 s matches
 *  Cursor's UX. */
const SLOW_START_THRESHOLD_MS = 3000;

interface Props {
  /** Active workspace's project root (or null when no project is selected
   *  — e.g. user opened settings from Home). Project-scoped config files
   *  are skipped when this is null; user-wide files still load. */
  projectRoot: string | null;
}

/**
 * Settings → MCP Servers (Stage 2: live runtime status + toggles).
 *
 * Mirrors Skills section structure. Each row shows:
 *   - server name + source badges (codemux always-on, multi-source dedupe).
 *   - live runtime status dot (running/starting/errored/stopped).
 *   - tools count when running.
 *   - toggle switch (suppressed for the Codemux always-on row).
 *   - restart button on errored rows; tooltip shows stderr tail.
 *
 * On mount: fetches discovered configs, syncs disabledIds to backend,
 * primes the runtime so users see live status without first opening
 * a chat. The hook listens to `mcp-status-changed` for incremental
 * updates so the UI never polls.
 */
export function McpSection({ projectRoot }: Props) {
  const [servers, setServers] = useState<McpServerConfig[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);

  const { runtimes } = useMcpRuntime();
  const isDisabled = useMcpStore((s) => s.isDisabled);
  const toggleDisabled = useMcpStore((s) => s.toggleDisabled);
  const syncToBackend = useMcpStore((s) => s.syncToBackend);

  // Stage 4 — cap envelope. Refreshed alongside the server list and
  // also after every `mcp-status-changed` event so the banner stays
  // accurate as servers come up / go down.
  const [capInfo, setCapInfo] = useState<CappedTools | null>(null);
  const refreshCapInfo = useCallback(async () => {
    try {
      const info = await listMcpToolsWithCapInfo();
      setCapInfo(info);
    } catch (err) {
      console.warn("[mcp] listMcpToolsWithCapInfo failed:", err);
    }
  }, []);

  // Stage 4 — modal state. Click a row → set `pendingServer` →
  // McpToolModal renders. Closing nulls it out.
  const [pendingServer, setPendingServer] =
    useState<McpServerConfig | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const result = await listMcpServers(projectRoot);
      setServers(result);
      await syncToBackend();
      await primeMcpRuntime(projectRoot);
      await refreshCapInfo();
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
      setLoaded(true);
    }
  }, [projectRoot, syncToBackend, refreshCapInfo]);

  useEffect(() => {
    void load();
  }, [load]);

  // Re-fetch cap info whenever a runtime status changes — a server
  // coming up or going down can flip whether the cap is engaged.
  // `listen()` throws synchronously when `window.__TAURI_INTERNALS__`
  // is missing (jsdom tests without the Tauri shim mocked); wrap in
  // try/catch so the section degrades gracefully under those envs.
  useEffect(() => {
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    try {
      void listen(MCP_STATUS_CHANGED_EVENT, () => {
        if (cancelled) return;
        void refreshCapInfo();
      })
        .then((dispose) => {
          if (cancelled) {
            dispose();
            return;
          }
          unlisten = dispose;
        })
        .catch((err) => {
          console.warn("[mcp-section] listen failed:", err);
        });
    } catch (err) {
      console.warn("[mcp-section] listen threw synchronously:", err);
    }
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [refreshCapInfo]);

  const groups = useMemo(() => groupByPrimarySource(servers), [servers]);
  const collidingNames = useMemo(() => collidingNameSet(servers), [servers]);

  return (
    <div>
      <div className="mb-6 flex items-start justify-between gap-4">
        <div>
          <h2 className="text-base font-semibold tracking-tight">
            MCP Servers
          </h2>
          <p className="mt-1 text-sm text-muted-foreground">
            Model Context Protocol servers expose tools to your agent.
            Servers spawn lazily on first chat session start; toggle a
            row off to stop it. Codemux's own MCP is always on.
          </p>
        </div>
        <Button
          variant="outline"
          size="sm"
          onClick={() => void load()}
          disabled={loading}
          aria-label="Refresh MCP servers"
        >
          <RotateCw
            className={cn("mr-1 h-3 w-3", loading && "animate-spin")}
            aria-hidden
          />
          Refresh
        </Button>
      </div>

      {error && (
        <p
          data-testid="mcp-error"
          className="mb-4 rounded-md bg-destructive/10 px-3 py-2 text-xs text-destructive"
        >
          Failed to load MCP servers: {error}
        </p>
      )}

      {loading && !loaded ? (
        <div
          data-testid="mcp-loading"
          className="flex items-center gap-2 py-6 text-sm text-muted-foreground"
        >
          <Loader2 className="h-4 w-4 animate-spin" aria-hidden />
          Loading MCP servers…
        </div>
      ) : servers.length === 0 && !error ? (
        <p
          data-testid="mcp-empty"
          className="py-6 text-center text-sm text-muted-foreground"
        >
          No MCP servers discovered.
        </p>
      ) : (
        <TooltipProvider>
          {capInfo && capInfo.droppedCount > 0 && (
            <CapBanner info={capInfo} />
          )}
          <div className="space-y-6">
            {groups.map((group) => (
              <ServerGroup
                key={group.source}
                source={group.source}
                servers={group.servers}
                collidingNames={collidingNames}
                runtimes={runtimes}
                cappedServerIds={
                  new Set(capInfo?.droppedServers ?? [])
                }
                isDisabled={isDisabled}
                toggleDisabled={toggleDisabled}
                projectRoot={projectRoot}
                onView={(s) => setPendingServer(s)}
              />
            ))}
          </div>
        </TooltipProvider>
      )}

      <McpToolModal
        server={pendingServer}
        runtime={
          pendingServer ? runtimes.get(pendingServer.id) ?? null : null
        }
        onClose={() => setPendingServer(null)}
      />
    </div>
  );
}

/** Stage 4 cap banner. Shown only when `apply_tool_cap` actually
 *  trimmed user-MCP tools to fit the 50-tool budget. Codemux's tools
 *  are protected so this banner is purely about user installs. */
function CapBanner({ info }: { info: CappedTools }) {
  return (
    <div
      data-testid="mcp-cap-banner"
      className="mb-4 flex items-start gap-2 rounded-md border border-status-working/30 bg-status-working/10 px-3 py-2 text-xs text-status-working dark:text-status-working"
    >
      <AlertTriangle className="size-3.5 shrink-0 mt-0.5" aria-hidden />
      <div className="flex-1">
        <p className="font-medium">
          {info.totalBeforeCap - info.droppedCount} of {info.totalBeforeCap}{" "}
          tools registered with the agent
        </p>
        <p className="mt-0.5 text-status-working/80 dark:text-status-working/80">
          Codemux's built-in tools are protected; {info.droppedCount} user
          MCP tool{info.droppedCount === 1 ? " was" : "s were"} dropped to
          fit the cap. Disable a server in this list to reclaim slots.
        </p>
      </div>
    </div>
  );
}

function ServerGroup({
  source,
  servers,
  collidingNames,
  runtimes,
  cappedServerIds,
  isDisabled,
  toggleDisabled,
  projectRoot,
  onView,
}: {
  source: McpConfigSource;
  servers: McpServerConfig[];
  collidingNames: Set<string>;
  runtimes: Map<string, McpServerRuntime>;
  cappedServerIds: Set<string>;
  isDisabled: (id: string) => boolean;
  toggleDisabled: (id: string) => void;
  projectRoot: string | null;
  onView: (server: McpServerConfig) => void;
}) {
  return (
    <section data-testid={`mcp-group-${source}`}>
      <header className="mb-2 flex items-baseline justify-between">
        <h3 className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          {sourceHeading(source)}
        </h3>
        <span className="text-[10px] text-muted-foreground/70">
          {servers.length} server{servers.length === 1 ? "" : "s"}
        </span>
      </header>
      <ul className="divide-y divide-border/40 rounded-md border border-border/50">
        {servers.map((server) => (
          <li key={server.id}>
            <ServerRow
              server={server}
              runtime={runtimes.get(server.id) ?? null}
              showSourceDisambiguator={collidingNames.has(server.name)}
              capped={cappedServerIds.has(server.id)}
              disabled={isDisabled(server.id)}
              onToggle={() => toggleDisabled(server.id)}
              onView={() => onView(server)}
              projectRoot={projectRoot}
            />
          </li>
        ))}
      </ul>
    </section>
  );
}

function ServerRow({
  server,
  runtime,
  showSourceDisambiguator,
  capped,
  disabled,
  onToggle,
  onView,
  projectRoot,
}: {
  server: McpServerConfig;
  runtime: McpServerRuntime | null;
  showSourceDisambiguator: boolean;
  capped: boolean;
  disabled: boolean;
  onToggle: () => void;
  onView: () => void;
  projectRoot: string | null;
}) {
  const additionalSources = server.sources.slice(1);
  const isCodemuxSelf = server.id === MCP_CODEMUX_SELF_ID;

  const handleToggle = (next: boolean) => {
    onToggle();
    // The store-level toggle persists + syncs to the backend; here we
    // just trigger the runtime spawn/stop so the row's status updates
    // immediately without waiting for the next prime.
    if (next) {
      void startMcpServerCmd(server.id, projectRoot).catch((err) =>
        console.warn(`[mcp] start ${server.id} failed:`, err),
      );
    } else {
      void stopMcpServerCmd(server.id).catch((err) =>
        console.warn(`[mcp] stop ${server.id} failed:`, err),
      );
    }
  };

  return (
    <div
      className="group flex items-center gap-3 px-3 py-2"
      data-testid={`mcp-row-${server.id}`}
    >
      <Server
        className={cn(
          "size-3.5 shrink-0 text-muted-foreground",
          disabled && "opacity-50",
        )}
        aria-hidden
      />
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2 flex-wrap">
          <span
            className={cn(
              "truncate text-sm font-medium",
              disabled && "opacity-60",
            )}
          >
            {server.name}
          </span>
          {showSourceDisambiguator && (
            <Tooltip>
              <TooltipTrigger asChild>
                <span
                  className="text-[10px] text-muted-foreground cursor-help"
                  data-testid={`mcp-row-${server.id}-disambig`}
                >
                  · {sourceHeading(server.sources[0])}
                </span>
              </TooltipTrigger>
              <TooltipContent className="max-w-xs text-xs">
                Same name as another server with a different config —
                they are treated as separate.
              </TooltipContent>
            </Tooltip>
          )}
          {server.sources.includes("codemux") && (
            <Badge variant="secondary" className="text-[10px]">
              always on
            </Badge>
          )}
          {server.transport === "http" && (
            <Badge variant="outline" className="text-[10px]">
              HTTP
            </Badge>
          )}
          <McpStatusBadge runtime={runtime} disabled={disabled} server={server} />
          {capped && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Badge
                  variant="outline"
                  className="text-[10px] cursor-help border-status-working/50 text-status-working dark:text-status-working"
                  data-testid={`mcp-row-${server.id}-capped`}
                >
                  capped
                </Badge>
              </TooltipTrigger>
              <TooltipContent className="max-w-xs text-xs">
                Some tools from this server were dropped to fit the
                50-tool cap. Disable other servers to reclaim slots.
              </TooltipContent>
            </Tooltip>
          )}
          {additionalSources.length > 0 && (
            <span
              className="text-[10px] text-muted-foreground/80"
              data-testid={`mcp-row-${server.id}-extra-sources`}
            >
              also: {additionalSources.map(sourceHeading).join(", ")}
            </span>
          )}
        </div>
        <p className="mt-0.5 truncate font-mono text-xs text-muted-foreground">
          {commandPreview(server)}
        </p>
      </div>
      {/* Hover-reveal "View tools" button — opens the modal. */}
      <Button
        variant="ghost"
        size="sm"
        className="h-7 px-2 text-xs opacity-0 transition-opacity group-hover:opacity-100"
        onClick={onView}
        data-testid={`mcp-row-${server.id}-view`}
      >
        View tools
      </Button>
      {/* Restart affordance for errored rows. Stays out of the way otherwise. */}
      {runtime?.status.kind === "errored" && !isCodemuxSelf && (
        <Button
          variant="ghost"
          size="sm"
          className="h-7 px-2 text-xs"
          onClick={() =>
            void restartMcpServerCmd(server.id).catch((err) =>
              console.warn(`[mcp] restart ${server.id} failed:`, err),
            )
          }
          data-testid={`mcp-row-${server.id}-restart`}
        >
          Restart
        </Button>
      )}
      {/* Codemux always-on row: no toggle. Every other row: switch. */}
      {!isCodemuxSelf && (
        <Switch
          checked={!disabled}
          onCheckedChange={handleToggle}
          aria-label={`Enable ${server.name}`}
          data-testid={`mcp-row-${server.id}-toggle`}
        />
      )}
    </div>
  );
}

function McpStatusBadge({
  runtime,
  disabled,
  server,
}: {
  runtime: McpServerRuntime | null;
  disabled: boolean;
  server: McpServerConfig;
}) {
  if (disabled) {
    return (
      <span
        className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-muted-foreground/80"
        data-testid={`mcp-row-${server.id}-status`}
        data-status="disabled"
      >
        <span className="size-1.5 rounded-full bg-muted-foreground/40" />
        disabled
      </span>
    );
  }

  if (!runtime || runtime.status.kind === "discovered") {
    return (
      <span
        className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-muted-foreground/80"
        data-testid={`mcp-row-${server.id}-status`}
        data-status="discovered"
      >
        <span className="size-1.5 rounded-full bg-muted-foreground/40" />
        discovered
      </span>
    );
  }

  if (runtime.status.kind === "starting") {
    const startedAt = runtime.startedAtMs ?? Date.now();
    const elapsedMs = Date.now() - startedAt;
    const slow = elapsedMs > SLOW_START_THRESHOLD_MS;
    return (
      <span
        className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-muted-foreground"
        data-testid={`mcp-row-${server.id}-status`}
        data-status={slow ? "starting-slow" : "starting"}
      >
        <Loader2 className="size-2.5 animate-spin" aria-hidden />
        {slow ? "slow start — taking longer than usual" : "starting…"}
      </span>
    );
  }

  if (runtime.status.kind === "running") {
    return (
      <span
        className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-status-open dark:text-status-open"
        data-testid={`mcp-row-${server.id}-status`}
        data-status="running"
      >
        <span className="size-1.5 rounded-full bg-status-open" />
        {runtime.status.toolCount} tool{runtime.status.toolCount === 1 ? "" : "s"}
      </span>
    );
  }

  if (runtime.status.kind === "errored") {
    const tail = runtime.stderrTail?.trim();
    return (
      <Tooltip>
        <TooltipTrigger asChild>
          <span
            className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-destructive"
            data-testid={`mcp-row-${server.id}-status`}
            data-status="errored"
          >
            <span className="size-1.5 rounded-full bg-destructive" />
            errored
          </span>
        </TooltipTrigger>
        <TooltipContent className="max-w-md whitespace-pre-wrap font-mono text-[10px]">
          {runtime.errorMessage ?? "MCP server error"}
          {tail ? `\n\n${tail}` : ""}
        </TooltipContent>
      </Tooltip>
    );
  }

  // stopped
  return (
    <span
      className="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[10px] text-muted-foreground/80"
      data-testid={`mcp-row-${server.id}-status`}
      data-status="stopped"
    >
      <span className="size-1.5 rounded-full bg-muted-foreground/40" />
      stopped
    </span>
  );
}

function commandPreview(server: McpServerConfig): string {
  if (server.transport === "http") return server.command;
  const args = server.args.length > 0 ? ` ${server.args.join(" ")}` : "";
  return `${server.command}${args}`;
}

function sourceHeading(source: McpConfigSource): string {
  switch (source) {
    case "codemux":
      return "Codemux";
    case "codemuxUser":
      return "Codemux · User";
    case "codemuxProject":
      return "Codemux · Project";
    case "claudeUser":
      return "Claude · User";
    case "claudeLocal":
      return "Claude · Local";
    case "claudeProject":
      return "Claude · Project";
    case "cursorUser":
      return "Cursor · User";
    case "cursorProject":
      return "Cursor · Project";
    case "codexUser":
      return "Codex · User";
    case "openCodeUser":
      return "OpenCode · User";
    case "openCodeProject":
      return "OpenCode · Project";
  }
}

const SOURCE_ORDER: McpConfigSource[] = [
  "codemux",
  "codemuxUser",
  "codemuxProject",
  "claudeUser",
  "claudeLocal",
  "claudeProject",
  "cursorUser",
  "cursorProject",
  "codexUser",
  "openCodeUser",
  "openCodeProject",
];

interface ServerGroupData {
  source: McpConfigSource;
  servers: McpServerConfig[];
}

function groupByPrimarySource(
  servers: McpServerConfig[],
): ServerGroupData[] {
  const buckets = new Map<McpConfigSource, McpServerConfig[]>();
  for (const s of servers) {
    const primary = s.sources[0] ?? "codemux";
    const arr = buckets.get(primary) ?? [];
    arr.push(s);
    buckets.set(primary, arr);
  }
  const out: ServerGroupData[] = [];
  for (const source of SOURCE_ORDER) {
    const list = buckets.get(source);
    if (list && list.length > 0) {
      out.push({ source, servers: list });
    }
  }
  return out;
}

function collidingNameSet(servers: McpServerConfig[]): Set<string> {
  const counts = new Map<string, number>();
  for (const s of servers) {
    counts.set(s.name, (counts.get(s.name) ?? 0) + 1);
  }
  const out = new Set<string>();
  for (const [name, count] of counts) {
    if (count > 1) out.add(name);
  }
  return out;
}
