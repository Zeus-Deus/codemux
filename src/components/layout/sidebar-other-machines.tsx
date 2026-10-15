import { useMemo, useState } from "react";
import { ChevronRight, Monitor } from "lucide-react";

import { Eyebrow } from "@/components/ui/eyebrow";
import {
  useEnvironmentsStore,
  type EnvironmentStatus,
  type EnvironmentView,
} from "@/environments/store";
import {
  getWorkspaceStatus,
  statusRank,
  STATUS_DOT_CLASS,
  STATUS_LABEL,
  STATUS_TEXT_CLASS,
} from "@/lib/pane-status";
import { cn } from "@/lib/utils";
import type { ActivePaneStatus, WorkspaceSnapshot } from "@/tauri/types";

/** What the machine header says about its connection; `null` when live. */
export function machineStatusLabel(view: EnvironmentView): string | null {
  const labels: Record<EnvironmentStatus, string | null> = {
    connected: null,
    connecting: "Connecting…",
    reconnecting: "Reconnecting…",
    "pending-approval": "Waiting for approval",
    offline: "Offline",
    unauthorized: "Sign in again",
  };
  return labels[view.status];
}

interface MachineWorkspace {
  workspace: WorkspaceSnapshot;
  status: ActivePaneStatus | null;
}

/** Project workspaces of one machine, most demanding first. */
export function machineWorkspaces(view: EnvironmentView): MachineWorkspace[] {
  const state = view.appState;
  if (!state) return [];
  return state.workspaces
    .filter((workspace) => workspace.workspace_type !== "home")
    .map((workspace) => ({
      workspace,
      status: getWorkspaceStatus(workspace.surfaces, state.pane_statuses ?? {}),
    }))
    .sort(
      (a, b) =>
        statusRank(b.status) - statusRank(a.status) ||
        a.workspace.title.localeCompare(b.workspace.title),
    );
}

function projectName(workspace: WorkspaceSnapshot): string | null {
  const root = workspace.project_root ?? workspace.cwd;
  const parts = root.split("/").filter(Boolean);
  return parts[parts.length - 1] ?? null;
}

function MachineSection({ view }: { view: EnvironmentView }) {
  const [collapsed, setCollapsed] = useState(false);
  const rows = useMemo(() => machineWorkspaces(view), [view]);
  const statusLabel = machineStatusLabel(view);
  const live = view.status === "connected";

  return (
    <section data-machine={view.device.deviceId} aria-label={view.device.name}>
      <button
        type="button"
        onClick={() => setCollapsed((c) => !c)}
        aria-expanded={!collapsed}
        className="flex w-full items-center gap-2 px-1 pb-1.5 pt-3"
      >
        <ChevronRight
          aria-hidden="true"
          className={cn(
            "size-3 shrink-0 text-muted-foreground/70 transition-transform duration-150",
            !collapsed && "rotate-90",
          )}
        />
        <Monitor aria-hidden="true" className="size-3 shrink-0 text-muted-foreground/70" />
        <Eyebrow className="shrink-0 whitespace-nowrap">{view.device.name}</Eyebrow>
        {statusLabel && (
          <span
            title={
              view.status === "pending-approval"
                ? `Approve this device in Codemux on ${view.device.name}`
                : undefined
            }
            className="truncate text-caption text-muted-foreground/70"
          >
            {statusLabel}
          </span>
        )}
        <span className="h-px flex-1 bg-border/60" />
      </button>
      {!collapsed && (
        <ul className={cn("flex flex-col", !live && "opacity-60")}>
          {rows.map(({ workspace, status }) => {
            const project = projectName(workspace);
            return (
              <li
                key={workspace.workspace_id}
                data-machine-workspace={workspace.workspace_id}
                className="flex h-8 items-center gap-2 rounded-lg px-2 text-label"
              >
                <span
                  aria-hidden="true"
                  className={cn(
                    "size-1.5 shrink-0 rounded-full",
                    status ? STATUS_DOT_CLASS[status] : "bg-muted-foreground/30",
                  )}
                />
                <span className="min-w-0 flex-1 truncate">{workspace.title}</span>
                {status ? (
                  <span className={cn("shrink-0 text-caption", STATUS_TEXT_CLASS[status])}>
                    {STATUS_LABEL[status]}
                  </span>
                ) : (
                  project && (
                    <span className="shrink-0 truncate text-caption text-muted-foreground/70">
                      {project}
                    </span>
                  )
                )}
              </li>
            );
          })}
          {rows.length === 0 && view.appState && (
            <li className="px-2 py-1 text-caption text-muted-foreground/70">No workspaces</li>
          )}
        </ul>
      )}
    </section>
  );
}

/** Live workspaces of the account's other machines, one section per machine. */
export function SidebarOtherMachines() {
  const environments = useEnvironmentsStore((s) => s.environments);
  const views = useMemo(
    () =>
      Object.values(environments).sort((a, b) => a.device.name.localeCompare(b.device.name)),
    [environments],
  );
  if (views.length === 0) return null;
  return (
    <div data-other-machines className="flex flex-col">
      {views.map((view) => (
        <MachineSection key={view.device.deviceId} view={view} />
      ))}
    </div>
  );
}
