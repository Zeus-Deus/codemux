import { useState, useEffect, useMemo } from "react";
import { X } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useAppStore, useHomeDir } from "@/stores/app-store";
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
  const appState = useAppStore((s) => s.appState);
  const homeDir = useHomeDir();
  const setShowSettings = useUIStore((s) => s.setShowSettings);

  const [dismissed, setDismissed] = useState(true); // default hidden
  const [hasScripts, setHasScripts] = useState(true); // default hidden

  const activeWorkspace = useMemo(() => {
    if (!appState) return null;
    return appState.workspaces.find(
      (w) => w.workspace_id === appState.active_workspace_id,
    );
  }, [appState]);

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
    if (!appState || !projectRoot) return false;
    return appState.workspaces.some(
      (w) => w.project_root === projectRoot || w.cwd === projectRoot,
    );
  }, [appState, projectRoot]);

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

  return (
    <div className="shrink-0">
      <div className="px-3 py-2">
        <div className="rounded-md border border-border bg-muted/30 px-3 pt-2.5 pb-3">
          <div className="flex items-start justify-between mb-1.5">
            <Badge variant="outline" className="text-[9px] px-1.5 py-0 h-4 uppercase tracking-wider font-semibold">
              Setup
            </Badge>
            <Button
              variant="ghost"
              size="icon-xs"
              className="shrink-0 h-5 w-5 -mr-1 -mt-0.5 text-muted-foreground hover:text-foreground"
              onClick={handleDismiss}
            >
              <X className="h-3 w-3" />
            </Button>
          </div>
          <p className="text-xs font-semibold text-foreground">Setup scripts</p>
          <p className="text-[11px] text-muted-foreground mt-0.5">
            Automate workspace setup for {projectName} with setup scripts
            and <code className="font-mono text-[10px] bg-muted/50 px-0.5 rounded">.codemuxinclude</code>
          </p>
          <Button
            variant="outline"
            size="sm"
            className="w-full mt-2.5 h-7 text-xs"
            onClick={handleConfigure}
          >
            Configure
          </Button>
        </div>
      </div>
    </div>
  );
}
