import { FileDiff } from "lucide-react";
import { useRef, useState } from "react";

import { Eyebrow } from "@/components/ui/eyebrow";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { useUIStore } from "@/stores/ui-store";

import type { TurnChangeSummary } from "./turn-changes";
import { TurnDiffDialog } from "./TurnDiffDialog";

/** Show a path relative to the workspace when it lives inside it. */
function displayPath(path: string, cwd?: string | null): string {
  if (!cwd) return path;
  const root = cwd.replace(/[\\/]+$/, "");
  if (path.startsWith(`${root}/`) || path.startsWith(`${root}\\`)) {
    return path.slice(root.length + 1);
  }
  return path;
}

function Counts({ added, removed }: { added: number; removed: number }) {
  return (
    <span className="shrink-0 font-mono tabular-nums">
      <span className="text-status-open">+{added}</span>{" "}
      <span className="text-status-attention">−{removed}</span>
    </span>
  );
}

/**
 * Closes a settled turn with the answer to "what did it touch?": a file count
 * and summed line counts, and a hover list per file. A click opens the turn's
 * real git diff when the turn is known, else the workspace's Changes pane.
 */
export function TurnChangesChip({
  changes,
  workspaceId,
  cwd,
  threadId,
  turnId,
}: {
  changes: TurnChangeSummary;
  workspaceId?: string | null;
  cwd?: string | null;
  threadId?: string | null;
  turnId?: string | null;
}) {
  const [diffOpen, setDiffOpen] = useState(false);
  const [listOpen, setListOpen] = useState(false);
  const chipRef = useRef<HTMLButtonElement>(null);
  const count = changes.files.length;
  const label = `${count} ${count === 1 ? "file" : "files"} changed`;
  const showsDiff = Boolean(threadId && turnId);
  return (
    <>
    <HoverCard
      openDelay={300}
      closeDelay={100}
      // Never left floating above the diff dialog the chip just opened.
      open={listOpen && !diffOpen}
      onOpenChange={setListOpen}
    >
      <HoverCardTrigger asChild>
        <button
          ref={chipRef}
          type="button"
          onClick={() => {
            if (showsDiff) {
              setListOpen(false);
              setDiffOpen(true);
            } else if (workspaceId) {
              useUIStore.getState().setRightPanelTab(workspaceId, "changes");
            }
          }}
          aria-label={`${label}, ${changes.added} lines added, ${changes.removed} removed. ${showsDiff ? "Show this turn's diff" : "Open changes"}`}
          className="inline-flex min-w-0 items-center gap-1.5 rounded-sm px-1 text-label text-muted-foreground transition-colors duration-100 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/60"
          data-testid="turn-changes-chip"
        >
          <FileDiff className="size-3 shrink-0" aria-hidden />
          <span className="truncate">{label}</span>
          <Counts added={changes.added} removed={changes.removed} />
        </button>
      </HoverCardTrigger>
      <HoverCardContent
        align="end"
        entrance="subtle"
        className="w-80 p-2 motion-reduce:animate-none"
        data-testid="turn-changes-list"
      >
        <Eyebrow className="mb-1.5 px-1">Changed in this turn</Eyebrow>
        <ul className="thin-scrollbar max-h-60 overflow-y-auto">
          {changes.files.map((file) => {
            const shown = displayPath(file.path, cwd);
            return (
              <li
                key={file.path}
                className="flex items-center justify-between gap-3 rounded-sm px-1 py-0.5 text-caption"
              >
                {/* Truncate from the left so the file name stays visible. */}
                <span
                  className="min-w-0 truncate font-mono text-foreground [direction:rtl]"
                  title={file.path}
                >
                  <bdi>{shown}</bdi>
                </span>
                <Counts added={file.added} removed={file.removed} />
              </li>
            );
          })}
        </ul>
      </HoverCardContent>
    </HoverCard>
    {showsDiff && threadId && turnId ? (
      <TurnDiffDialog
        open={diffOpen}
        onOpenChange={setDiffOpen}
        threadId={threadId}
        turnId={turnId}
        workspaceId={workspaceId}
        returnFocusRef={chipRef}
      />
    ) : null}
    </>
  );
}
