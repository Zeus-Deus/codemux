import { useEffect, useRef, useSyncExternalStore } from "react";
import { Badge } from "@/components/ui/badge";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { useActiveWorkspace } from "@/stores/app-store";
import { cn } from "@/lib/utils";
import type { PluginThreadStatusTone } from "./contract";
import {
  newPluginOperationId,
  pluginRunnableInstallations,
  pluginThreadStatusEntries,
} from "./model";
import { openPlatformView } from "./plugin-actions";
import {
  pluginChangeKey,
  pluginStatusTopic,
  platformInvoke,
  useDocumentVisible,
  usePlatformCatalogue,
  usePlatformPluginsStore,
} from "./platform-adapter";
import {
  createPluginThreadStatusPoller,
  type PluginThreadStatusPoller,
  type PluginThreadStatusValue,
} from "./thread-status";
import { pluginProjectIdOf, workspaceActiveThreadId } from "./workspace-context";

let poller: PluginThreadStatusPoller | null = null;
/** One poller for the app: the open chat is the only one asked. */
function sharedPoller(): PluginThreadStatusPoller {
  poller ??= createPluginThreadStatusPoller({ invoke: platformInvoke, operationId: () => newPluginOperationId() });
  return poller;
}

const subscribe = (listener: () => void) => sharedPoller().subscribe(listener);
const snapshot = () => sharedPoller().getSnapshot();

/** The last answers for one chat; empty for any other. */
export function pluginThreadStatusesFor(threadId: string | null): ReadonlyArray<PluginThreadStatusValue> {
  if (threadId === null) return [];
  return snapshot().filter((value) => value.threadId === threadId);
}

/**
 * Keeps the open chat's plugin statuses current: asks when the chat or the
 * plugins change, every interval while the window is in front, on focus, and
 * when a plugin says a shown status changed. Renders nothing.
 */
export function PluginThreadStatusDriver() {
  const { catalogue } = usePlatformCatalogue();
  const workspace = useActiveWorkspace();
  const visible = useDocumentVisible();
  const threadId = workspaceActiveThreadId(workspace);
  const entries = threadId === null ? [] : pluginThreadStatusEntries(pluginRunnableInstallations(catalogue));
  const target = threadId === null || workspace === null ? null : { threadId, projectId: pluginProjectIdOf(workspace) };

  // Reconciling is cheap and idempotent, so it follows every render.
  useEffect(() => {
    sharedPoller().update({ target, entries, visible });
  });
  useEffect(() => {
    const onFocus = () => sharedPoller().refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, []);

  // A plugin that says a shown status changed is asked again now.
  const statusRevision = usePlatformPluginsStore((state) =>
    entries
      .map(({ installation, item }) =>
        state.revisions.get(
          pluginChangeKey({
            installationId: installation.installationId,
            generation: installation.generation,
            topic: pluginStatusTopic(item.id),
          }),
        ) ?? 0,
      )
      .join(","),
  );
  const seen = useRef(statusRevision);
  useEffect(() => {
    if (seen.current !== statusRevision) sharedPoller().refresh();
    seen.current = statusRevision;
  }, [statusRevision]);
  return null;
}

/** Subscribe/isVisible for `COMPOSER_STATUS_AREA`, so an empty slot drops its heading. */
export const pluginThreadStatusSlotData = {
  subscribe,
  isVisible: ({ threadId }: { threadId: string | null }) => pluginThreadStatusesFor(threadId).length > 0,
};

const TONE_DOT: Record<PluginThreadStatusTone, string> = {
  neutral: "bg-muted-foreground/50",
  info: "bg-status-working",
  success: "bg-status-open",
  warning: "bg-status-attention",
  danger: "bg-destructive",
};

export function pluginThreadStatusDescription(value: PluginThreadStatusValue): string {
  const name = value.entry.installation.manifest.name;
  return value.status.detail === undefined ? name : `${name}: ${value.status.detail}`;
}

function StatusChip({
  value,
  workspaceId,
}: {
  value: PluginThreadStatusValue;
  workspaceId: string | null;
}) {
  const { label, tone } = value.status;
  const description = pluginThreadStatusDescription(value);
  const { installation, item } = value.entry;
  const viewId = item.viewId;
  const content = (
    <>
      <span aria-hidden className={cn("size-1.5 shrink-0 rounded-full", TONE_DOT[tone])} />
      {label}
    </>
  );
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        {viewId === undefined ? (
          <Badge variant="outline" data-plugin-status={value.key} data-tone={tone}>
            {content}
            <span className="sr-only">{`, ${description}`}</span>
          </Badge>
        ) : (
          <Badge
            asChild
            variant="outline"
            data-plugin-status={value.key}
            data-tone={tone}
            className="cursor-pointer hover:bg-surface-2"
          >
            <button
              type="button"
              aria-description={description}
              onClick={() => openPlatformView({ installation, workspaceId, threadId: value.threadId }, viewId)}
            >
              {content}
            </button>
          </Badge>
        )}
      </TooltipTrigger>
      <TooltipContent side="bottom">{description}</TooltipContent>
    </Tooltip>
  );
}

/**
 * Compact plugin statuses for a chat, e.g. "Realm live". Nothing renders
 * until a plugin reports one, and a failing plugin stays quiet.
 */
export function PluginThreadStatusChips({
  workspaceId,
  threadId,
}: {
  workspaceId: string | null;
  threadId: string | null;
}) {
  useSyncExternalStore(subscribe, snapshot, snapshot);
  const values = pluginThreadStatusesFor(threadId);
  if (values.length === 0) return null;
  return (
    <div role="group" aria-label="Plugin status" className="flex shrink-0 flex-wrap items-center gap-1">
      {values.map((value) => (
        <StatusChip key={value.key} value={value} workspaceId={workspaceId} />
      ))}
    </div>
  );
}

/** Test-only reset. */
export function resetPluginThreadStatusesForTests(): void {
  poller?.dispose();
  poller = null;
}
