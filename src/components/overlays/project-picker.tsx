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
import { cn } from "@/lib/utils";
import { useAppStore, useProjectGroupedWorkspaces } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { dbGetRecentProjects, dbGetUiState } from "@/tauri/commands";
import { useProjectActions } from "@/hooks/use-project-actions";

interface ProjectPickerProps {
  value: string | null;
  onChange: (path: string, name: string) => void;
}

interface RecentProject {
  path: string;
  name: string;
}

function hexToRgba(hex: string, alpha: number): string {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}

function ProjectAvatar({
  name,
  color,
  className,
}: {
  name: string;
  color: string | null | undefined;
  className?: string;
}) {
  const letter = (name || "?").charAt(0).toUpperCase();
  const hasColor = !!color;
  return (
    <div
      className={cn(
        "size-5 rounded flex items-center justify-center shrink-0 text-[10px] font-medium border-[1.5px]",
        !hasColor && "bg-muted text-muted-foreground border-border",
        className,
      )}
      style={
        hasColor
          ? {
              borderColor: hexToRgba(color!, 0.6),
              backgroundColor: hexToRgba(color!, 0.15),
              color: color!,
            }
          : undefined
      }
    >
      {letter}
    </div>
  );
}

export function ProjectPicker({ value, onChange }: ProjectPickerProps) {
  const [open, setOpen] = useState(false);
  const [recentProjects, setRecentProjects] = useState<RecentProject[]>([]);
  const [projectColors, setProjectColors] = useState<Record<string, string>>(
    {},
  );

  const workspaces = useAppStore((s) => s.appState?.workspaces ?? []);
  const projectGroups = useProjectGroupedWorkspaces(workspaces);

  // Load recent projects and project colors when popover opens
  useEffect(() => {
    if (!open) return;
    dbGetRecentProjects(10)
      .then((projects) => setRecentProjects(projects))
      .catch(() => setRecentProjects([]));

    // Load colors for all active projects
    const colors: Record<string, string> = {};
    const promises = projectGroups.map((g) =>
      dbGetUiState(`project.color:${g.projectPath}`)
        .then((val) => {
          if (val) colors[g.projectPath] = val;
        })
        .catch(() => {}),
    );
    Promise.all(promises).then(() => setProjectColors(colors));
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
    return value.split("/").filter(Boolean).pop() || value;
  }, [value, projectGroups, recentProjects]);

  const selectedColor = value ? projectColors[value] || null : null;
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
            <ProjectAvatar name={selectedName} color={selectedColor} />
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
                      color={projectColors[g.projectPath]}
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
                    <ProjectAvatar name={p.name} color={null} />
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
