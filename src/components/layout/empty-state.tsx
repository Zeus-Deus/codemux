import { FolderOpen, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useUIStore } from "@/stores/ui-store";
import wordmark from "@/assets/codemux-wordmark.svg";

export function EmptyState() {
  const { openProject } = useProjectActions();
  const setShowNewProjectScreen = useUIStore(
    (s) => s.setShowNewProjectScreen,
  );

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-background">
      <div className="flex flex-col items-center">
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
          className="w-[400px] rounded-xl border-2 border-dashed border-border/60 bg-card/50 px-6 py-12 text-center transition-all hover:border-foreground/30 hover:bg-card"
        >
          <FolderOpen className="h-8 w-8 mx-auto mb-3 text-muted-foreground" />
          <div className="text-sm font-medium text-foreground">
            Open Project
          </div>
          <div className="text-xs text-muted-foreground mt-1">
            Open a local folder with your code
          </div>
        </button>

        {/* New project link */}
        <div className="mt-6 flex flex-col items-center gap-2">
          <span className="text-xs text-muted-foreground/60">
            Or start a new project
          </span>
          <Button
            variant="ghost"
            size="sm"
            className="text-xs text-muted-foreground hover:text-foreground"
            onClick={() => setShowNewProjectScreen(true)}
          >
            <Plus className="h-3.5 w-3.5 mr-1" />
            New Project
          </Button>
        </div>
      </div>
    </div>
  );
}
