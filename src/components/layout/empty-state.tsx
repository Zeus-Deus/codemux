import { FolderOpen, FolderPlus } from "lucide-react";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useUIStore } from "@/stores/ui-store";
import logomark from "@/assets/codemux-logomark.svg";

function ActionCard({
  icon,
  title,
  description,
  onClick,
}: {
  icon: React.ReactNode;
  title: string;
  description: string;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="flex w-full items-center gap-4 rounded-xl border-2 border-dashed border-border/60 bg-card/50 px-5 py-4 text-left transition-all hover:border-primary/40 hover:bg-card"
    >
      <div className="rounded-lg bg-muted p-2.5 text-muted-foreground">
        {icon}
      </div>
      <div>
        <div className="text-sm font-medium text-foreground">{title}</div>
        <div className="text-xs text-muted-foreground">{description}</div>
      </div>
    </button>
  );
}

export function EmptyState() {
  const { openProject } = useProjectActions();
  const setShowNewProjectScreen = useUIStore(
    (s) => s.setShowNewProjectScreen,
  );

  return (
    <div className="flex h-full items-center justify-center px-6">
      <div className="w-full max-w-sm space-y-6">
        <div className="flex flex-col items-center gap-4">
          <img
            src={logomark}
            alt=""
            className="h-12 w-auto select-none opacity-25"
            draggable={false}
          />
          <p className="text-sm text-muted-foreground">
            Open a project to get started
          </p>
        </div>

        <div className="space-y-3">
          <ActionCard
            icon={<FolderOpen className="h-5 w-5" />}
            title="Open existing project"
            description="Open a local folder with your code"
            onClick={() => openProject()}
          />
          <ActionCard
            icon={<FolderPlus className="h-5 w-5" />}
            title="New project"
            description="Create a new repo or clone from URL"
            onClick={() => setShowNewProjectScreen(true)}
          />
        </div>
      </div>
    </div>
  );
}
