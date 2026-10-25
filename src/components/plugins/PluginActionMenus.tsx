import { useRef, useState } from "react";
import { ChevronDown, Puzzle } from "lucide-react";
import { ContextMenuItem, ContextMenuLabel } from "@/components/ui/context-menu";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { MENU_ROW, MENU_SECTION_LABEL } from "@/components/ui/menu-chrome";
import { MESSAGE_ACTION_CLASS } from "@/components/chat/message-action";
import { cn } from "@/lib/utils";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { pluginActionEntries, pluginRunnableInstallations, type PluginActionEntry } from "./model";
import { PluginBoundary } from "./PluginBoundary";
import { capturePluginTarget, runPluginAction } from "./plugin-actions";
import { readPlatformCatalogue, usePlatformCatalogue } from "./platform-adapter";
import { pluginProjectIdOf, workspaceActiveThreadId, workspaceById } from "./workspace-context";

/** Longest selection a plugin receives; the contract bounds action context text. */
export const PLUGIN_SELECTION_MAX_LENGTH = 64_000;

/**
 * Thread-menu plugin actions in a sidebar row's context menu. Entries are
 * read when the menu opens (it mounts only while open), and the target is the
 * row's chat, wherever the user is when the action ends.
 */
export function PluginThreadMenuItems({ workspace }: { workspace: WorkspaceSnapshot }) {
  return (
    <PluginBoundary label="Plugin chat actions" fallback={null}>
      <ThreadMenuItems workspace={workspace} />
    </PluginBoundary>
  );
}

function ThreadMenuItems({ workspace }: { workspace: WorkspaceSnapshot }) {
  const threadId = workspaceActiveThreadId(workspace);
  const [entries] = useState(() =>
    threadId === null ? [] : pluginActionEntries(pluginRunnableInstallations(readPlatformCatalogue()), "thread-menu"),
  );
  if (threadId === null || entries.length === 0) return null;
  const run = ({ installation, action }: PluginActionEntry) => {
    const target = capturePluginTarget({ installation, workspaceId: workspace.workspace_id, threadId, composer: true });
    void runPluginAction(target, action, { projectId: pluginProjectIdOf(workspace), threadId });
  };
  return (
    <>
      <ContextMenuLabel className={cn(MENU_SECTION_LABEL, "mt-1")}>Plugins</ContextMenuLabel>
      {entries.map((entry) => (
        <ContextMenuItem
          key={`${entry.installation.installationId}:${entry.action.id}`}
          className={MENU_ROW}
          onClick={() => run(entry)}
          data-plugin-action={entry.action.id}
        >
          <Puzzle />
          <span className="min-w-0 flex-1 truncate">
            {entry.action.title}
            <span className="text-muted-foreground"> · {entry.installation.manifest.name}</span>
          </span>
        </ContextMenuItem>
      ))}
    </>
  );
}

/** The text selected inside `root`, if the selection lies within it. */
function selectionWithin(root: Element | null): string | null {
  if (root === null || typeof window.getSelection !== "function") return null;
  const selection = window.getSelection();
  if (selection === null || selection.isCollapsed || selection.rangeCount === 0) return null;
  const range = selection.getRangeAt(0);
  if (!root.contains(range.commonAncestorContainer)) return null;
  const text = selection.toString();
  return text.trim() === "" ? null : text;
}

/**
 * Message-selection plugin actions under an assistant reply. The selection
 * inside this reply is captured as the menu opens (else the whole reply);
 * the action's result lands only in this chat's composer.
 */
interface PluginMessageActionsProps {
  messageId: string;
  text: string;
  workspaceId: string | null;
  threadId: string | null;
}

export function PluginMessageActions(props: PluginMessageActionsProps) {
  return (
    <PluginBoundary label="Plugin reply actions" fallback={null}>
      <MessageActions {...props} />
    </PluginBoundary>
  );
}

function MessageActions({ messageId, text, workspaceId, threadId }: PluginMessageActionsProps) {
  const { catalogue } = usePlatformCatalogue();
  const entries = threadId === null ? [] : pluginActionEntries(pluginRunnableInstallations(catalogue), "message-selection");
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const [captured, setCaptured] = useState<{ text: string; partial: boolean } | null>(null);
  if (threadId === null || entries.length === 0) return null;
  const capture = () => {
    const selected = selectionWithin(triggerRef.current?.closest("[data-plugin-message-root]") ?? null);
    setCaptured(selected === null ? { text, partial: false } : { text: selected, partial: true });
  };
  const run = ({ installation, action }: PluginActionEntry) => {
    const selection = (captured?.text ?? text).slice(0, PLUGIN_SELECTION_MAX_LENGTH);
    const target = capturePluginTarget({ installation, workspaceId, threadId, composer: true });
    const workspace = workspaceById(workspaceId);
    void runPluginAction(target, action, {
      ...(workspace ? { projectId: pluginProjectIdOf(workspace) } : {}),
      threadId,
      messageId,
      selection,
    });
  };
  return (
    <DropdownMenu
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setCaptured(null);
      }}
    >
      <DropdownMenuTrigger asChild>
        <button
          ref={triggerRef}
          type="button"
          aria-label="Plugin actions"
          title="Plugin actions"
          data-testid="message-plugin-actions"
          // Before the press can collapse the reader's selection.
          onPointerDownCapture={capture}
          onKeyDownCapture={(event) => {
            if (event.key === "Enter" || event.key === " " || event.key === "ArrowDown") capture();
          }}
          className={cn(MESSAGE_ACTION_CLASS, open && "pointer-events-auto opacity-100")}
        >
          <Puzzle className="size-3" aria-hidden />
          <span className="pointer-coarse:sr-only">Plugins</span>
          <ChevronDown className="size-2.5 opacity-60" aria-hidden />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-60">
        <DropdownMenuLabel className="text-label font-normal text-muted-foreground">
          {captured?.partial ? "Uses the selected text" : "Uses the whole reply"}
        </DropdownMenuLabel>
        {entries.map((entry) => (
          <DropdownMenuItem
            key={`${entry.installation.installationId}:${entry.action.id}`}
            onSelect={() => run(entry)}
            data-plugin-action={entry.action.id}
          >
            <span className="min-w-0 flex-1 truncate">
              {entry.action.title}
              <span className="text-muted-foreground"> · {entry.installation.manifest.name}</span>
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
