import { useMobileNavigationStore } from "@/stores/mobile-navigation-store";
import { MobileWorkspaceActions } from "./mobile-workspace-actions";
import { CloneDialog } from "@/components/overlays/clone-dialog";
import { BrowserPeekOverlay } from "@/components/browser/BrowserPeekOverlay";
import { useBrowserPeekStore } from "@/stores/browser-peek-store";
import { useEffect, useRef, useState, type ReactNode } from "react";
import {
  ArrowLeft,
  Bell,
  Check,
  ChevronRight,
  ChevronDown,
  MessageSquare,
  Globe,
  ListTodo,
  Users,
  Code2,
  Folder,
  GitBranch,
  Keyboard,
  MoreHorizontal,
  Plus,
  Search,
  Settings,
  Undo2,
} from "lucide-react";
import { useActiveWorkspace, useAppStore } from "@/stores/app-store";
import { useUIStore, type RightPanelTab } from "@/stores/ui-store";
import { useSidebarInboxStore } from "@/stores/sidebar-inbox-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { activateWorkspace } from "@/tauri/commands";
import { getWorkspaceStatus, STATUS_LABEL } from "@/lib/pane-status";
import { WorkspaceMain } from "@/components/layout/workspace-main";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
} from "@/components/ui/dialog";
import { useRemoteConnectionStore } from "@/remote/remote-connection-store";
import { toast } from "@/lib/toast";
import { MobileInstall } from "./mobile-install";
import { MobileSessionSheet } from "./mobile-session-sheet";
import { WorkspaceStatusCluster } from "@/components/chat/WorkspaceStatusCluster";
import {
  Sheet,
  SheetContent,
  SheetHeader,
  SheetTitle,
  SheetDescription,
} from "@/components/ui/sheet";

export function MobileShell({ overlays }: { overlays: ReactNode }) {
  const workspace = useActiveWorkspace();
  const newWorkspaceOpen = useUIStore((s) => s.showNewWorkspaceDialog);
  const newWorkspaceProjectDir = useUIStore((s) => s.newWorkspaceProjectDir);
  const peek = useBrowserPeekStore((s) => s.openWorkspaceId);
  const snapshot = useAppStore((s) => s.appState);
  const draft = useChatDraftStore((s) => s.activeDraftId);
  const home = useMobileNavigationStore((s) => s.home);
  const [openedWorkspace, setOpenedWorkspace] = useState(!home || !!draft);
  useEffect(() => {
    if (!home || draft) setOpenedWorkspace(true);
  }, [home, draft]);
  const setHome = useMobileNavigationStore((s) => s.setHome);
  const [filter, setFilter] = useState("all");
  const [search, setSearch] = useState("");
  const [more, setMore] = useState(false);
  const [install, setInstall] = useState(false);
  const [sessions, setSessions] = useState(false);
  const toolsButton = useRef<HTMLButtonElement>(null);
  const sessionButton = useRef<HTMLButtonElement>(null);
  const settled = useSidebarInboxStore((s) => s.settled);
  const inboxLoaded = useSidebarInboxStore((s) => s.loaded);
  const connection = useRemoteConnectionStore((s) => s.status);
  const panel = useUIStore((s) =>
    workspace ? s.rightPanelTabs[workspace.workspace_id] : null,
  );
  const ui = useUIStore.getState;
  useEffect(() => {
    void useSidebarInboxStore.getState().load();
  }, []);
  useEffect(() => {
    if (!inboxLoaded || !snapshot) return;
    for (const entry of settled) {
      const workspace = snapshot.workspaces.find(
        (w) => w.workspace_id === entry.id,
      );
      const status =
        workspace &&
        getWorkspaceStatus(workspace.surfaces, snapshot.pane_statuses);
      if (status === "working" || status === "permission")
        useSidebarInboxStore.getState().unsettle(entry.id, "activity");
    }
  }, [inboxLoaded, snapshot, settled]);
  const startChat = (projectDir?: string | null) => {
    const store = useChatDraftStore.getState();
    const next = projectDir
      ? store.getOrCreateProjectDraft(projectDir)
      : store.getOrCreateHomeDraft({ lockedToHome: true });
    store.setActiveDraft(next.draftId);
    setHome(false);
    setMore(false);
  };
  // Shared commands and project actions also enter the GUI flow on mobile.
  useEffect(() => {
    if (!newWorkspaceOpen) return;
    startChat(newWorkspaceProjectDir);
    ui().setShowNewWorkspaceDialog(false);
  }, [newWorkspaceOpen, newWorkspaceProjectDir]);
  // Creating a workspace or following a notification should enter its content.
  useEffect(() => {
    if (draft) setHome(false);
  }, [draft]);
  const openWorkspace = async (id: string) => {
    try {
      await activateWorkspace(id);
      setHome(false);
    } catch (error) {
      toast.error("Could not open workspace", { description: String(error) });
    }
  };
  const selectPanel = (tab: RightPanelTab | null) => {
    if (workspace) ui().setRightPanelTab(workspace.workspace_id, tab);
  };
  const workspaces = (snapshot?.workspaces ?? []).filter((w) => {
    const status = getWorkspaceStatus(
      w.surfaces,
      snapshot?.pane_statuses ?? {},
    );
    const isSettled = settled.some((s) => s.id === w.workspace_id);
    return (
      `${w.title} ${w.project_root ?? w.cwd}`
        .toLowerCase()
        .includes(search.toLowerCase()) &&
      (filter === "all" ||
        (filter === "attention" &&
          (status === "permission" || status === "review")) ||
        (filter === "working" &&
          (status === "working" || status === "monitoring")) ||
        (filter === "settled" && isSettled))
    );
  });
  return (
    <div className="mobile-shell">
      <header className="mobile-header">
        {!home && (
          <button
            aria-label={
              panel && !draft ? "Back to conversation" : "All workspaces"
            }
            onClick={() =>
              panel && !draft ? selectPanel(null) : setHome(true)
            }
          >
            <ArrowLeft size={20} />
          </button>
        )}
        {!home && workspace && !draft ? (
          <button
            ref={sessionButton}
            className="mobile-title"
            aria-label="Switch session"
            aria-haspopup="dialog"
            aria-expanded={sessions}
            onClick={() => setSessions(true)}
          >
            <span className="min-w-0">
              <span className="block truncate font-semibold">
                {workspace.title}
              </span>
              <span className="block truncate text-xs text-muted-foreground">
                {connection === "reconnecting"
                  ? "Reconnecting…"
                  : connection === "offline"
                    ? "Disconnected"
                    : (workspace.tabs.find(
                        (t) => t.tab_id === workspace.active_tab_id,
                      )?.title ?? "Sessions")}
              </span>
            </span>
            <ChevronDown size={14} className="shrink-0 text-muted-foreground" />
          </button>
        ) : (
          <div className="min-w-0 flex-1 truncate font-semibold">
            {home ? "Codemux" : "New workspace"}
          </div>
        )}
        {home && (
          <button
            aria-label="Install and notifications"
            onClick={() => setInstall(true)}
          >
            <Bell size={20} />
          </button>
        )}
        {!home && (
          <button
            className="mobile-keyboard-dismiss"
            aria-label="Hide keyboard"
            onClick={() => (document.activeElement as HTMLElement)?.blur()}
          >
            <Keyboard size={19} />
          </button>
        )}
        <button
          ref={toolsButton}
          className="mobile-tools-trigger"
          aria-label={home ? "Actions" : "Workspace tools"}
          aria-haspopup="dialog"
          aria-expanded={more}
          onClick={() => setMore(true)}
        >
          {home ? (
            <MoreHorizontal size={20} />
          ) : (
            <>
              <Folder size={17} />
              <span>Tools</span>
            </>
          )}
        </button>
      </header>
      {home ? (
        <main className="mobile-home">
          <div className="flex items-center justify-between gap-3">
            <h1 className="text-2xl font-semibold tracking-tight">
              Workspaces
            </h1>
            <button className="mobile-primary" onClick={() => startChat()}>
              <Plus size={18} />
              New
            </button>
          </div>
          <label className="mobile-search">
            <Search size={18} />
            <input
              aria-label="Search workspaces"
              placeholder="Find a workspace…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
            />
          </label>
          <div className="mobile-filters" aria-label="Filter workspaces">
            {[
              ["all", "All"],
              ["attention", "Needs you"],
              ["working", "Working"],
              ["settled", "Settled"],
            ].map(([id, label]) => (
              <button
                key={id}
                aria-pressed={filter === id}
                onClick={() => setFilter(id)}
              >
                {label}
              </button>
            ))}
          </div>
          <div className="mobile-workspaces">
            {workspaces.map((w) => {
              const status = getWorkspaceStatus(
                w.surfaces,
                snapshot?.pane_statuses ?? {},
              );
              const isSettled = settled.some((s) => s.id === w.workspace_id);
              return (
                <article key={w.workspace_id} className="mobile-workspace">
                  <button
                    className="mobile-workspace-open"
                    onClick={() => void openWorkspace(w.workspace_id)}
                  >
                    <span className="mobile-project-icon">
                      <Code2 size={21} />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium">
                        {w.title}
                      </span>
                      <span className="block truncate text-xs text-muted-foreground mt-1">
                        {(w.project_root ?? w.cwd).split(/[\\/]/).pop()} ·{" "}
                        {status
                          ? STATUS_LABEL[status]
                          : isSettled
                            ? "Settled"
                            : "Ready"}
                      </span>
                      {w.git_changed_files > 0 && (
                        <span className="block text-xs text-muted-foreground mt-1">
                          {w.git_changed_files} changed files
                        </span>
                      )}
                    </span>
                    <ChevronRight size={16} />
                  </button>
                  <button
                    className="mobile-settle"
                    disabled={
                      !isSettled &&
                      (status === "working" ||
                        status === "permission" ||
                        status === "monitoring")
                    }
                    aria-label={`${isSettled ? "Unsettle" : "Settle"} ${w.title}`}
                    onClick={() =>
                      isSettled
                        ? useSidebarInboxStore
                            .getState()
                            .unsettle(w.workspace_id, "user")
                        : useSidebarInboxStore.getState().settle(w.workspace_id)
                    }
                  >
                    {isSettled ? <Undo2 size={16} /> : <Check size={16} />}
                  </button>
                </article>
              );
            })}
            {workspaces.length === 0 && (
              <p className="py-12 text-center text-muted-foreground">
                No workspaces here yet.
              </p>
            )}
          </div>
          <MobileInstall compact />
        </main>
      ) : null}
      {openedWorkspace && (
        <div
          className="mobile-content min-h-0 flex-1 flex flex-col"
          style={{ display: home ? "none" : "flex" }}
          inert={home || undefined}
        >
          <WorkspaceMain mobile />
        </div>
      )}
      {workspace && !draft && (
        <MobileSessionSheet
          workspace={workspace}
          open={sessions}
          onOpenChange={setSessions}
          returnFocusRef={sessionButton}
        />
      )}
      <Sheet open={more} onOpenChange={setMore}>
        <SheetContent
          side="bottom"
          className="mobile-bottom-sheet mobile-tools-sheet"
          onCloseAutoFocus={(event) => {
            event.preventDefault();
            if (!sessions) toolsButton.current?.focus();
          }}
        >
          <SheetHeader>
            <SheetTitle>
              {home || draft ? "Codemux" : "Workspace tools"}
            </SheetTitle>
            <SheetDescription>
              {!home && !draft && workspace
                ? workspace.title
                : "Projects, preferences and your desktop."}
            </SheetDescription>
          </SheetHeader>
          {workspace && !draft && !home && (
            <>
              <div className="mobile-tool-grid">
                {[
                  { tab: null, label: "Conversation", icon: MessageSquare },
                  { tab: "files" as const, label: "Files", icon: Folder },
                  {
                    tab: "changes" as const,
                    label: "Changes",
                    icon: GitBranch,
                  },
                  { tab: "review" as const, label: "Review", icon: Check },
                  { tab: "tasks" as const, label: "Tasks", icon: ListTodo },
                  {
                    tab: "subagents" as const,
                    label: "Subagents",
                    icon: Users,
                  },
                  { tab: "browser" as const, label: "Browser", icon: Globe },
                ].map(({ tab, label, icon: Icon }) => (
                  <button
                    key={label}
                    aria-current={
                      panel === tab || (!panel && tab === null)
                        ? "page"
                        : undefined
                    }
                    onClick={() => {
                      selectPanel(tab);
                      setMore(false);
                    }}
                  >
                    <Icon size={20} />
                    <span>{label}</span>
                  </button>
                ))}
                <button
                  onClick={() => {
                    setMore(false);
                    setSessions(true);
                  }}
                >
                  <MessageSquare size={20} />
                  <span>Sessions</span>
                </button>
              </div>
              <div className="mobile-workspace-context">
                <span className="text-xs text-muted-foreground">
                  {workspace.git_branch ?? workspace.cwd}
                </span>
                <WorkspaceStatusCluster />
              </div>
            </>
          )}
          <details className="mobile-more-actions" open={home || !!draft}>
            <summary>Workspace and app actions</summary>
            <div className="mobile-actions">
              <button
                onClick={() => {
                  ui().setShowCommandPalette(true);
                  setMore(false);
                }}
              >
                <Search size={18} />
                All commands
              </button>
              <button
                onClick={() => {
                  startChat();
                }}
              >
                <Plus size={18} />
                New workspace
              </button>
              <button
                onClick={() => {
                  ui().setShowNewProjectScreen(true);
                  setMore(false);
                }}
              >
                New project
              </button>
              <button
                onClick={() => {
                  ui().setShowCloneDialog(true);
                  setMore(false);
                }}
              >
                Clone repository
              </button>
              {workspace && !draft && !home && (
                <MobileWorkspaceActions
                  key={workspace.workspace_id}
                  workspace={workspace}
                  onDone={() => setMore(false)}
                />
              )}
              <button
                onClick={() => {
                  ui().setShowSettings(true);
                  setMore(false);
                }}
              >
                <Settings size={18} />
                Settings
              </button>
              <button
                onClick={() => {
                  ui().setShowAutomations(true);
                  setMore(false);
                }}
              >
                Automations
              </button>
              <button
                onClick={() => {
                  ui().setShowDevices(true);
                  setMore(false);
                }}
              >
                Devices
              </button>
              <button
                onClick={() => {
                  ui().setShowPullRequests(true);
                  setMore(false);
                }}
              >
                All pull requests
              </button>
              <button
                onClick={() => {
                  setMore(false);
                  setInstall(true);
                }}
              >
                <Bell size={18} />
                Install and notifications
              </button>
            </div>
          </details>
        </SheetContent>
      </Sheet>
      <Dialog open={install} onOpenChange={setInstall}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Codemux on your phone</DialogTitle>
            <DialogDescription>
              Install the web app and choose your notifications.
            </DialogDescription>
          </DialogHeader>
          <MobileInstall />
        </DialogContent>
      </Dialog>
      <CloneDialog />
      <Dialog
        open={!!peek}
        onOpenChange={(open) => {
          if (!open) useBrowserPeekStore.getState().closeAll();
        }}
      >
        <DialogContent className="mobile-browser-dialog">
          <DialogHeader>
            <DialogTitle>Browser preview</DialogTitle>
            <DialogDescription>
              The browser running on your desktop.
            </DialogDescription>
          </DialogHeader>
          <div className="min-h-0 flex-1 relative">
            <BrowserPeekOverlay />
          </div>
        </DialogContent>
      </Dialog>
      {overlays}
    </div>
  );
}
