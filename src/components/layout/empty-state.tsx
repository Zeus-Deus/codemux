import { LocalSessionImportEntry } from "@/components/chat/LocalSessionImportEntry";
import { FolderOpen, GitBranch, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { MenuKeycap } from "@/components/ui/menu-chrome";
import { CloneDialog } from "@/components/overlays/clone-dialog";
import { WindowChrome } from "@/components/layout/window-chrome";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useUIStore } from "@/stores/ui-store";
import wordmark from "@/assets/codemux-wordmark.svg";
import { RecentProjectList, useRecentProjects } from "./recent-projects";

export function EmptyState() {
  const { openProject, openCloneDialog } = useProjectActions();
  const setShowNewProjectScreen = useUIStore(
    (s) => s.setShowNewProjectScreen,
  );
  const recentProjects = useRecentProjects(true);

  return (
    <div className="fixed inset-0 z-50 flex flex-col items-center overflow-y-auto bg-background">
      <WindowChrome />
      {/* `my-auto` centres while it fits and scrolls from the top when the
          window is too short, where flex centring would clip the top. */}
      <div className="my-auto flex w-full max-w-[432px] flex-col items-center px-4 py-12">
        {/* Wordmark */}
        <img
          src={wordmark}
          alt=""
          className="h-[72px] w-auto select-none opacity-80 mb-10"
          draggable={false}
        />

        {/* Open project card */}
        <button
          type="button"
          onClick={() => openProject()}
          className="w-full rounded-lg border-2 border-dashed border-border/60 bg-card/50 px-6 py-10 text-center transition-colors duration-150 hover:border-foreground/30 hover:bg-card"
        >
          <span className="mx-auto mb-3 flex size-9 items-center justify-center rounded-md bg-surface-2 text-muted-foreground">
            <FolderOpen className="size-4" />
          </span>
          <div className="text-body font-medium text-foreground">
            Open Project
          </div>
          <div className="text-label text-muted-foreground mt-1">
            Open a local folder with your code
          </div>
          <MenuKeycap actionId="openProject" className="mt-3 ml-0 inline-block" />
        </button>

        <RecentProjectList projects={recentProjects} className="mt-6" />

        {/* Other ways in */}
        <div className="mt-6 flex flex-col items-center gap-2">
          <span className="text-label text-muted-foreground/60">
            Or start from somewhere else
          </span>
          <div className="flex flex-wrap items-center justify-center gap-1">
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-foreground"
              onClick={openCloneDialog}
            >
              <GitBranch />
              Clone repository
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="text-muted-foreground hover:text-foreground"
              onClick={() => setShowNewProjectScreen(true)}
            >
              <Plus />
              New Project
            </Button>
          </div>
        </div>

        <div className="mt-6"><LocalSessionImportEntry firstRun /></div>
      </div>
      {/* The sidebar normally hosts this dialog; this page renders without it. */}
      <CloneDialog />
    </div>
  );
}
