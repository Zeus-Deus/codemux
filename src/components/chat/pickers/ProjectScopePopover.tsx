import {
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactElement,
} from "react";
import { ChevronDown, FolderPlus, Home } from "lucide-react";

import {
  Command,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
} from "@/components/ui/command";
import { Eyebrow } from "@/components/ui/eyebrow";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { ProjectAvatar } from "@/components/ui/project-avatar";
import { useProjectActions } from "@/hooks/use-project-actions";
import { fuzzyFilter, fuzzyMatch } from "@/lib/fuzzy";
import { cn } from "@/lib/utils";
import {
  useAppStore,
  useHomeDir,
  useProjectGroupedWorkspaces,
  type ProjectGroup,
} from "@/stores/app-store";
import { useHosts } from "@/stores/hosts-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import type { DraftTarget } from "@/stores/chat-draft-store";
import { dbGetUiState } from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";

import { focusCmdkOnOpen } from "./focus-cmdk-root";
import {
  partitionProjectScopes,
  visibleSettledProjects,
} from "./project-scope-list";

// Module-scoped stable empty array — returning a fresh `[]` literal
// from a Zustand selector triggers React's "getSnapshot should be
// cached" warning and re-render loops.
const EMPTY_WORKSPACES: WorkspaceSnapshot[] = [];

/** Location-picker row geometry, shared by the Home row and the project
 *  rows so both sections line up on the same baseline. */
const PICKER_ITEM =
  "flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-label font-medium text-foreground";
/** Tints `CommandItem`'s built-in trailing check (its last child) to the
 *  accent the location picker has always used for "this is the current
 *  target". Falls back to the neutral check if the selector ever stops
 *  matching, so the affordance can't disappear. */
const CHECKED_ACCENT =
  "data-[checked=true]:[&>svg:last-child]:text-accent-ember";

interface ProjectAvatarState {
  color: string | null;
  image: string | null;
  imageVersion: string | null;
}
const EMPTY_AVATAR: ProjectAvatarState = { color: null, image: null, imageVersion: null };

export interface ProjectScopePopoverProps {
  /** The element that opens the list — the new-thread headline's inline
   *  project name, or the "No project" link under it. */
  trigger: ReactElement;
  onChangeTarget: (target: DraftTarget) => void;
  isHome: boolean;
  /** The currently-active project root (`null` when home / unresolved). */
  activeProjectPath: string | null;
  disabled?: boolean;
  side?: "top" | "bottom";
  align?: "start" | "center" | "end";
}

/**
 * The "Run in" project list: Home, then every project with a live
 * workspace split into Active / Settled, with type-to-filter and an
 * "Open another project…" escape hatch. Retargets the draft
 * (`onChangeTarget`) — nothing is created until first send.
 */
export function ProjectScopePopover({
  trigger,
  onChangeTarget,
  isHome,
  activeProjectPath,
  disabled,
  side = "top",
  align = "start",
}: ProjectScopePopoverProps) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [settledExpanded, setSettledExpanded] = useState(false);
  const [projectAvatars, setProjectAvatars] = useState<
    Record<string, ProjectAvatarState>
  >({});

  const homeDir = useHomeDir();
  const workspaces = useAppStore((s) => s.appState?.workspaces ?? EMPTY_WORKSPACES);
  const hosts = useHosts();
  const groups = useProjectGroupedWorkspaces(workspaces, homeDir, hosts);
  const { openProject } = useProjectActions();

  // Sidebar-inbox state drives the Active/Settled split and both sections'
  // recency order — see `project-scope-list.ts`. Read-only here: selecting a
  // settled project deliberately does NOT un-settle it, because the inbox
  // already resurfaces a settled workspace the moment its agent goes
  // `working`, which is exactly what first send does. Un-settling on mere
  // selection would also fire while the user is only browsing locations.
  const settled = useSidebarInboxStore((s) => s.settled);
  const snoozed = useSidebarInboxStore((s) => s.snoozed);
  const activity = useSidebarInboxStore((s) => s.activity);
  const loadInbox = useSidebarInboxStore((s) => s.load);
  // Idempotent + memoized at module scope. The sidebar normally loads this
  // first, but the picker must not depend on a sidebar being mounted.
  useEffect(() => {
    void loadInbox();
  }, [loadInbox]);

  // Home has its own pinned row above the sections — never list it twice.
  const projectGroups = useMemo(
    () => groups.filter((g) => g.projectPath !== homeDir),
    [groups, homeDir],
  );
  const sections = useMemo(
    () => partitionProjectScopes(projectGroups, settled, snoozed, activity),
    [projectGroups, settled, snoozed, activity],
  );

  // Type-to-filter: rank by fuzzy score so `cdx` or the initials of a
  // hyphenated name land on the right row without reaching for the
  // mouse. A query containing `/` switches the haystack from the
  // display name to the full path — that's how you disambiguate two
  // checkouts of the same repo. (Matching name AND path unconditionally
  // does not narrow: a short query is a subsequence of nearly every
  // long path.) Home matches its own synonyms ("home", "~") and always
  // sorts first when it survives — it is a fixed destination, not a
  // project. Each section is filtered separately: a search spans both,
  // ranking within a section, with Active always above Settled.
  const searching = query.trim() !== "";
  const scopeHaystack = useMemo(() => {
    const byPath = query.includes("/");
    return (g: ProjectGroup) => (byPath ? g.projectPath : g.projectName);
  }, [query]);
  const activeRows = useMemo(
    () => fuzzyFilter(sections.active, query, scopeHaystack),
    [sections.active, query, scopeHaystack],
  );
  const settledRows = useMemo(
    () => fuzzyFilter(sections.settled, query, scopeHaystack),
    [sections.settled, query, scopeHaystack],
  );
  // Collapsed settled tail — a search or an explicit expand reveals the
  // whole section, so the "Show N more" row can never leak into (or hide)
  // search results.
  const visibleSettled = useMemo(
    () =>
      visibleSettledProjects(settledRows, {
        expanded: settledExpanded,
        searching,
        activeProjectPath,
      }),
    [settledRows, settledExpanded, searching, activeProjectPath],
  );
  const hiddenSettledCount = settledRows.length - visibleSettled.length;
  // Section headings only earn their space once something is actually
  // settled; with an empty settled set the picker stays the flat list it
  // always was.
  const showSectionHeadings = sections.settled.length > 0;

  const homeVisible = fuzzyMatch("home directory ~", query.trim());
  const noMatches =
    !homeVisible && activeRows.length === 0 && settledRows.length === 0;

  // Avatar loading is scoped to the rows actually on screen. It used to fetch
  // 3 UI-state keys for EVERY known project on every open — 50+ IPC round
  // trips on a long-lived install, most of them for rows behind the collapsed
  // settled tail. Now the collapsed list costs a handful, and expanding or
  // searching pays only for what it newly reveals.
  const visibleAvatarPaths = useMemo(
    () => [...activeRows, ...visibleSettled].map((g) => g.projectPath),
    [activeRows, visibleSettled],
  );
  // Bumped on each open so appearance edits made elsewhere in the session are
  // picked up, matching `useProjectAppearance`'s refresh-on-mount contract.
  const avatarFetchGen = useRef(0);
  const fetchedAvatarPaths = useRef<Set<string>>(new Set());
  useEffect(() => {
    if (!open) return;
    avatarFetchGen.current += 1;
    fetchedAvatarPaths.current = new Set();
  }, [open]);
  useEffect(() => {
    if (!open) return;
    const pending = visibleAvatarPaths.filter(
      (path) => !fetchedAvatarPaths.current.has(path),
    );
    if (pending.length === 0) return;
    for (const path of pending) fetchedAvatarPaths.current.add(path);
    const gen = avatarFetchGen.current;
    void (async () => {
      const entries = await Promise.all(
        pending.map(async (path) => {
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
          ] as const;
        }),
      );
      // Deliberately NOT cancelled when the visible set changes mid-flight —
      // that would drop a batch and leave those rows on default avatars. Only
      // a reopen (new generation) invalidates the result.
      if (avatarFetchGen.current !== gen) return;
      setProjectAvatars((prev) => ({ ...prev, ...Object.fromEntries(entries) }));
    })();
  }, [open, visibleAvatarPaths]);

  const handleOpenChange = (next: boolean) => {
    // The headline trigger is a plain inline button, so the disabled
    // state is enforced here rather than relying on the trigger.
    if (next && disabled) return;
    setOpen(next);
    // Every open starts from the full list — a stale query from last
    // time would silently hide projects, and a stale expansion would
    // pre-spend the "Show N more" affordance.
    if (!next) {
      setQuery("");
      setSettledExpanded(false);
    }
  };

  const handleSelectHome = () => {
    handleOpenChange(false);
    onChangeTarget({ kind: "home" });
  };

  const handleSelectProject = (targetProjectPath: string) => {
    handleOpenChange(false);
    onChangeTarget({ kind: "project", projectPath: targetProjectPath });
  };

  const renderProjectItem = (g: ProjectGroup) => {
    const active = g.projectPath === activeProjectPath;
    const avatar = projectAvatars[g.projectPath] ?? EMPTY_AVATAR;
    return (
      <CommandItem
        key={g.projectPath}
        value={g.projectPath}
        onSelect={() => handleSelectProject(g.projectPath)}
        className={cn(PICKER_ITEM, CHECKED_ACCENT)}
        data-checked={active ? "true" : undefined}
        // Stable hook for tests/automation: the row's visible text is the
        // display name, which collides across same-basename projects and is
        // prefixed by the avatar's initial glyph.
        data-project-path={g.projectPath}
      >
        <ProjectAvatar
          name={g.projectName}
          color={avatar.color}
          imageUrl={avatar.image}
          cacheBust={avatar.imageVersion}
          size="md"
          shape="square"
        />
        <span className="min-w-0 flex-1 truncate">{g.projectName}</span>
      </CommandItem>
    );
  };

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>{trigger}</PopoverTrigger>
      <PopoverContent
        // Bounded by the space Radix actually has beside the trigger, so the
        // taller sectioned popover can't clip off a short window —
        // `CommandList` is the flex child that gives way.
        className="flex max-h-[var(--radix-popover-content-available-height)] w-[260px] flex-col p-0"
        align={align}
        side={side}
        collisionPadding={10}
        onOpenAutoFocus={focusCmdkOnOpen}
      >
        {/* `shouldFilter={false}`: cmdk's built-in scorer ranks by its
            own rules; we filter each section with `fuzzyFilter` so
            initials-style queries win and the Active/Settled split
            survives a search. cmdk still owns highlight + Enter. */}
        <Command shouldFilter={false} loop>
          <Eyebrow className="px-2.5 pb-1 pt-2">
            Run in
          </Eyebrow>
          <CommandInput
            placeholder="Search projects…"
            value={query}
            onValueChange={setQuery}
            className="text-label"
          />
          <CommandList
            className="max-h-[280px] min-h-0 flex-1 overflow-y-auto p-1.5 pb-0 thin-scrollbar"
            onWheel={(e) => e.stopPropagation()}
          >
            {noMatches && (
              <div className="px-2 py-3 text-center text-body-sm text-muted-foreground">
                No projects match “{query.trim()}”
              </div>
            )}
            {homeVisible && (
              <CommandItem
                value="home-directory"
                onSelect={handleSelectHome}
                className={cn(PICKER_ITEM, CHECKED_ACCENT)}
                data-checked={isHome ? "true" : undefined}
              >
                <span className="flex size-5 shrink-0 items-center justify-center rounded-md bg-muted text-muted-foreground">
                  <Home className="size-3" />
                </span>
                <span className="min-w-0 flex-1 truncate">
                  Home directory (~)
                </span>
              </CommandItem>
            )}
            {activeRows.length > 0 && (
              <CommandGroup
                heading={showSectionHeadings ? "Active" : undefined}
              >
                {activeRows.map(renderProjectItem)}
              </CommandGroup>
            )}
            {visibleSettled.length > 0 && (
              <CommandGroup
                heading={
                  // Spells out the thing the section title alone implies but
                  // doesn't say: settling parks a workspace, it never closes
                  // it, so these are still perfectly valid places to run.
                  <span>
                    Settled{" "}
                    <span className="font-normal opacity-60">· still open</span>
                  </span>
                }
              >
                {visibleSettled.map(renderProjectItem)}
                {hiddenSettledCount > 0 && (
                  <CommandItem
                    value="show-all-settled-projects"
                    onSelect={() => setSettledExpanded(true)}
                    className="gap-2 rounded-lg px-2 py-1.5 text-body-sm text-muted-foreground"
                  >
                    <ChevronDown className="size-3.5 shrink-0" />
                    Show {hiddenSettledCount} more
                  </CommandItem>
                )}
              </CommandGroup>
            )}
          </CommandList>
          {/* Outside the list, and never filtered: the escape hatch has
              to stay reachable precisely when nothing matched — and it
              must never scroll away behind a long project list. */}
          <div className="mt-1.5 border-t border-border p-1.5">
            <button
              type="button"
              onClick={async () => {
                handleOpenChange(false);
                const result = await openProject();
                if (result.success && result.path) {
                  handleSelectProject(result.path);
                }
              }}
              className="flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-body-sm text-muted-foreground transition-colors duration-150 hover:bg-surface-2 hover:text-foreground"
            >
              <FolderPlus className="size-3.5" />
              Open another project…
            </button>
          </div>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
