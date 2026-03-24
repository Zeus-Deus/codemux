import { useState, useMemo } from "react";
import { useAppStore } from "@/stores/app-store";
import { activateWorkspace } from "@/tauri/commands";
import { Workflow, ChevronRight } from "lucide-react";

export function SidebarOpenflowSection() {
  const appState = useAppStore((s) => s.appState);
  const [expanded, setExpanded] = useState(false);

  const openflowWorkspaces = useMemo(
    () =>
      appState?.workspaces.filter((w) => w.workspace_type === "open_flow") ??
      [],
    [appState],
  );

  return (
    <div className="shrink-0 border-t border-sidebar-border">
      <button
        className="flex w-full items-center gap-1.5 px-3 py-1.5 text-left text-[11px] font-semibold uppercase tracking-wider text-sidebar-foreground/70 hover:text-sidebar-foreground transition-colors"
        onClick={() => setExpanded(!expanded)}
      >
        <ChevronRight
          className={`h-3 w-3 shrink-0 transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <span>OpenFlow</span>
        {openflowWorkspaces.length > 0 && (
          <span className="ml-auto text-[10px] font-semibold tabular-nums text-muted-foreground">
            {openflowWorkspaces.length}
          </span>
        )}
      </button>

      {expanded && (
        <div className="px-3 pb-2">
          {openflowWorkspaces.length === 0 ? (
            <div className="flex items-center gap-1.5 py-1 text-muted-foreground">
              <Workflow className="h-3.5 w-3.5" />
              <span className="text-xs">No active runs</span>
            </div>
          ) : (
            <div className="space-y-0.5">
              {openflowWorkspaces.map((ws) => (
                <button
                  key={ws.workspace_id}
                  className="flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left text-xs hover:bg-accent/20 transition-colors"
                  onClick={() =>
                    activateWorkspace(ws.workspace_id).catch(console.error)
                  }
                >
                  <Workflow className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                  <span className="truncate text-foreground">{ws.title}</span>
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
