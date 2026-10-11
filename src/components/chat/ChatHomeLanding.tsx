import { useRef, type ReactElement, type ReactNode } from "react";
import { LocalSessionImportEntry } from "./LocalSessionImportEntry";

interface Props {
  composer: ReactNode;
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
 */
export function ChatHomeLanding({
  composer,
  projectName,
  projectPicker,
  onStartWithoutProject,
}: Props) {
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
    <div className="flex h-full w-full flex-col items-center justify-center gap-8 pb-12">
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
      {/* The composer carries the shared column rails itself (see
          chat-column.ts), so the landing card lines up with the
          mid-conversation composer at every pane width. */}
      <div className="w-full">{composer}</div>
      <LocalSessionImportEntry firstRun />
    </div>
  );
}
