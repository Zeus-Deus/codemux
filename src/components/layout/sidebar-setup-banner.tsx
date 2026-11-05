import { useState, useEffect, useMemo } from "react";
import { Wand2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { selectActiveWorkspaceId, useAppStore, useHomeDir } from "@/stores/app-store";
import { basename } from "@/lib/path";
import { useUIStore } from "@/stores/ui-store";
import { useSidebar } from "@/components/ui/sidebar";
import {
  dbGetUiState,
  dbSetUiState,
  getProjectScripts,
  getWorkspaceConfig,
} from "@/tauri/commands";

export function SidebarSetupBanner() {
  const { state } = useSidebar();
  // Narrow subscriptions, not the whole snapshot: the banner only needs the
  // active workspace's project root and the workspace list, and `appState`
  // itself moves on every commit — which meant re-running the project-root
  // derivation (and its effects' dependency checks) on every backend tick.
  const workspaces = useAppStore((s) => s.appState?.workspaces);
  const activeWorkspaceId = useAppStore((s) => selectActiveWorkspaceId(s));
  const homeDir = useHomeDir();
  const setShowSettings = useUIStore((s) => s.setShowSettings);

  const [dismissed, setDismissed] = useState(true); // default hidden
  const [hasScripts, setHasScripts] = useState(true); // default hidden

  const activeWorkspace = useMemo(() => {
    if (!workspaces) return null;
    return workspaces.find((w) => w.workspace_id === activeWorkspaceId);
  }, [workspaces, activeWorkspaceId]);

  // A home-rooted workspace (`project_root === $HOME`) is the
  // materialise target for a sidebar "New agent" chat that the user
  // intentionally kept in the home directory. It is NOT a project, so
  // any UI keyed on "this is a project" should treat it as project-less.
  // Mirrors the same `(project_root ?? cwd) !== homeDir` distinction
  // used by `useEnsureDraftWhenEmpty`.
  const rawProjectRoot = activeWorkspace?.project_root ?? null;
  const projectRoot =
    rawProjectRoot !== null && homeDir !== null && rawProjectRoot === homeDir
      ? null
      : rawProjectRoot;
  const projectName = projectRoot ? basename(projectRoot) || "project" : null;

  // Check if there are any workspaces for this project
  const hasProjectWorkspaces = useMemo(() => {
    if (!workspaces || !projectRoot) return false;
    return workspaces.some(
      (w) => w.project_root === projectRoot || w.cwd === projectRoot,
    );
  }, [workspaces, projectRoot]);

  // Check dismiss state and scripts
  useEffect(() => {
    if (!projectRoot) return;

    dbGetUiState(`setup-banner-dismissed:${projectRoot}`)
      .then((val) => setDismissed(val === "true"))
      .catch(() => setDismissed(false));

    // Check both file config and DB scripts
    Promise.all([
      getWorkspaceConfig(projectRoot),
      getProjectScripts(projectRoot),
    ])
      .then(([fileConfig, dbScripts]) => {
        const fileHasScripts =
          fileConfig &&
          (fileConfig.setup.length > 0 || fileConfig.teardown.length > 0);
        const dbHasScripts =
          dbScripts &&
          (dbScripts.setup.length > 0 || dbScripts.teardown.length > 0);
        setHasScripts(Boolean(fileHasScripts || dbHasScripts));
      })
      .catch(() => setHasScripts(true));
  }, [projectRoot]);

  // Don't show if: collapsed to the icon rail (the banner is a wide,
  // text-heavy card that has no place in a 52px rail), no project, already
  // dismissed, already has scripts, or no workspaces.
  if (
    state === "collapsed" ||
    !projectRoot ||
    dismissed ||
    hasScripts ||
    !hasProjectWorkspaces
  ) {
    return null;
  }

  const handleDismiss = () => {
    setDismissed(true);
    dbSetUiState(`setup-banner-dismissed:${projectRoot}`, "true").catch(
      console.error,
    );
  };

  const handleConfigure = () => {
    setShowSettings(true, "projects");
  };

  // One quiet row rather than a card: the tip is optional and stays until
  // dismissed, so it must not take a slab of the rail from the workspaces.
  return (
    <div className="shrink-0 px-2 py-1">
      <div className="group/setup flex h-8 items-center rounded-md transition-colors duration-150 hover:bg-surface-2">
        <button
          type="button"
          onClick={handleConfigure}
          title="Automate workspace setup with setup scripts and .codemuxinclude"
          className="flex h-full min-w-0 flex-1 items-center gap-2 rounded-md pl-2 text-left text-label text-muted-foreground transition-colors duration-150 group-hover/setup:text-foreground"
        >
          <Wand2 aria-hidden className="size-3.5 shrink-0" />
          <span className="truncate">Add setup scripts for {projectName}</span>
        </button>
        <Button
          variant="ghost"
          size="icon-xs"
          className="mr-1 shrink-0 text-subtle-foreground hover:text-foreground"
          onClick={handleDismiss}
          aria-label="Dismiss setup tip"
        >
          <X className="size-3" />
        </Button>
      </div>
    </div>
  );
}
