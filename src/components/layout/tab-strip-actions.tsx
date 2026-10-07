import { useRef, useState } from "react";
import { flushSync } from "react-dom";

import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu";
import { cn } from "@/lib/utils";
import { useEditorStore } from "@/stores/editor-store";
import { closeTab, renameTab, splitPane } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

/**
 * Pieces shared by the workspace tab strips (the GUI title bar and the
 * legacy terminal `TabBar`), so both offer the same menu, the same inline
 * rename and the same unsaved-changes cue.
 */

interface TabContextMenuProps {
  workspace: WorkspaceSnapshot;
  tabId: string;
  onRename: () => void;
  /** The tab element itself; must accept a ref and spread props. */
  children: React.ReactNode;
}

export function TabContextMenu({
  workspace,
  tabId,
  onRename,
  children,
}: TabContextMenuProps) {
  const workspaceId = workspace.workspace_id;
  const index = workspace.tabs.findIndex((t) => t.tab_id === tabId);
  const renamePendingRef = useRef(false);

  // Closes run one at a time: each close mutates the backend's tab list,
  // and firing them concurrently races those mutations.
  const closeAll = async (ids: string[]) => {
    for (const id of ids) {
      await closeTab(workspaceId, id).catch(console.error);
    }
  };

  const handleSplit = (direction: "horizontal" | "vertical") => {
    const surface = workspace.surfaces.find(
      (s) => s.surface_id === workspace.active_surface_id,
    );
    if (surface) {
      splitPane(surface.active_pane_id, direction).catch(console.error);
    }
  };

  return (
    <ContextMenu>
      <ContextMenuTrigger asChild>{children}</ContextMenuTrigger>
      <ContextMenuContent
        // Rename starts only once the menu has fully closed: the label's
        // input focuses on mount, and the open menu's focus trap (or the
        // focus hand-back to the trigger) would otherwise blur it at once.
        onCloseAutoFocus={(e) => {
          if (!renamePendingRef.current) return;
          renamePendingRef.current = false;
          e.preventDefault();
          onRename();
        }}
      >
        <ContextMenuItem onSelect={() => void closeAll([tabId])}>
          Close tab
        </ContextMenuItem>
        <ContextMenuItem
          onSelect={() =>
            void closeAll(
              workspace.tabs.map((t) => t.tab_id).filter((id) => id !== tabId),
            )
          }
          disabled={workspace.tabs.length <= 1}
        >
          Close other tabs
        </ContextMenuItem>
        <ContextMenuItem
          onSelect={() =>
            void closeAll(
              workspace.tabs
                .slice(index + 1)
                .map((t) => t.tab_id)
                .reverse(),
            )
          }
          disabled={index < 0 || index >= workspace.tabs.length - 1}
        >
          Close tabs to the right
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem onSelect={() => handleSplit("horizontal")}>
          Split right
        </ContextMenuItem>
        <ContextMenuItem onSelect={() => handleSplit("vertical")}>
          Split down
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          onSelect={() => {
            renamePendingRef.current = true;
          }}
        >
          Rename tab
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/** Per-tab inline-rename state: `start` swaps the label for a
 *  `TabTitleInput`, which calls `stop` when it commits or cancels. */
export function useTabRename() {
  const [renaming, setRenaming] = useState(false);
  return {
    renaming,
    start: () => setRenaming(true),
    stop: () => setRenaming(false),
  };
}

interface TabTitleInputProps {
  workspaceId: string;
  tabId: string;
  title: string;
  onDone: () => void;
  className?: string;
}

/**
 * Inline replacement for a tab's label while renaming. Enter or blur
 * commits, Escape cancels; an empty or unchanged title is a no-op.
 */
export function TabTitleInput({
  workspaceId,
  tabId,
  title,
  onDone,
  className,
}: TabTitleInputProps) {
  const [value, setValue] = useState(title);
  // Enter commits and then unmounts the input, which fires blur; this keeps
  // that blur from committing a second time (or after an Escape).
  const settledRef = useRef(false);

  const finish = (commit: boolean, refocusFrom?: HTMLElement) => {
    if (settledRef.current) return;
    settledRef.current = true;
    const next = value.trim();
    if (commit && next && next !== title) {
      renameTab(workspaceId, tabId, next).catch(console.error);
    }
    if (!refocusFrom) {
      onDone();
      return;
    }
    // A keyboard finish hands focus back to the tab's label, which replaces
    // this input on the same tab element; otherwise focus falls to <body>
    // and a keyboard-started rename loses the user's place.
    const tab = refocusFrom.closest<HTMLElement>("[data-tab-id]");
    flushSync(onDone);
    tab?.querySelector<HTMLElement>("button")?.focus();
  };

  return (
    <input
      autoFocus
      // Pointer presses here select text; they must not start a tab drag.
      data-no-drag
      aria-label="Tab name"
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onFocus={(e) => e.currentTarget.select()}
      onBlur={() => finish(true)}
      onKeyDown={(e) => {
        // Keep the strip's and the app's shortcuts out of the edit.
        e.stopPropagation();
        // Enter/Escape during IME composition confirm or cancel the
        // composition, not the rename.
        if (e.nativeEvent.isComposing) return;
        if (e.key === "Enter") {
          e.preventDefault();
          finish(true, e.currentTarget);
        } else if (e.key === "Escape") {
          e.preventDefault();
          finish(false, e.currentTarget);
        }
      }}
      className={cn(
        "h-5 w-[130px] min-w-0 rounded-sm border border-input bg-background px-1 text-label text-foreground outline-none focus-visible:ring-2 focus-visible:ring-ring/60",
        className,
      )}
    />
  );
}

/** Unsaved-changes cue for editor tabs. */
export function EditorDirtyDot({ tabId }: { tabId: string }) {
  const isDirty = useEditorStore((s) => s.getTab(tabId)?.isDirty ?? false);
  if (!isDirty) return null;
  return (
    <span
      className="size-1.5 shrink-0 rounded-full bg-foreground/50"
      title="Unsaved changes"
      aria-label="Unsaved changes"
      role="img"
    />
  );
}

/** Middle-click closes a tab, as in every browser and editor tab strip. The
 *  mousedown is cancelled too, so a middle press on an overflowing strip
 *  doesn't also engage the webview's autoscroll. */
export function middleClickCloseProps(onClose: () => void) {
  return {
    onMouseDown: (e: React.MouseEvent) => {
      if (e.button === 1) e.preventDefault();
    },
    onAuxClick: (e: React.MouseEvent) => {
      if (e.button !== 1) return;
      e.preventDefault();
      onClose();
    },
  };
}
