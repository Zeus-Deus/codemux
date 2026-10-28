import { useRef, type ReactElement, type ReactNode } from "react";
import { LandingBackdropHost } from "@/contrib/react/landing-backdrop";
import { Slot } from "@/contrib/react/slot";
import { LANDING_BELOW_COMPOSER_AREA } from "@/sdk/areas";
import { ChevronDown, FolderOpen } from "lucide-react";
import { useProjectPickerHintStore } from "@/stores/project-picker-hint-store";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import type { AgentChatProviderKind } from "@/tauri/types";
import { AgentStatusLine } from "./AgentStatusLine";
import { ConnectAgentCard } from "./ConnectAgentCard";
import { FirstProjectActions } from "./FirstProjectActions";
import { useFirstRun, useFirstRunAgents, useReprobeOnReturn } from "./first-run-agents";
import { LocalSessionImportEntry } from "./LocalSessionImportEntry";
import { ProjectPickerHint } from "./ProjectPickerHint";

interface Props {
  composer: ReactNode;
  /** The composer's provider. Given only by the draft surface for a
   *  thread on this machine; with it, a first run (no projects yet) says
   *  which agents are ready here, and asks to connect one when none is. */
  agents?: { selected: AgentChatProviderKind };
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
  /** The workspace this landing will run in, for plugin areas. */
  workspaceId?: string | null;
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
 * With no project open yet, project tiles sit under the composer: this
 * landing replaces the full-screen Open Project page under lazy workspace
 * creation, so it has to offer the same ways in. On that first run a status
 * line says which agents can run here; when none can, the headline and
 * composer give way to a card that connects one.
 */
export function ChatHomeLanding({
  composer,
  agents,
  onProjectOpened,
  projectName,
  projectPicker,
  onStartWithoutProject,
  workspaceId = null,
}: Props) {
  const firstRun = useFirstRun();
  const checkAgents = !!agents && firstRun && !isRemoteClient();
  const firstRunAgents = useFirstRunAgents(checkAgents);
  const connectAgent = checkAgents && firstRunAgents.noneReady;
  useReprobeOnReturn(firstRunAgents.providers, connectAgent);
  // "or start without a project" unmounts once clicked, and the "No
  // project" picker takes its slot; focus follows so keyboard users
  // don't drop back to the document body.
  const focusNoProjectRef = useRef(false);

  // Until the picker has been opened once, a dotted arrow points at it.
  const hintSeen = useProjectPickerHintStore((s) => s.seen);
  const markHintSeen = useProjectPickerHintStore((s) => s.markSeen);
  const showHint = !!projectPicker && !hintSeen;

  let headline: ReactNode = "What should we do today?";
  let subline: ReactNode = null;
  if (projectName === null) {
    headline = "What should we work on?";
    if (projectPicker) {
      subline = (
        <span className="relative inline-flex">
          {projectPicker(
            <button
              type="button"
              ref={(el) => {
                if (el && focusNoProjectRef.current) {
                  focusNoProjectRef.current = false;
                  el.focus();
                }
              }}
              onClick={markHintSeen}
              className="inline-flex h-8 items-center gap-1.5 rounded-full border border-border/80 px-3 text-body text-muted-foreground transition-colors duration-150 hover:border-foreground/30 hover:bg-surface-2 hover:text-foreground data-[state=open]:border-foreground/30 data-[state=open]:text-foreground"
            >
              <FolderOpen className="size-4" aria-hidden />
              No project
              <ChevronDown className="size-3.5 opacity-70" aria-hidden />
            </button>,
          )}
          {showHint && <ProjectPickerHint placement="side" />}
        </span>
      );
    }
  } else if (projectName !== undefined) {
    const name = (
      <button
        type="button"
        onClick={markHintSeen}
        className="group/project -ml-1 inline-flex items-baseline gap-0.5 rounded-lg pl-1 pr-0.5 transition-colors duration-150 hover:bg-surface-2 data-[state=open]:bg-surface-2"
      >
        <span className="underline decoration-muted-foreground/50 decoration-dotted decoration-2 underline-offset-[6px] transition-colors duration-150 group-hover/project:decoration-foreground">
          {projectName}
        </span>
        <ChevronDown
          className="-mr-1.5 size-6 self-center text-muted-foreground transition-colors duration-150 group-hover/project:text-foreground"
          aria-hidden
        />
      </button>
    );
    headline = (
      <>
        What should we build in{" "}
        {projectPicker ? (
          <span className="relative inline-block">
            {projectPicker(name)}
            {showHint && <ProjectPickerHint placement="below" />}
          </span>
        ) : (
          projectName
        )}
        ?
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
    <LandingBackdropHost>
    <div className="@container flex h-full w-full flex-col items-center overflow-x-hidden overflow-y-auto">
      <div className="my-auto flex w-full flex-col items-center gap-8 pt-6 pb-12">
        {connectAgent ? (
          <ConnectAgentCard agents={firstRunAgents} />
        ) : (
        <>
        <div className="flex flex-col items-center gap-2 px-4" data-backdrop-calm>
          <h1 className="text-3xl font-medium tracking-tight text-foreground text-center">
            {headline}
          </h1>
          {/* Reserved on every project-scoped draft, so the heading stays
              put when the project is dropped or picked. */}
          {projectName !== undefined && (
            <div className="flex h-8 items-center" data-testid="landing-subline">
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
          {composer}
          {firstRun && (
            <FirstProjectActions onProjectOpened={onProjectOpened} />
          )}
          <Slot area={LANDING_BELOW_COMPOSER_AREA} props={{ workspaceId }} variant="block" />
        </div>
        {checkAgents && (
          <AgentStatusLine agents={firstRunAgents} selected={agents.selected} />
        )}
        </>
        )}
        <LocalSessionImportEntry firstRun />
      </div>
    </div>
    </LandingBackdropHost>
  );
}
