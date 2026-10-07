import { useEffect, useState } from "react";
import { Eyebrow } from "@/components/ui/eyebrow";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { openProjectAtPath } from "@/hooks/use-project-actions";
import { shortenPath } from "@/lib/shorten-path";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useHomeDir } from "@/stores/app-store";
import { dbGetRecentProjects } from "@/tauri/commands";
import { useProjectAppearance } from "./use-project-appearance";

export interface RecentProject {
  path: string;
  name: string;
}

const RECENT_PROJECT_LIMIT = 5;

/**
 * Folders Codemux has opened before, newest first. Only fetched while
 * `enabled`, so the landings that render it ask once per visit instead of
 * on every workspace tick.
 */
export function useRecentProjects(enabled: boolean): RecentProject[] {
  const homeDir = useHomeDir();
  const [projects, setProjects] = useState<RecentProject[]>([]);
  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    dbGetRecentProjects(RECENT_PROJECT_LIMIT)
      .then((rows) => {
        if (cancelled) return;
        setProjects(
          (rows ?? [])
            // Home is a chat location, not a project to reopen.
            .filter((row) => row.path !== homeDir)
            .map(({ path, name }) => ({ path, name })),
        );
      })
      .catch(() => {
        if (!cancelled) setProjects([]);
      });
    return () => {
      cancelled = true;
    };
  }, [enabled, homeDir]);
  return projects;
}

function describeError(err: unknown): string {
  if (err instanceof Error) return err.message;
  return String(err);
}

/**
 * One-click reopen for recent projects on the zero-workspace landings, so a
 * returning user does not have to find the folder in the OS picker again.
 */
export function RecentProjectList({
  projects,
  onOpened,
  className,
}: {
  projects: RecentProject[];
  /** Called with the project path once its workspace exists. */
  onOpened?: (path: string) => void;
  className?: string;
}) {
  const [openingPath, setOpeningPath] = useState<string | null>(null);
  if (projects.length === 0) return null;

  const open = async (project: RecentProject) => {
    if (openingPath) return;
    setOpeningPath(project.path);
    try {
      const result = await openProjectAtPath(project.path);
      if (result.success) onOpened?.(project.path);
    } catch (err) {
      toast.error(`Couldn't open ${project.name}: ${describeError(err)}`);
    } finally {
      setOpeningPath(null);
    }
  };

  return (
    <section
      aria-label="Recent projects"
      className={cn("flex w-full flex-col gap-1", className)}
    >
      <Eyebrow className="px-2">Recent</Eyebrow>
      <ul className="flex flex-col">
        {projects.map((project) => (
          <li key={project.path}>
            <RecentProjectRow
              project={project}
              disabled={openingPath !== null}
              onOpen={() => void open(project)}
            />
          </li>
        ))}
      </ul>
    </section>
  );
}

function RecentProjectRow({
  project,
  disabled,
  onOpen,
}: {
  project: RecentProject;
  disabled: boolean;
  onOpen: () => void;
}) {
  const homeDir = useHomeDir();
  const { customColor, imageUrl, imageVersion } = useProjectAppearance(
    project.path,
  );
  return (
    <button
      type="button"
      onClick={onOpen}
      disabled={disabled}
      title={project.path}
      className="flex w-full min-w-0 items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-body-sm transition-colors duration-150 hover:bg-surface-2 disabled:opacity-60"
    >
      <ProjectAvatar
        name={project.name}
        color={customColor}
        imageUrl={imageUrl}
        cacheBust={imageVersion}
        size="md"
        shape="circle"
      />
      <span className="max-w-[60%] truncate font-medium text-foreground">
        {project.name}
      </span>
      <span className="ml-auto min-w-0 truncate font-mono text-caption text-muted-foreground">
        {shortenPath(project.path, homeDir)}
      </span>
    </button>
  );
}
