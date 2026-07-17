import { useCallback } from "react";
import {
  Terminal,
  Globe,
  ExternalLink,
  Search,
  Trash2,
} from "lucide-react";
import { useAppStore } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import {
  createTab,
  createBrowserPane,
  openInEditor,
  detectEditors,
  closeWorkspace,
  closeWorkspaceWithWorktree,
} from "@/tauri/commands";
import { toast } from "@/lib/toast";
import { runDeleteWithForceToast } from "@/hooks/use-force-delete";
import logomark from "@/assets/codemux-logomark.svg";

function Kbd({ children }: { children: React.ReactNode }) {
  return (
    <kbd className="inline-flex h-5 min-w-5 items-center justify-center rounded-md border border-border bg-muted px-1.5 text-[10px] font-mono text-muted-foreground">
      {children}
    </kbd>
  );
}

interface ActionRowProps {
  icon: React.ReactNode;
  label: string;
  keys: string[];
  onClick: () => void;
}

function ActionRow({ icon, label, keys, onClick }: ActionRowProps) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="group flex w-full items-center gap-3 rounded-lg px-3 py-2.5 text-sm text-muted-foreground/80 transition-colors hover:bg-muted/60 hover:text-foreground"
    >
      <span className="rounded p-1 text-muted-foreground/70 transition-colors group-hover:text-foreground">
        {icon}
      </span>
      <span className="flex-1 text-left">{label}</span>
      <span className="flex items-center gap-1">
        {keys.map((k) => (
          <Kbd key={k}>{k}</Kbd>
        ))}
      </span>
    </button>
  );
}

export function EmptyWorkspaceState() {
  const appState = useAppStore((s) => s.appState);
  const ws = appState?.workspaces.find(
    (w) => w.workspace_id === appState.active_workspace_id,
  );
  const setShowFileSearch = useUIStore((s) => s.setShowFileSearch);

  const workspaceId = ws?.workspace_id;

  const handleOpenTerminal = useCallback(() => {
    if (workspaceId) createTab(workspaceId, "terminal").catch(console.error);
  }, [workspaceId]);

  const handleOpenBrowser = useCallback(() => {
    if (!ws) return;
    const surface = ws.surfaces.find(
      (s) => s.surface_id === ws.active_surface_id,
    );
    if (surface) {
      createBrowserPane(surface.active_pane_id).catch(console.error);
    }
  }, [ws]);

  const handleOpenInEditor = useCallback(async () => {
    if (!ws) return;
    const editors = await detectEditors().catch(() => []);
    if (editors.length > 0) {
      openInEditor(editors[0].id, ws.cwd).catch(console.error);
    }
  }, [ws]);

  const handleSearchFiles = useCallback(() => {
    setShowFileSearch(true);
  }, [setShowFileSearch]);

  // A per-branch worktree can have its files removed; a repo root
  // (protected) or a non-worktree workspace (primary checkout, plain
  // folder, Home — `worktree_path` null) can only be closed. This mirrors
  // `canRemoveWorktree` in the workspaces-overview row so the two delete
  // affordances stay in lockstep with the backend's removal guard.
  const isWorktree = !!ws?.worktree_path;
  const isRepoRoot = ws?.protected === true;
  const canRemoveWorktree = isWorktree && !isRepoRoot;

  const handleDeleteWorkspace = useCallback(() => {
    if (!workspaceId) return;
    const title = ws?.title || "this workspace";
    const confirmed = window.confirm(
      canRemoveWorktree
        ? `Delete the worktree "${title}"? Files on disk will be removed.`
        : `Close "${title}"? Files on disk are left untouched.`,
    );
    if (!confirmed) return;
    if (canRemoveWorktree) {
      // Worktree removal: the first attempt goes out non-forced. A dirty
      // worktree now REFUSES before the workspace is closed; the helper
      // toasts that message and its "Force delete" action reissues with
      // forceDelete=true. Any other rejection is a plain error toast.
      void runDeleteWithForceToast({
        run: (force) =>
          closeWorkspaceWithWorktree(workspaceId, true, false, force),
      });
    } else {
      // No worktree of its own — a destructive close would be refused by
      // the backend guard and toast a misleading error. Close
      // (non-destructive) so the workspace disappears while its files
      // stay on disk.
      closeWorkspace(workspaceId, false).catch((err) =>
        toast.error("Close failed", {
          description: err instanceof Error ? err.message : String(err),
        }),
      );
    }
  }, [workspaceId, ws?.title, canRemoveWorktree]);

  return (
    <div className="flex h-full flex-1 items-center justify-center px-6 py-10">
      <div className="w-full max-w-sm">
        {/* Logomark */}
        <div className="mb-8 flex items-center justify-center">
          <img
            src={logomark}
            alt=""
            className="h-12 w-auto select-none opacity-25"
            draggable={false}
          />
        </div>

        {/* Action rows */}
        <div className="mx-auto grid w-full gap-0.5">
          <ActionRow
            icon={<Terminal className="h-4 w-4" />}
            label="Open Terminal"
            keys={["Ctrl", "T"]}
            onClick={handleOpenTerminal}
          />
          <ActionRow
            icon={<Globe className="h-4 w-4" />}
            label="Open Browser"
            keys={["Ctrl", "Alt", "B"]}
            onClick={handleOpenBrowser}
          />
          <ActionRow
            icon={<ExternalLink className="h-4 w-4" />}
            label="Open in Editor"
            keys={["Ctrl", "Shift", "E"]}
            onClick={handleOpenInEditor}
          />
          <ActionRow
            icon={<Search className="h-4 w-4" />}
            label="Search Files"
            keys={["Ctrl", "Shift", "P"]}
            onClick={handleSearchFiles}
          />
        </div>

        {/* Delete / close workspace — the label matches the action the
            backend will actually take: only a per-branch worktree is
            deleted (files removed); everything else is closed. */}
        <div className="mt-8 flex justify-center">
          <button
            type="button"
            onClick={handleDeleteWorkspace}
            className="flex items-center gap-1.5 text-xs text-muted-foreground/40 transition-colors hover:text-muted-foreground"
          >
            <Trash2 className="h-3 w-3" />
            {canRemoveWorktree ? "Delete workspace" : "Close workspace"}
          </button>
        </div>
      </div>
    </div>
  );
}
