import { useRef, type ReactElement, type ReactNode } from "react";
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
import { isRemoteClient } from "@/components/remote/is-remote-client";
import type { AgentChatProviderKind } from "@/tauri/types";
import { AgentAvailabilityStrip } from "./AgentAvailabilityStrip";
import { LocalSessionImportEntry } from "./LocalSessionImportEntry";

interface Props {
  composer: ReactNode;
  /** The composer's provider and how to switch it. Given only by the
   *  draft surface; with it, a first run (no workspaces yet) shows which
   *  agents are installed and signed in on this machine. */
  agents?: {
    selected: AgentChatProviderKind;
    onSelect: (provider: AgentChatProviderKind) => void;
  };
  /** Optional line above the composer (e.g. the Full access notice). */
  notice?: ReactNode;
  /** A project was opened from the zero-project actions below. */
  onProjectOpened?: (projectPath: string) => void;
  /** Project the new thread will run in. `undefined` keeps the generic
   *  headline (surfaces with no project scope); `null` means no project. */
  projectName?: string | null;
  /** Wraps a project trigger in a project picker: the project name in the
   *  headline, or the "No project" link under it. Receives the trigger
   *  element and returns it wrapped (e.g. in a popover). */
  projectPicker?: (trigger: ReactElement) => ReactNode;
  /** Shown under a project headline; switches the thread to no project. */
  onStartWithoutProject?: () => void;
}

/**
 * Empty-state landing for the chat surface. Per the chat-ui skill,
 * this is the sole place inside the chat feature that may exceed
 * prose size — one marquee headline above the composer, no grid of
 * example prompts, no marketing copy. The composer is the invitation.
 *
 * On a new-thread draft the headline is also where the project is
 * chosen; the scope strip under the composer doesn't repeat it.
 *
 * With no project open yet, a quiet row of project actions sits under the
 * composer: this landing replaces the full-screen Open Project page under
 * lazy workspace creation, so it has to offer the same ways in.
 */
export function ChatHomeLanding({
  composer,
  agents,
  notice,
  onProjectOpened,
  projectName,
  projectPicker,
  onStartWithoutProject,
}: Props) {
  const noWorkspaces = useAppStore(
    (s) => s.appState !== null && (s.appState.workspaces?.length ?? 0) === 0,
  );
  // "or start without a project" unmounts once clicked, and the "No
  // project" picker takes its slot; focus follows so keyboard users
  // don't drop back to the document body.
  const focusNoProjectRef = useRef(false);

  let headline: ReactNode = "What should we do today?";
  let subline: ReactNode = null;
  if (projectName === null) {
    headline = "What should we work on?";
    if (projectPicker) {
      subline = projectPicker(
        <button
          type="button"
          ref={(el) => {
            if (el && focusNoProjectRef.current) {
              focusNoProjectRef.current = false;
              el.focus();
            }
          }}
          className="rounded-sm text-body-sm text-muted-foreground underline decoration-muted-foreground/50 decoration-dotted underline-offset-4 transition-colors duration-150 hover:text-foreground hover:decoration-foreground"
        >
          No project
        </button>,
      );
    }
  } else if (projectName !== undefined) {
    const name = (
      <button
        type="button"
        className="rounded-sm underline decoration-muted-foreground/50 decoration-dotted decoration-2 underline-offset-[6px] transition-colors duration-150 hover:decoration-foreground"
      >
        {projectName}
      </button>
    );
    headline = (
      <>
        What should we build in {projectPicker ? projectPicker(name) : projectName}?
      </>
    );
    if (onStartWithoutProject) {
      subline = (
        <button
          type="button"
          onClick={() => {
            focusNoProjectRef.current = true;
            onStartWithoutProject();
          }}
          className="text-body-sm text-muted-foreground transition-colors duration-150 hover:text-foreground"
        >
          or start without a project
        </button>
      );
    }
  }

  return (
    // `my-auto` on the column centres it while it fits and lets it scroll
    // from the top in a short pane, where flex centring would clip the
    // headline off the top.
    <div className="flex h-full w-full flex-col items-center overflow-y-auto">
      <div className="my-auto flex w-full flex-col items-center gap-8 pt-6 pb-12">
        <div className="flex flex-col items-center gap-2 px-4">
          <h1 className="text-3xl font-medium tracking-tight text-foreground text-center">
            {headline}
          </h1>
          {/* Reserved on every project-scoped draft, so the heading stays
              put when the project is dropped or picked. */}
          {projectName !== undefined && (
            <div className="flex h-6 items-center" data-testid="landing-subline">
              {subline}
            </div>
          )}
        </div>
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
        {agents && noWorkspaces && !isRemoteClient() && (
          <AgentAvailabilityStrip selected={agents.selected} onSelect={agents.onSelect} />
        )}
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
