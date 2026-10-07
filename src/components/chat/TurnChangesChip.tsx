import { FileDiff } from "lucide-react";

import { Eyebrow } from "@/components/ui/eyebrow";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import { useUIStore } from "@/stores/ui-store";

import type { TurnChangeSummary } from "./turn-changes";

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
 * and summed line counts, a hover list per file, and a click through to the
 * workspace's Changes pane.
 */
export function TurnChangesChip({
  changes,
  workspaceId,
  cwd,
}: {
  changes: TurnChangeSummary;
  workspaceId?: string | null;
  cwd?: string | null;
}) {
  const count = changes.files.length;
  const label = `${count} ${count === 1 ? "file" : "files"} changed`;
  return (
    <HoverCard openDelay={300} closeDelay={100}>
      <HoverCardTrigger asChild>
        <button
          type="button"
          onClick={() => {
            if (workspaceId) {
              useUIStore.getState().setRightPanelTab(workspaceId, "changes");
            }
          }}
          aria-label={`${label}, ${changes.added} lines added, ${changes.removed} removed. Open changes`}
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
  );
}
