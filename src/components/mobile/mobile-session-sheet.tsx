import { useEffect, useRef, useState, type RefObject } from "react";
import {
  Check,
  FileCode,
  GitCompare,
  Globe,
  MessageSquare,
  Terminal,
  X,
} from "lucide-react";
import { AgentLauncher } from "@/components/layout/agent-launcher";
import { leafPanes } from "@/components/layout/mobile-pane-container";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { getHighestPriorityStatus, STATUS_LABEL } from "@/lib/pane-status";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useAppStore } from "@/stores/app-store";
import { useEditorStore } from "@/stores/editor-store";
import { useUIStore } from "@/stores/ui-store";
import { activatePane, activateTab, closeTab } from "@/tauri/commands";
import type { PaneNodeSnapshot, WorkspaceSnapshot } from "@/tauri/types";

interface Props {
  workspace: WorkspaceSnapshot;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  returnFocusRef?: RefObject<HTMLElement | null>;
}

function SessionIcon({ kind }: { kind: string }) {
  const Icon =
    kind === "agent_chat"
      ? MessageSquare
      : kind === "browser"
        ? Globe
        : kind === "editor"
          ? FileCode
          : kind === "diff"
            ? GitCompare
            : Terminal;
  return (
    <Icon size={18} className="shrink-0 text-muted-foreground" aria-hidden />
  );
}

/** Tabs and split leaves share one picker instead of consuming two phone rows. */
export function MobileSessionSheet({
  workspace,
  open,
  onOpenChange,
  returnFocusRef,
}: Props) {
  const statuses = useAppStore((s) => s.appState?.pane_statuses);
  const editorTabs = useEditorStore((s) => s.tabs);
  const [busy, setBusy] = useState(false);
  const pending = useRef(false);
  const opener = useRef<HTMLElement | null>(null);
  const previousTab = useRef(workspace.active_tab_id);
  useEffect(() => {
    const changed = previousTab.current !== workspace.active_tab_id;
    previousTab.current = workspace.active_tab_id;
    // The launcher opens a new tab. Our own selection closes only after
    // its awaited pane activation, rather than midway through that work.
    if (open && changed && !pending.current) onOpenChange(false);
  }, [workspace.active_tab_id, open, onOpenChange]);

  const select = async (tabId: string, paneId?: string) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      // Tab activation hydrates its surface before selecting a split leaf.
      await activateTab(workspace.workspace_id, tabId);
      if (paneId) await activatePane(paneId);
      useUIStore.getState().setRightPanelTab(workspace.workspace_id, null);
      onOpenChange(false);
    } catch (error) {
      toast.error("Could not open session", { description: String(error) });
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  const close = async (tabId: string) => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    try {
      await closeTab(workspace.workspace_id, tabId);
    } catch (error) {
      toast.error("Could not close session", { description: String(error) });
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent
        side="bottom"
        className="mobile-bottom-sheet"
        onOpenAutoFocus={() => {
          opener.current =
            document.activeElement instanceof HTMLElement
              ? document.activeElement
              : null;
        }}
        onCloseAutoFocus={(event) => {
          const target = returnFocusRef?.current ?? opener.current;
          if (target?.isConnected) {
            event.preventDefault();
            target.focus({ preventScroll: true });
          }
        }}
      >
        <SheetHeader className="pr-14">
          <SheetTitle>Sessions</SheetTitle>
          <SheetDescription className="truncate">
            {workspace.title}
          </SheetDescription>
        </SheetHeader>
        <div
          className="min-h-0 overflow-y-auto px-3 pb-2"
          aria-label="Workspace sessions"
          aria-busy={busy}
        >
          {workspace.tabs.map((tab) => {
            const surface = workspace.surfaces.find(
              (s) => s.surface_id === tab.surface_id,
            );
            const leaves = surface ? leafPanes(surface.root) : [];
            const selectedPane =
              leaves.find((p) => p.pane_id === surface?.active_pane_id) ??
              leaves[0];
            const active = tab.tab_id === workspace.active_tab_id;
            const status = getHighestPriorityStatus(
              leaves.map((p) => statuses?.[p.pane_id]),
            );
            const dirty =
              tab.kind === "editor" && editorTabs[tab.tab_id]?.isDirty;
            const row = (pane?: PaneNodeSnapshot) => {
              const paneStatus = pane
                ? getHighestPriorityStatus([statuses?.[pane.pane_id]])
                : status;
              const title =
                pane && pane.kind !== "split" ? pane.title : tab.title;
              const isSelected =
                active && (!pane || pane.pane_id === selectedPane?.pane_id);
              return (
                <button
                  type="button"
                  className={cn(
                    "flex min-h-12 min-w-0 flex-1 items-center gap-3 rounded-xl px-3 py-2 text-left",
                    isSelected && "bg-secondary",
                  )}
                  aria-label={`Open ${title}`}
                  aria-current={isSelected ? "true" : undefined}
                  disabled={busy}
                  onClick={() =>
                    void select(
                      tab.tab_id,
                      pane?.pane_id ??
                        (leaves.length === 1
                          ? selectedPane?.pane_id
                          : undefined),
                    )
                  }
                >
                  <SessionIcon
                    kind={
                      pane?.kind ??
                      (leaves.length === 1 ? leaves[0].kind : tab.kind)
                    }
                  />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-medium">{title}</span>
                    {(paneStatus || dirty) && (
                      <span className="block text-xs text-muted-foreground">
                        {[
                          paneStatus && STATUS_LABEL[paneStatus],
                          dirty && "Unsaved changes",
                        ]
                          .filter(Boolean)
                          .join(" · ")}
                      </span>
                    )}
                  </span>
                  {isSelected && (
                    <Check size={16} className="shrink-0" aria-hidden />
                  )}
                </button>
              );
            };
            return (
              <div key={tab.tab_id} className="mb-1">
                <div className="flex min-w-0 items-center gap-1">
                  {row()}
                  <button
                    type="button"
                    className="flex size-11 shrink-0 items-center justify-center rounded-xl text-muted-foreground hover:bg-muted"
                    aria-label={`Close ${tab.title}`}
                    disabled={busy}
                    onClick={() => void close(tab.tab_id)}
                  >
                    <X size={17} aria-hidden />
                  </button>
                </div>
                {leaves.length > 1 && (
                  <div className="ml-5 border-l border-border pl-2">
                    {leaves.map((pane) => (
                      <div key={pane.pane_id} className="flex min-w-0">
                        {row(pane)}
                      </div>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
          {workspace.tabs.length === 0 && (
            <p className="px-3 py-4 text-muted-foreground">No sessions yet.</p>
          )}
        </div>
        <div className="mx-4 flex items-center justify-between border-t border-border pt-2 pb-4">
          <span className="font-medium">Add chat, terminal or browser</span>
          <AgentLauncher workspace={workspace} mobile />
        </div>
      </SheetContent>
    </Sheet>
  );
}
