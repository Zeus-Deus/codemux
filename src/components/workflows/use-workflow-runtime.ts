import { randomUUID } from "@/lib/uuid";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  workflowCancel,
  workflowCancelTask,
  workflowCapabilities,
  workflowGet,
  workflowList,
  workflowMessage,
  workflowPause,
  workflowResume,
  workflowReconcile,
  workflowRetire,
  workflowRetry,
  workflowScriptRun,
  workflowScriptsList,
  workflowScriptSave,
  type WorkflowChangeEvent,
  type WorkflowRunSnapshot,
  type WorkflowRunSpec,
} from "@/tauri/workflows";
import { useWorkflowUIStore } from "@/stores/workflow-ui-store";

function pollingInterval(run: WorkflowRunSnapshot | undefined): number | false {
  if (!run) return false;
  if (run.status === "unknown") return 5000;
  if (["running", "stopping"].includes(run.status)) return 1000;
  return run.status === "paused" && run.tasks.some((task) => ["running", "unknown", "stopping"].includes(task.status)) ? 1000 : false;
}

export function useWorkflowRuntime(workspaceId: string) {
  const client = useQueryClient();
  const selectedId = useWorkflowUIStore((state) => state.selectedRuns[workspaceId] ?? null);
  const select = useWorkflowUIStore((state) => state.selectRun);
  const key = ["workflows", workspaceId] as const;
  const runs = useQuery({
    queryKey: [...key, "runs"],
    queryFn: () => workflowList(workspaceId),
  });
  const snapshot = useQuery({
    queryKey: [...key, "run", selectedId],
    queryFn: () => workflowGet(selectedId!),
    enabled: !!selectedId,
    structuralSharing: (previous, incoming) => {
      const old = previous as WorkflowRunSnapshot | undefined;
      const next = incoming as WorkflowRunSnapshot;
      return old && old.revision >= next.revision ? old : next;
    },
    refetchInterval: (query) => pollingInterval(query.state.data),
  });
  useEffect(() => {
    if (snapshot.data && ["completed", "failed", "cancelled"].includes(snapshot.data.status)) {
      void client.invalidateQueries({ queryKey: ["workflows", workspaceId, "runs"] });
    }
  }, [client, workspaceId, snapshot.data?.id, snapshot.data?.status]);
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    let pending: ReturnType<typeof setTimeout> | undefined;
    let refreshHistory = false;
    let refreshRun = false;
    const queue = () => {
      if (pending) return;
      pending = setTimeout(() => {
        pending = undefined;
        if (disposed) return;
        if (refreshRun && selectedId) void client.invalidateQueries({ queryKey: ["workflows", workspaceId, "run", selectedId] });
        if (refreshHistory) void client.invalidateQueries({ queryKey: ["workflows", workspaceId, "runs"] });
        refreshRun = false;
        refreshHistory = false;
      }, 250);
    };
    void listen<WorkflowChangeEvent>("workflow-changed", ({ payload }) => {
      if (disposed) return;
      if (payload.kind === "created") refreshHistory = true;
      if (payload.run_id === null) {
        refreshRun = !!selectedId;
        // Host-wide notifications can describe another run finishing while
        // the selected terminal run has stopped polling.
        refreshHistory = true;
      }
      if (payload.run_id !== null && payload.run_id === selectedId) {
        const cached = client.getQueryData<WorkflowRunSnapshot>(["workflows", workspaceId, "run", selectedId]);
        if (!cached || payload.revision === null || payload.revision > cached.revision) refreshRun = true;
      }
      if (refreshHistory || refreshRun) queue();
    }).then((stop) => { if (disposed) stop(); else unlisten = stop; }).catch(() => {
      // Polling remains available if this runtime cannot deliver events.
    });
    return () => { disposed = true; if (pending) clearTimeout(pending); unlisten?.(); };
  }, [client, workspaceId, selectedId]);
  const capabilities = useQuery({ queryKey: ["workflow-capabilities"], queryFn: workflowCapabilities, staleTime: 60_000 });
  const scripts = useQuery({ queryKey: [...key, "scripts"], queryFn: () => workflowScriptsList(workspaceId) });
  const accept = (run: WorkflowRunSnapshot) => {
    client.setQueryData<WorkflowRunSnapshot>([...key, "run", run.id], (old) => old && old.revision > run.revision ? old : run);
    select(workspaceId, run.id);
    void client.invalidateQueries({ queryKey: [...key, "runs"] });
  };
  const launch = useMutation({
    mutationFn: ({ spec, source }: { spec: WorkflowRunSpec; source: string }) =>
      workflowScriptRun(spec, source, { routes: spec.routes.map((route) => route.id) }, randomUUID()),
    onSuccess: accept,
  });
  const control = useMutation({
    mutationFn: ({ action, runId, taskId, text }: { action: "pause" | "resume" | "cancel" | "retry" | "retire" | "message" | "cancel_task" | "reconcile"; runId: string; taskId?: string; text?: string }) => {
      switch (action) {
        case "pause": return workflowPause(runId);
        case "resume": return workflowResume(runId);
        case "cancel": return workflowCancel(runId);
        case "cancel_task": return workflowCancelTask(runId, taskId!);
        case "reconcile": return workflowReconcile(runId, taskId!);
        case "retry": return workflowRetry(runId, taskId!);
        case "retire": return workflowRetire(runId, taskId!);
        case "message": return workflowMessage(runId, taskId!, text!);
      }
    },
    onSuccess: accept,
  });
  const save = useMutation({
    mutationFn: ({ name, source }: { name: string; source: string }) => workflowScriptSave(workspaceId, name, source),
    onSuccess: () => { void client.invalidateQueries({ queryKey: [...key, "scripts"] }); },
  });
  useEffect(() => {
    // Mutation feedback belongs to the selected run/setup, including selection
    // changes made through the pane toolbar rather than this hook's helper.
    launch.reset();
    control.reset();
    save.reset();
  }, [selectedId, launch.reset, control.reset, save.reset]);
  return { runs, snapshot, capabilities, scripts, launch, control, save, selectedId, selectRun: (id: string | null) => { launch.reset(); control.reset(); save.reset(); select(workspaceId, id); } };
}
