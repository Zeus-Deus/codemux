import type { ReactNode } from "react";
import { FolderOpen, GitBranch, Plus } from "lucide-react";
import { MenuKeycap } from "@/components/ui/menu-chrome";
import {
  RecentProjectList,
  describeError,
  useRecentProjects,
} from "@/components/layout/recent-projects";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { openDroppedFolder, useProjectActions } from "@/hooks/use-project-actions";
import { useFolderDrop } from "@/hooks/use-folder-drop";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useUIStore } from "@/stores/ui-store";
import { CHAT_COLUMN } from "./chat-column";

/**
 * The first-run ways to get a project: three equal tiles under the composer
 * (Open folder, Clone repository, New project), then recent projects once
 * there are some. A folder dropped anywhere on the window opens it.
 */
export function FirstProjectActions({
  onProjectOpened,
}: {
  onProjectOpened?: (projectPath: string) => void;
}) {
  const { openProject, openAddProject } = useProjectActions();
  const recentProjects = useRecentProjects(true);
  const dragging = useFolderDrop((path) => {
    // The Add project sheet takes drops itself while it is open.
    if (useUIStore.getState().addProjectSheet) return;
    void openDroppedFolder(path).then((opened) => {
      if (opened) onProjectOpened?.(path);
    });
  });

  const handleOpenProject = async () => {
    try {
      const result = await openProject();
      if (result.success && result.path) onProjectOpened?.(result.path);
    } catch (err) {
      toast.error(`Couldn't open the project: ${describeError(err)}`);
    }
  };

  return (
    <div className={cn(CHAT_COLUMN, "flex flex-col gap-6")}>
      <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
        <ProjectTile
          icon={<FolderOpen />}
          label="Open folder"
          hint={
            dragging
              ? "Drop it to open it"
              : isRemoteClient()
                ? "Browse the host's folders"
                : "or drop one anywhere"
          }
          keycap={<MenuKeycap actionId="openProject" />}
          highlighted={dragging}
          onClick={() => void handleOpenProject()}
        />
        <ProjectTile
          icon={<GitBranch />}
          label="Clone repository"
          hint="GitHub or any git URL"
          onClick={() => openAddProject("clone")}
        />
        <ProjectTile
          icon={<Plus />}
          label="New project"
          hint="Empty, or scaffolded by an agent"
          onClick={() => openAddProject("new")}
        />
      </div>
      <RecentProjectList
        projects={recentProjects}
        onOpened={onProjectOpened}
      />
    </div>
  );
}

function ProjectTile({
  icon,
  label,
  hint,
  keycap,
  highlighted = false,
  onClick,
}: {
  icon: ReactNode;
  label: string;
  hint: string;
  keycap?: ReactNode;
  highlighted?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className={cn(
        "flex min-w-0 flex-col items-start rounded-lg border border-border px-3 py-3 text-left transition-colors duration-150 hover:bg-surface-2",
        highlighted && "border-foreground/30 bg-surface-2",
      )}
    >
      <span className="flex w-full items-center justify-between gap-2 text-muted-foreground [&_svg]:size-4">
        {icon}
        {keycap}
      </span>
      <span className="mt-2.5 w-full truncate text-body text-foreground">{label}</span>
      <span className="w-full truncate text-label text-muted-foreground">{hint}</span>
    </button>
  );
}
