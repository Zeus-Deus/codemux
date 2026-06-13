import { useState, useEffect, useMemo } from "react";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";
import { Check, ChevronDown, FolderOpen, FolderPlus } from "lucide-react";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
} from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { basename } from "@/lib/path";
import { dbGetRecentProjects, dbGetUiState } from "@/tauri/commands";
import { useProjectActions } from "@/hooks/use-project-actions";
import { ProjectAvatar } from "@/components/ui/project-avatar";

interface ProjectPickerProps {
  value: string | null;
  onChange: (path: string, name: string) => void;
}

interface RecentProject {
  path: string;
  name: string;
}

/**
 * Per-project avatar inputs, mirrored from the same device-local UI state the
 * sidebar reads (`project.color:`, `project.image:`, `project.image.v:`) so the
 * picker renders the exact same circular favicon/color/letter the sidebar does.
 */
interface ProjectAvatarState {
  color: string | null;
  image: string | null;
  imageVersion: string | null;
}

const EMPTY_AVATAR: ProjectAvatarState = {
  color: null,
  image: null,
  imageVersion: null,
};

export function ProjectPicker({ value, onChange }: ProjectPickerProps) {
  const [open, setOpen] = useState(false);
  const [recentProjects, setRecentProjects] = useState<RecentProject[]>([]);
  const [projectAvatars, setProjectAvatars] = useState<
    Record<string, ProjectAvatarState>
  >({});

  const workspaces = useAppStore((s) => s.appState?.workspaces ?? []);
  const homeDir = useHomeDir();
  const projectGroups = useProjectGroupedWorkspaces(workspaces, homeDir);

  // Load recent projects plus each project's avatar (accent color + custom
  // image + favicon cache-bust token) when the popover opens, so the picker
  // shows the same circular favicon the sidebar does instead of a bare letter.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;

    const loadAvatar = async (
      path: string,
    ): Promise<[string, ProjectAvatarState]> => {
      const [color, image, imageVersion] = await Promise.all([
        dbGetUiState(`project.color:${path}`).catch(() => null),
        dbGetUiState(`project.image:${path}`).catch(() => null),
        dbGetUiState(`project.image.v:${path}`).catch(() => null),
      ]);
      return [
        path,
        {
          color: color || null,
          image: image || null,
          imageVersion: imageVersion || null,
        },
      ];
    };

    (async () => {
      let recents: RecentProject[] = [];
      try {
        recents = await dbGetRecentProjects(10);
      } catch {
        recents = [];
      }
      if (cancelled) return;
      setRecentProjects(recents);

      // Resolve avatar state for every project shown in either group.
      const paths = new Set<string>([
        ...projectGroups.map((g) => g.projectPath),
        ...recents.map((p) => p.path),
      ]);
      const entries = await Promise.all([...paths].map(loadAvatar));
      if (cancelled) return;
      setProjectAvatars(Object.fromEntries(entries));
    })();

    return () => {
      cancelled = true;
    };
  }, [open, projectGroups]);

  const activeProjectPaths = useMemo(
    () => new Set(projectGroups.map((g) => g.projectPath)),
    [projectGroups],
  );

  const filteredRecent = useMemo(
    () => recentProjects.filter((p) => !activeProjectPaths.has(p.path)),
    [recentProjects, activeProjectPaths],
  );

  const selectedName = useMemo(() => {
    if (!value) return null;
    const group = projectGroups.find((g) => g.projectPath === value);
    if (group) return group.projectName;
    const recent = recentProjects.find((p) => p.path === value);
    if (recent) return recent.name;
    return basename(value);
  }, [value, projectGroups, recentProjects]);

  const selectedAvatar = value
    ? projectAvatars[value] ?? EMPTY_AVATAR
    : EMPTY_AVATAR;
  const { openProject } = useProjectActions();
  const setShowNewProjectScreen = useUIStore(
    (s) => s.setShowNewProjectScreen,
  );

  const handleOpenProject = async () => {
    setOpen(false);
    const result = await openProject();
    if (result.success && result.path && result.name) {
      onChange(result.path, result.name);
    }
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1.5 rounded-full bg-muted/60 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground outline-none"
        >
          {selectedName ? (
            // `size="sm"` keeps the trigger pill the same height
            // as the neighboring device + branch pills (their
            // icons are 14px lucide glyphs). The full-size avatar
            // is still used inside the dropdown list below.
            <ProjectAvatar
              name={selectedName}
              color={selectedAvatar.color}
              imageUrl={selectedAvatar.image}
              cacheBust={selectedAvatar.imageVersion}
              size="sm"
              shape="circle"
            />
          ) : (
            <FolderOpen className="h-3.5 w-3.5" />
          )}
          <span className="max-w-[120px] truncate">
            {selectedName || "Select project"}
          </span>
          <ChevronDown className="h-2.5 w-2.5 opacity-40" />
        </button>
      </PopoverTrigger>
      <PopoverContent className="w-60 p-0" align="start">
        <Command>
          <CommandInput placeholder="Search projects..." className="h-8" />
          <CommandList className="max-h-72" onWheel={(e) => e.stopPropagation()}>
            <CommandEmpty>No projects found.</CommandEmpty>
            {projectGroups.length > 0 && (
              <CommandGroup heading="Active">
                {projectGroups.map((g) => (
                  <CommandItem
                    key={g.projectPath}
                    value={g.projectPath}
                    onSelect={() => {
                      onChange(g.projectPath, g.projectName);
                      setOpen(false);
                    }}
                    className="text-xs gap-2"
                  >
                    <ProjectAvatar
                      name={g.projectName}
                      color={projectAvatars[g.projectPath]?.color}
                      imageUrl={projectAvatars[g.projectPath]?.image}
                      cacheBust={projectAvatars[g.projectPath]?.imageVersion}
                      size="md"
                      shape="circle"
                    />
                    <span className="flex-1 truncate">{g.projectName}</span>
                    {value === g.projectPath && (
                      <Check className="ml-auto h-3.5 w-3.5 text-primary" />
                    )}
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            {filteredRecent.length > 0 && (
              <CommandGroup heading="Recent">
                {filteredRecent.map((p) => (
                  <CommandItem
                    key={p.path}
                    value={p.path}
                    onSelect={() => {
                      onChange(p.path, p.name);
                      setOpen(false);
                    }}
                    className="text-xs gap-2"
                  >
                    <ProjectAvatar
                      name={p.name}
                      color={projectAvatars[p.path]?.color}
                      imageUrl={projectAvatars[p.path]?.image}
                      cacheBust={projectAvatars[p.path]?.imageVersion}
                      size="md"
                      shape="circle"
                    />
                    <span className="flex-1 truncate">{p.name}</span>
                    {value === p.path && (
                      <Check className="ml-auto h-3.5 w-3.5 text-primary" />
                    )}
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
          </CommandList>
          <CommandSeparator className="my-1" />
          <div className="p-1">
            <button
              type="button"
              onClick={handleOpenProject}
              className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-xs text-muted-foreground hover:bg-accent hover:text-accent-foreground transition-colors"
            >
              <FolderOpen className="h-3.5 w-3.5" />
              Open project
            </button>
            <button
              type="button"
              onClick={() => {
                setOpen(false);
                setShowNewProjectScreen(true);
              }}
              className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-xs text-muted-foreground hover:bg-accent hover:text-accent-foreground transition-colors"
            >
              <FolderPlus className="h-3.5 w-3.5" />
              New project
            </button>
          </div>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
