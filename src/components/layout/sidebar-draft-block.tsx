import {
  memo,
  useCallback,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
} from "react";
import { Paperclip, SquarePen, X } from "lucide-react";
import { useShallow } from "zustand/react/shallow";

import { ProjectAvatar } from "@/components/ui/project-avatar";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";
import {
  projectDisplayName,
  type ProjectGroup,
} from "@/stores/app-store";
import {
  useAgentChatStore,
  type Attachment,
} from "@/stores/agent-chat-store";
import {
  chatDraftHasUserContent,
  useChatDraftStore,
  type ChatDraft,
  type DraftId,
} from "@/stores/chat-draft-store";

const EMPTY_ATTACHMENTS: Attachment[] = [];

interface DraftProjectIdentity {
  path: string | null;
  name: string;
}

/** Stable project lookup shared by the isolated draft block and the inbox's
 *  count-only selector. Existing-workspace drafts resolve through the same
 *  grouped labels as regular sidebar cards, including Home and host suffixes. */
export interface SidebarDraftCatalog {
  homeDir: string | null;
  nameByProjectPath: ReadonlyMap<string, string>;
  projectByWorkspaceId: ReadonlyMap<string, DraftProjectIdentity>;
}

export function buildSidebarDraftCatalog(
  projectGroups: ProjectGroup[],
  homeDir: string | null,
): SidebarDraftCatalog {
  const nameByProjectPath = new Map<string, string>();
  const projectByWorkspaceId = new Map<string, DraftProjectIdentity>();
  for (const group of projectGroups) {
    nameByProjectPath.set(group.projectPath, group.projectName);
    for (const workspace of group.workspaces) {
      projectByWorkspaceId.set(workspace.workspace_id, {
        path: group.projectPath,
        name: group.projectName,
      });
    }
  }
  return { homeDir, nameByProjectPath, projectByWorkspaceId };
}

function projectForDraft(
  draft: ChatDraft,
  catalog: SidebarDraftCatalog,
): DraftProjectIdentity {
  switch (draft.target.kind) {
    case "home":
      return { path: catalog.homeDir, name: "Home" };
    case "project":
      return {
        path: draft.target.projectPath,
        name:
          catalog.nameByProjectPath.get(draft.target.projectPath) ??
          projectDisplayName(draft.target.projectPath, catalog.homeDir),
      };
    case "existing_workspace":
      return (
        catalog.projectByWorkspaceId.get(draft.target.workspaceId) ?? {
          path: null,
          name: "Workspace draft",
        }
      );
  }
}

function isDraftLifecycleVisible(draft: ChatDraft): boolean {
  return (
    draft.promotedTo === null &&
    draft.materializedTo === null &&
    !draft.promoting
  );
}

function draftMatchesFilter(
  draft: ChatDraft,
  catalog: SidebarDraftCatalog,
  filterPath: string | null,
): boolean {
  if (filterPath === null) return true;
  return projectForDraft(draft, catalog).path === filterPath;
}

/** Primitive count subscriptions keep the enormous SidebarInbox from
 *  repainting on every composer keystroke. The count changes only on the
 *  empty↔invested boundary, draft lifecycle transitions, attachment changes,
 *  or project-filter changes.
 *
 *  The active draft is counted through `frozenActive`, the same snapshot the
 *  block renders from, so the count can never disagree with what is on screen
 *  (a draft that was empty at activation renders no row and must not count). */
export function useVisibleSidebarDraftCount(
  catalog: SidebarDraftCatalog,
  filterPath: string | null,
  frozenActive: FrozenActiveDraftRow,
): number {
  const activeDraftId = frozenActive.draftId;
  const textDraftCount = useChatDraftStore((state) => {
    let count = 0;
    for (const draft of Object.values(state.draftsById)) {
      if (draft.draftId === activeDraftId) continue;
      if (!isDraftLifecycleVisible(draft)) continue;
      if (!draftMatchesFilter(draft, catalog, filterPath)) continue;
      if (draft.inputDraft.trim().length > 0) count += 1;
    }
    return count;
  });
  const attachmentOnlyThreadKey = useChatDraftStore((state) =>
    Object.values(state.draftsById)
      .filter(
        (draft) =>
          draft.draftId !== activeDraftId &&
          isDraftLifecycleVisible(draft) &&
          draftMatchesFilter(draft, catalog, filterPath) &&
          draft.inputDraft.trim().length === 0,
      )
      .map((draft) => draft.threadId)
      .sort()
      .join("\u0000"),
  );
  const attachmentOnlyThreadIds = useMemo(
    () =>
      attachmentOnlyThreadKey.length > 0
        ? attachmentOnlyThreadKey.split("\u0000")
        : [],
    [attachmentOnlyThreadKey],
  );
  const attachmentOnlyDraftCount = useAgentChatStore((state) => {
    let count = 0;
    for (const threadId of attachmentOnlyThreadIds) {
      if ((state.threads[threadId]?.stagedAttachments.length ?? 0) > 0) {
        count += 1;
      }
    }
    return count;
  });
  return textDraftCount + attachmentOnlyDraftCount + (frozenActive.row ? 1 : 0);
}

interface SidebarDraftRowData {
  draftId: DraftId;
  createdAt: string;
  project: DraftProjectIdentity;
  preview: string;
  attachmentCount: number;
}

function toDraftRowData(
  draft: ChatDraft,
  attachmentCount: number,
  catalog: SidebarDraftCatalog,
): SidebarDraftRowData {
  const promptPreview = draft.inputDraft.trim().split(/\r?\n/, 1)[0] ?? "";
  return {
    draftId: draft.draftId,
    createdAt: draft.createdAt,
    project: projectForDraft(draft, catalog),
    preview:
      promptPreview.length > 0
        ? promptPreview
        : `${attachmentCount} attachment${attachmentCount === 1 ? "" : "s"}`,
    attachmentCount,
  };
}

/** The active draft's row, captured once and then held still. `row === null`
 *  means the active draft renders no row at all — it was empty when activated,
 *  or the project filter excludes it. */
export interface FrozenActiveDraftRow {
  draftId: DraftId | null;
  filterPath: string | null;
  row: SidebarDraftRowData | null;
}

function captureActiveDraftRow(
  activeDraftId: DraftId | null,
  catalog: SidebarDraftCatalog,
  filterPath: string | null,
): FrozenActiveDraftRow {
  const frozen: FrozenActiveDraftRow = {
    draftId: activeDraftId,
    filterPath,
    row: null,
  };
  const draft = activeDraftId
    ? useChatDraftStore.getState().draftsById[activeDraftId]
    : undefined;
  if (!draft) return frozen;
  if (!isDraftLifecycleVisible(draft)) return frozen;
  if (!draftMatchesFilter(draft, catalog, filterPath)) return frozen;
  const attachmentCount =
    useAgentChatStore.getState().threads[draft.threadId]?.stagedAttachments
      .length ?? 0;
  if (!chatDraftHasUserContent(draft, attachmentCount)) return frozen;
  return { ...frozen, row: toDraftRowData(draft, attachmentCount, catalog) };
}

/** Capture the active row exactly when it becomes active. A fresh empty draft
 *  captures null and therefore does not sprout a row while the user types.
 *  Reopening an existing row captures its current preview and freezes it until
 *  the user leaves again, preventing sidebar repaint churn per keystroke.
 *
 *  Recaptured on project-filter changes too: a draft activated while the filter
 *  hid it captures null, and would otherwise stay invisible after the filter
 *  clears. Typing still never recaptures, so the preview stays frozen. */
export function useFrozenActiveDraftRow(
  catalog: SidebarDraftCatalog,
  filterPath: string | null,
): FrozenActiveDraftRow {
  const activeDraftId = useChatDraftStore((state) => state.activeDraftId);
  const [frozen, setFrozen] = useState<FrozenActiveDraftRow>(() =>
    captureActiveDraftRow(activeDraftId, catalog, filterPath),
  );
  if (frozen.draftId !== activeDraftId || frozen.filterPath !== filterPath) {
    setFrozen(captureActiveDraftRow(activeDraftId, catalog, filterPath));
  }
  return frozen;
}

const SidebarDraftRow = memo(function SidebarDraftRow(props: {
  row: SidebarDraftRowData;
  active: boolean;
  onActivate: (draftId: DraftId) => void;
  onDiscard: (draftId: DraftId) => void;
}) {
  const { row } = props;
  const handleActivate = useCallback(
    () => props.onActivate(row.draftId),
    [props, row.draftId],
  );
  const handleKeyDown = useCallback(
    (event: ReactKeyboardEvent<HTMLDivElement>) => {
      if (event.target !== event.currentTarget) return;
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      props.onActivate(row.draftId);
    },
    [props, row.draftId],
  );
  const handleDiscard = useCallback(
    (event: ReactMouseEvent<HTMLButtonElement>) => {
      event.preventDefault();
      event.stopPropagation();
      props.onDiscard(row.draftId);
    },
    [props, row.draftId],
  );

  return (
    <li className="list-none py-0.5">
      <div
        role="button"
        tabIndex={0}
        data-testid="sidebar-draft-row"
        data-draft-id={row.draftId}
        data-active={props.active ? "true" : "false"}
        aria-label={`Open draft: ${row.preview}`}
        onClick={handleActivate}
        onKeyDown={handleKeyDown}
        className={cn(
          "group/draft relative cursor-pointer overflow-hidden rounded-lg text-left outline-none",
          "transition-[background-color,box-shadow] duration-150",
          "focus-visible:ring-2 focus-visible:ring-ring/50",
          props.active
            ? "bg-accent-ember/[0.09]"
            : "bg-transparent hover:bg-accent-ember/[0.055]",
        )}
      >
        <div className="px-2 py-2">
          <div className="flex h-4 min-w-0 items-center gap-1.5">
            <SquarePen
              aria-hidden="true"
              className="size-3 shrink-0 text-accent-ember"
            />
            <ProjectAvatar
              name={row.project.name}
              size="sm"
              shape="square"
              className="size-3.5"
            />
            <span className="min-w-0 flex-1 truncate text-[11px] font-semibold text-muted-foreground">
              {row.project.name}
            </span>
            {row.attachmentCount > 0 && (
              <span
                aria-label={`${row.attachmentCount} attachment${row.attachmentCount === 1 ? "" : "s"}`}
                className="flex shrink-0 items-center gap-0.5 text-[10px] tabular-nums text-muted-foreground/65"
              >
                <Paperclip className="size-2.5" />
                {row.attachmentCount}
              </span>
            )}
            <button
              type="button"
              aria-label={`Discard draft: ${row.preview}`}
              title="Discard draft"
              onClick={handleDiscard}
              className={cn(
                "-mr-0.5 flex size-5 shrink-0 items-center justify-center rounded-md",
                "text-muted-foreground opacity-0 transition-[opacity,color,background-color]",
                "hover:bg-foreground/[0.07] hover:text-foreground",
                "focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
                "group-hover/draft:opacity-100",
              )}
            >
              <X className="size-3" />
            </button>
          </div>
          <div className="mt-1 truncate text-[12px] font-medium leading-4 text-foreground/90">
            {row.preview}
          </div>
        </div>
      </div>
    </li>
  );
});

/** Invested, unsent drafts live above the workspace inbox. It owns the
 *  high-frequency store subscriptions so typing only repaints this tiny block,
 *  while each row is memoized and the active row is a frozen snapshot. */
export const SidebarDraftBlock = memo(function SidebarDraftBlock(props: {
  catalog: SidebarDraftCatalog;
  filterPath: string | null;
  /** Owned by the inbox (`useFrozenActiveDraftRow`) so the rows below and the
   *  "Nothing active" placeholder agree on whether the active draft shows. */
  frozenActive: FrozenActiveDraftRow;
}) {
  const draftsById = useChatDraftStore((state) => state.draftsById);
  const activeDraftId = useChatDraftStore((state) => state.activeDraftId);
  const setActiveDraft = useChatDraftStore((state) => state.setActiveDraft);
  const discardDraft = useChatDraftStore((state) => state.discardDraft);
  const draftThreadIds = useMemo(
    () => Object.values(draftsById).map((draft) => draft.threadId),
    [draftsById],
  );
  const attachmentsByThreadId = useAgentChatStore(
    useShallow((state) => {
      const result: Record<string, Attachment[]> = {};
      for (const threadId of draftThreadIds) {
        result[threadId] =
          state.threads[threadId]?.stagedAttachments ?? EMPTY_ATTACHMENTS;
      }
      return result;
    }),
  );
  const rowCacheRef = useRef(
    new Map<
      DraftId,
      {
        draft: ChatDraft;
        attachmentCount: number;
        catalog: SidebarDraftCatalog;
        row: SidebarDraftRowData;
      }
    >(),
  );
  const rowForDraft = useCallback(
    (draft: ChatDraft, attachmentCount: number): SidebarDraftRowData => {
      const cached = rowCacheRef.current.get(draft.draftId);
      if (
        cached?.draft === draft &&
        cached.attachmentCount === attachmentCount &&
        cached.catalog === props.catalog
      ) {
        return cached.row;
      }
      const row = toDraftRowData(draft, attachmentCount, props.catalog);
      rowCacheRef.current.set(draft.draftId, {
        draft,
        attachmentCount,
        catalog: props.catalog,
        row,
      });
      return row;
    },
    [props.catalog],
  );

  const frozenActive = props.frozenActive;

  const rows = useMemo(() => {
    const result: SidebarDraftRowData[] = [];
    for (const draft of Object.values(draftsById)) {
      if (!isDraftLifecycleVisible(draft)) continue;
      if (!draftMatchesFilter(draft, props.catalog, props.filterPath)) continue;
      const attachments =
        attachmentsByThreadId[draft.threadId] ?? EMPTY_ATTACHMENTS;
      if (draft.draftId === activeDraftId) {
        if (
          frozenActive.draftId === draft.draftId &&
          frozenActive.row !== null
        ) {
          result.push(frozenActive.row);
        }
        continue;
      }
      if (!chatDraftHasUserContent(draft, attachments.length)) continue;
      result.push(rowForDraft(draft, attachments.length));
    }
    result.sort((left, right) => right.createdAt.localeCompare(left.createdAt));
    return result;
  }, [
    activeDraftId,
    attachmentsByThreadId,
    draftsById,
    frozenActive,
    props.catalog,
    props.filterPath,
    rowForDraft,
  ]);

  const handleActivate = useCallback(
    (draftId: DraftId) => setActiveDraft(draftId),
    [setActiveDraft],
  );
  const handleDiscard = useCallback(
    (draftId: DraftId) => discardDraft(draftId),
    [discardDraft],
  );

  if (rows.length === 0) return null;

  return (
    <ul
      aria-label="Unsent drafts"
      data-testid="sidebar-draft-block"
      className="mb-1 animate-in fade-in slide-in-from-top-1 duration-150"
    >
      {rows.map((row) => (
        <SidebarDraftRow
          key={row.draftId}
          row={row}
          active={row.draftId === activeDraftId}
          onActivate={handleActivate}
          onDiscard={handleDiscard}
        />
      ))}
      <li
        aria-hidden="true"
        data-testid="sidebar-draft-divider"
        className="mx-1 my-1.5 h-px list-none bg-border/60"
      />
    </ul>
  );
});

/** Compact parity for Codemux's collapsed sidebar. The amber pen overlay is
 *  the durable distinction from real workspace avatars; the tooltip carries
 *  the prompt preview that the expanded two-line row would show. */
export const SidebarRailDrafts = memo(function SidebarRailDrafts(props: {
  catalog: SidebarDraftCatalog;
}) {
  const draftsById = useChatDraftStore((state) => state.draftsById);
  const activeDraftId = useChatDraftStore((state) => state.activeDraftId);
  const setActiveDraft = useChatDraftStore((state) => state.setActiveDraft);
  const draftThreadIds = useMemo(
    () => Object.values(draftsById).map((draft) => draft.threadId),
    [draftsById],
  );
  const attachmentCountByThreadId = useAgentChatStore(
    useShallow((state) => {
      const result: Record<string, number> = {};
      for (const threadId of draftThreadIds) {
        result[threadId] =
          state.threads[threadId]?.stagedAttachments.length ?? 0;
      }
      return result;
    }),
  );
  const rows = useMemo(() => {
    const result: SidebarDraftRowData[] = [];
    for (const draft of Object.values(draftsById)) {
      if (!isDraftLifecycleVisible(draft)) continue;
      const attachmentCount = attachmentCountByThreadId[draft.threadId] ?? 0;
      if (!chatDraftHasUserContent(draft, attachmentCount)) continue;
      result.push(toDraftRowData(draft, attachmentCount, props.catalog));
    }
    result.sort((left, right) => right.createdAt.localeCompare(left.createdAt));
    return result;
  }, [attachmentCountByThreadId, draftsById, props.catalog]);

  if (rows.length === 0) return null;

  return (
    <>
      <div
        aria-label="Unsent drafts"
        data-testid="sidebar-rail-drafts"
        className="flex flex-col items-center gap-1.5"
      >
        {rows.map((row) => (
          <Tooltip key={row.draftId} delayDuration={250}>
            <TooltipTrigger asChild>
              <button
                type="button"
                data-rail-draft={row.draftId}
                aria-label={`Open draft: ${row.preview}`}
                onClick={() => setActiveDraft(row.draftId)}
                className={cn(
                  "relative flex size-7 items-center justify-center rounded-lg transition-colors duration-150 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
                  row.draftId === activeDraftId
                    ? "bg-accent-ember/[0.10]"
                    : "bg-transparent hover:bg-accent-ember/[0.07]",
                )}
              >
                <ProjectAvatar
                  name={row.project.name}
                  size="md"
                  shape="square"
                />
                <span className="absolute -bottom-0.5 -right-0.5 flex size-3 items-center justify-center rounded bg-sidebar text-accent-ember">
                  <SquarePen className="size-2.5" />
                </span>
              </button>
            </TooltipTrigger>
            <TooltipContent side="right" className="max-w-64 text-xs">
              <div className="font-semibold">Draft · {row.project.name}</div>
              <div className="mt-0.5 truncate text-muted-foreground">
                {row.preview}
              </div>
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
      <div aria-hidden="true" className="my-1 h-px w-[26px] bg-border/60" />
    </>
  );
});
