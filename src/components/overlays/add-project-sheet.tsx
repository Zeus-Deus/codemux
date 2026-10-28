import {
  useEffect,
  useId,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from "react";
import { FolderOpen, GitBranch, Plus } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogTitle,
  DIALOG_CRISP_POSITION,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Eyebrow } from "@/components/ui/eyebrow";
import { Input } from "@/components/ui/input";
import { MenuKeycap } from "@/components/ui/menu-chrome";
import { Spinner } from "@/components/ui/spinner";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { CloneProgressRow } from "@/components/overlays/clone-progress-row";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import {
  RecentProjectList,
  describeError,
  useRecentProjects,
} from "@/components/layout/recent-projects";
import {
  cloneProjectFlow,
  createProjectFlow,
  openDroppedFolder,
  openProjectFlow,
  pointHomeDraftAt,
} from "@/hooks/use-project-actions";
import { useCloneProgress } from "@/hooks/use-clone-progress";
import { useFolderDrop } from "@/hooks/use-folder-drop";
import {
  cloneFolderName,
  defaultProjectsLocation,
  joinPath,
  normalizeCloneUrl,
  projectNameError,
} from "@/lib/add-project";
import { pickFolder } from "@/lib/file-dialog";
import { shortenPath } from "@/lib/shorten-path";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { useHomeDir } from "@/stores/app-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore, type AddProjectTab } from "@/stores/ui-store";

const TABS: { value: AddProjectTab; label: string; icon: typeof FolderOpen }[] = [
  { value: "open", label: "Open", icon: FolderOpen },
  { value: "clone", label: "Clone", icon: GitBranch },
  { value: "new", label: "New", icon: Plus },
];

function closeSheet(): void {
  useUIStore.getState().setAddProjectSheet(null);
}

/** A project is open: aim the draft on screen at it and get out of the way. */
function finish(projectPath: string): void {
  pointHomeDraftAt(projectPath);
  closeSheet();
}

/** False once the tab unmounts. Closing the sheet mid-clone leaves the clone
 *  running; when it settles, only a still-mounted tab may close the sheet or
 *  show the error inline — otherwise a failure becomes a toast. */
function useMountedRef() {
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  return mounted;
}

/**
 * The one place projects get added: open a folder, clone a repository, or
 * start a new one. Every entry point (landing tiles, the sidebar project
 * menu, the project picker, the mobile menu, the command palette) opens it
 * on the matching tab via `useUIStore.addProjectSheet`.
 */
export function AddProjectSheet() {
  const tab = useUIStore((s) => s.addProjectSheet);
  const setTab = useUIStore((s) => s.setAddProjectSheet);
  // A clone or create in flight pins the sheet to its tab, so switching
  // can't unmount the progress it is showing.
  const [busy, setBusy] = useState(false);
  // The last open tab keeps rendering while the sheet fades out, so the
  // closing dialog doesn't collapse to an empty frame.
  const [shownTab, setShownTab] = useState<AddProjectTab>(tab ?? "open");
  if (tab && tab !== shownTab) setShownTab(tab);

  return (
    <Dialog
      open={tab !== null}
      onOpenChange={(open) => {
        if (open) return;
        setTab(null);
        setBusy(false);
      }}
    >
      <DialogContent
        showCloseButton={false}
        data-mobile-add-project
        className={cn(DIALOG_CRISP_POSITION, "gap-0 p-4 sm:max-w-[540px]")}
      >
        <DialogTitle className="sr-only">Add project</DialogTitle>
        <DialogDescription className="sr-only">
          Open a folder, clone a repository, or create a new project.
        </DialogDescription>
        <Tabs
          value={shownTab}
          onValueChange={(value) => setTab(value as AddProjectTab)}
        >
          <TabsList className="w-full" aria-label="Add project">
            {TABS.map(({ value, label, icon: Icon }) => (
              <TabsTrigger
                key={value}
                value={value}
                disabled={busy && value !== shownTab}
              >
                <Icon />
                {label}
              </TabsTrigger>
            ))}
          </TabsList>
          <TabsContent value="open" className="mt-3">
            <OpenTab />
          </TabsContent>
          <TabsContent value="clone" className="mt-3">
            <CloneTab onBusyChange={setBusy} />
          </TabsContent>
          <TabsContent value="new" className="mt-3">
            <NewTab onBusyChange={setBusy} />
          </TabsContent>
        </Tabs>
      </DialogContent>
    </Dialog>
  );
}

function OpenTab() {
  const recentProjects = useRecentProjects(true);
  const dragging = useFolderDrop((path) => {
    void openDroppedFolder(path).then((opened) => opened && finish(path));
  });

  const choose = async () => {
    try {
      const result = await openProjectFlow();
      if (result.success && result.path) finish(result.path);
    } catch (err) {
      toast.error(`Couldn't open the project: ${describeError(err)}`);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      <div
        data-drop-target={dragging || undefined}
        className={cn(
          "flex flex-col items-center gap-2 rounded-lg border border-dashed border-border px-6 py-8 text-center transition-colors duration-150",
          dragging && "border-foreground/30 bg-surface-2",
        )}
      >
        <Button variant="outline" onClick={() => void choose()} autoFocus>
          <FolderOpen />
          Choose folder…
          <MenuKeycap actionId="openProject" className="ml-1" />
        </Button>
        {/* A remote client's browser has no local folders to drop. */}
        {!isRemoteClient() && (
          <p className="text-label text-muted-foreground">
            {dragging ? "Drop the folder to open it" : "or drop a folder anywhere in the window"}
          </p>
        )}
      </div>
      <RecentProjectList projects={recentProjects} onOpened={finish} />
    </div>
  );
}

/** The parent folder new projects go into: the remembered choice, or
 *  `~/Projects`. */
function useProjectsLocation() {
  const homeDir = useHomeDir();
  const picked = useUIStore((s) => s.projectsLocation);
  const location = picked ?? defaultProjectsLocation(homeDir);
  const change = async () => {
    const folder = await pickFolder("Projects location");
    if (folder) useUIStore.getState().setProjectsLocation(folder);
  };
  return { location, homeDir, change };
}

/** "→ ~/Projects/name" — the full destination, always visible, with the
 *  way to move it while nothing is in flight. */
function DestinationPreview({
  name,
  disabled,
}: {
  name: string;
  disabled: boolean;
}) {
  const { location, homeDir, change } = useProjectsLocation();
  const base = shortenPath(location, homeDir);
  return (
    <div className="flex min-w-0 items-baseline gap-1.5 text-label text-muted-foreground">
      <span aria-hidden>→</span>
      <span className="min-w-0 break-all font-mono" data-testid="destination-path">
        {joinPath(base, "")}
        <span className="text-foreground">{name}</span>
      </span>
      {!disabled && (
        <button
          type="button"
          onClick={() => void change()}
          className="shrink-0 underline underline-offset-3 transition-colors duration-150 hover:text-foreground"
        >
          Change
        </button>
      )}
    </div>
  );
}

/** One reserved line under a form, so the first error doesn't push the
 *  footer down. */
function FormError({ error }: { error: string | null }) {
  return (
    <p role="alert" className="min-h-[1lh] text-label text-destructive">
      {error}
    </p>
  );
}

function CloneTab({ onBusyChange }: { onBusyChange: (busy: boolean) => void }) {
  const urlId = useId();
  const nameId = useId();
  const { location } = useProjectsLocation();
  const mounted = useMountedRef();
  const [url, setUrl] = useState("");
  // Null until the user edits the name; until then it follows the URL.
  const [nameOverride, setNameOverride] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The exact path handed to `git clone`, so the progress subscription
  // filters to this clone. Empty = no clone running.
  const [cloneTarget, setCloneTarget] = useState("");
  const cloning = cloneTarget !== "";
  const progress = useCloneProgress(cloning, cloneTarget);
  const name = nameOverride ?? cloneFolderName(url);

  const clone = async () => {
    if (cloning) return;
    const repoUrl = normalizeCloneUrl(url);
    if (!repoUrl) {
      setError("Enter a git URL, or owner/repo for GitHub.");
      return;
    }
    const nameError = projectNameError(name);
    if (nameError) {
      setError(nameError);
      return;
    }
    const target = joinPath(location, name);
    setError(null);
    setCloneTarget(target);
    onBusyChange(true);
    try {
      const { path } = await cloneProjectFlow(repoUrl, target);
      if (mounted.current) finish(path);
    } catch (err) {
      if (mounted.current) setError(describeError(err));
      else toast.error(`Couldn't clone ${name}: ${describeError(err)}`);
    } finally {
      setCloneTarget("");
      if (mounted.current) onBusyChange(false);
    }
  };

  if (cloning) {
    return (
      <div className="flex flex-col gap-3 py-1">
        <div className="min-w-0">
          <div className="truncate text-body font-medium text-foreground">{name}</div>
          <DestinationPreview name={name} disabled />
        </div>
        <CloneProgressRow progress={progress} />
      </div>
    );
  }

  return (
    <form
      className="flex flex-col gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        void clone();
      }}
    >
      <div className="flex flex-col gap-1.5">
        <Eyebrow asChild>
          <label htmlFor={urlId}>Repository</label>
        </Eyebrow>
        <Input
          id={urlId}
          value={url}
          onChange={(e) => {
            setUrl(e.target.value);
            setError(null);
          }}
          placeholder="https://, git@, or owner/repo"
          autoFocus
          spellCheck={false}
          autoComplete="off"
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <Eyebrow asChild>
          <label htmlFor={nameId}>Folder name</label>
        </Eyebrow>
        <Input
          id={nameId}
          value={name}
          onChange={(e) => {
            setNameOverride(e.target.value);
            setError(null);
          }}
          placeholder="repo-name"
          spellCheck={false}
          autoComplete="off"
        />
        <DestinationPreview name={name} disabled={false} />
      </div>
      <FormError error={error} />
      <div className="flex justify-end">
        <Button type="submit" disabled={!url.trim()}>
          Clone
          <MenuKeycap keys="Enter" className="ml-1 text-primary-foreground/60" />
        </Button>
      </div>
    </form>
  );
}

type StartWith = "empty" | "agent";

function NewTab({ onBusyChange }: { onBusyChange: (busy: boolean) => void }) {
  const nameId = useId();
  const { location } = useProjectsLocation();
  const mounted = useMountedRef();
  // Agent scaffolding lands in a chat draft, which only the agent-chat
  // interface shows.
  const agentChat = useFeatureFlags(
    (s) => s.enableAgentChat && s.enableLazyWorkspaceCreation,
  );
  const [name, setName] = useState("");
  const [startWith, setStartWith] = useState<StartWith>("empty");
  const [description, setDescription] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const descriptionRef = useRef<HTMLTextAreaElement>(null);
  const withAgent = agentChat && startWith === "agent";

  // Picking the agent card means typing next; put the caret there.
  useEffect(() => {
    if (startWith === "agent") descriptionRef.current?.focus();
  }, [startWith]);

  const create = async () => {
    if (creating) return;
    const nameError = projectNameError(name);
    if (nameError) {
      setError(nameError);
      return;
    }
    if (withAgent && !description.trim()) {
      setError("Describe what the agent should build.");
      return;
    }
    setError(null);
    setCreating(true);
    onBusyChange(true);
    try {
      const { path } = await createProjectFlow(
        location,
        name,
        withAgent ? description.trim() : undefined,
      );
      // The agent flow already put its own draft on screen.
      if (!mounted.current) return;
      if (withAgent) closeSheet();
      else finish(path);
    } catch (err) {
      if (mounted.current) setError(describeError(err));
      else toast.error(`Couldn't create ${name}: ${describeError(err)}`);
    } finally {
      setCreating(false);
      if (mounted.current) onBusyChange(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      void create();
    }
  };

  return (
    <div className="flex flex-col gap-3" onKeyDown={onKeyDown}>
      <div className="flex flex-col gap-1.5">
        <Eyebrow asChild>
          <label htmlFor={nameId}>Name</label>
        </Eyebrow>
        <Input
          id={nameId}
          value={name}
          onChange={(e) => {
            setName(e.target.value);
            setError(null);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.ctrlKey && !e.metaKey) {
              e.preventDefault();
              void create();
            }
          }}
          placeholder="my-project"
          disabled={creating}
          autoFocus
          spellCheck={false}
          autoComplete="off"
        />
        <DestinationPreview name={name} disabled={creating} />
      </div>
      {agentChat && (
        <div className="flex flex-col gap-1.5">
          <Eyebrow id={`${nameId}-start`}>Start with</Eyebrow>
          <div
            role="radiogroup"
            aria-labelledby={`${nameId}-start`}
            className="grid grid-cols-[1fr_1.35fr] gap-2"
          >
            <StartCard
              selected={startWith === "empty"}
              onSelect={() => setStartWith("empty")}
              disabled={creating}
              label="Empty repository"
              hint="git init with a first commit"
            />
            <StartCard
              selected={startWith === "agent"}
              onSelect={() => setStartWith("agent")}
              disabled={creating}
              label="Describe it, and an agent builds it"
            >
              <Textarea
                ref={descriptionRef}
                value={description}
                onChange={(e) => {
                  setDescription(e.target.value);
                  setError(null);
                }}
                onFocus={() => setStartWith("agent")}
                aria-label="What should the agent build?"
                placeholder="A tiny habit tracker with a weekly streak view"
                disabled={creating}
                className="mt-2 max-h-40 min-h-16 resize-none text-body"
              />
            </StartCard>
          </div>
        </div>
      )}
      <FormError error={error} />
      <div className="flex justify-end">
        <Button onClick={() => void create()} disabled={creating}>
          {creating && <Spinner className="size-3" aria-hidden />}
          {creating ? "Creating…" : "Create"}
          {!creating && (
            <MenuKeycap keys="Ctrl+Enter" className="ml-1 text-primary-foreground/60" />
          )}
        </Button>
      </div>
    </div>
  );
}

/** A radio card. The whole card selects; extra content (the agent prompt)
 *  sits inside it. */
function StartCard({
  selected,
  onSelect,
  disabled,
  label,
  hint,
  children,
}: {
  selected: boolean;
  onSelect: () => void;
  disabled: boolean;
  label: string;
  hint?: string;
  children?: ReactNode;
}) {
  return (
    <div
      className={cn(
        "rounded-lg border p-3 transition-colors duration-150",
        selected ? "border-foreground/40 bg-surface-2" : "border-border hover:bg-surface-2/60",
      )}
      onClick={() => !disabled && onSelect()}
    >
      <button
        type="button"
        role="radio"
        aria-checked={selected}
        disabled={disabled}
        onClick={onSelect}
        className="flex w-full items-start gap-2 text-left text-body text-foreground outline-none focus-visible:underline"
      >
        <span
          aria-hidden
          className={cn(
            "mt-0.5 size-3.5 shrink-0 rounded-full border",
            selected ? "border-4 border-foreground" : "border-muted-foreground/50",
          )}
        />
        {label}
      </button>
      {hint && <p className="mt-1 pl-5.5 text-label text-muted-foreground">{hint}</p>}
      {children}
    </div>
  );
}
