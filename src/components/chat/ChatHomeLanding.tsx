import type { ReactNode } from "react";
import { FolderOpen, GitBranch } from "lucide-react";
import { Button } from "@/components/ui/button";
import { MenuKeycap } from "@/components/ui/menu-chrome";
import {
  RecentProjectList,
  describeError,
  useRecentProjects,
} from "@/components/layout/recent-projects";
import { toast } from "@/lib/toast";
import { useProjectActions } from "@/hooks/use-project-actions";
import { useAppStore } from "@/stores/app-store";
import { LocalSessionImportEntry } from "./LocalSessionImportEntry";

interface Props {
  composer: ReactNode;
  /** Optional line above the composer (e.g. the Full access notice). */
  notice?: ReactNode;
  /** A project was opened from the zero-project actions below. */
  onProjectOpened?: (projectPath: string) => void;
}

/**
 * Empty-state landing for the chat surface. Per the chat-ui skill,
 * this is the sole place inside the chat feature that may exceed
 * prose size — one marquee headline above the composer, no grid of
 * example prompts, no marketing copy. The composer is the invitation.
 *
 * With no project open yet, a quiet row of project actions sits under the
 * composer: this landing replaces the full-screen Open Project page under
 * lazy workspace creation, so it has to offer the same ways in.
 */
export function ChatHomeLanding({ composer, notice, onProjectOpened }: Props) {
  const noWorkspaces = useAppStore(
    (s) => s.appState !== null && (s.appState.workspaces?.length ?? 0) === 0,
  );
  return (
    // `my-auto` on the column centres it while it fits and lets it scroll
    // from the top in a short pane, where flex centring would clip the
    // headline off the top.
    <div className="flex h-full w-full flex-col items-center overflow-y-auto">
      <div className="my-auto flex w-full flex-col items-center gap-8 pt-6 pb-12">
        <h1 className="px-4 text-3xl font-medium tracking-tight text-foreground text-center">
          What should we do today?
        </h1>
        {/* The project actions sit closer to the composer than the
            headline does, so they read as part of the same start step. */}
        <div className="flex w-full flex-col items-center gap-4">
          {/* The composer carries the shared column rails itself (see
              chat-column.ts), so the landing card lines up with the
              mid-conversation composer at every pane width. */}
          <div className="flex w-full flex-col gap-2">
            {notice}
            {composer}
          </div>
          {noWorkspaces && (
            <FirstProjectActions onProjectOpened={onProjectOpened} />
          )}
        </div>
        <LocalSessionImportEntry firstRun />
      </div>
    </div>
  );
}

function FirstProjectActions({
  onProjectOpened,
}: {
  onProjectOpened?: (projectPath: string) => void;
}) {
  const { openProject, openCloneDialog } = useProjectActions();
  const recentProjects = useRecentProjects(true);

  const handleOpenProject = async () => {
    try {
      const result = await openProject();
      if (result.success && result.path) onProjectOpened?.(result.path);
    } catch (err) {
      toast.error(`Couldn't open the project: ${describeError(err)}`);
    }
  };

  return (
    <div className="flex w-full max-w-[400px] flex-col items-center gap-3 px-4">
      <div className="flex flex-wrap items-center justify-center gap-2">
        <Button variant="outline" size="sm" onClick={() => void handleOpenProject()}>
          <FolderOpen />
          Open project
          <MenuKeycap actionId="openProject" className="ml-1" />
        </Button>
        <Button variant="outline" size="sm" onClick={openCloneDialog}>
          <GitBranch />
          Clone repository
        </Button>
      </div>
      <RecentProjectList
        projects={recentProjects}
        onOpened={onProjectOpened}
      />
    </div>
  );
}
