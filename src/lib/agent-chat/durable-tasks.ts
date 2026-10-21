import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface DurableTask {
  task_id: string;
  backend: "local" | "ssh";
  parent_thread_id: string;
  child_thread_id: string | null;
  title: string;
  provider: string;
  model?: string | null;
  effort?: string | null;
  status:
    | "queued"
    | "running"
    | "waiting"
    | "completed"
    | "failed"
    | "cancelled"
    | "interrupted";
  progress?: string | null;
  result?: { text: string; version: number } | null;
  created_at: number;
  started_at?: number | null;
  completed_at?: number | null;
  pending_request?: { id: string; kind: string; created_at: number } | null;
  tool_call_id?: string | null;
  remote_task_id?: string | null;
  host?: { name: string } | null;
  cwd?: string;
  isolation?: string;
  worktree?: { path?: string; branch?: string } | null;
}
export interface ChildOrigin {
  relationship: "subagent";
  parent_thread_id: string;
  parent_title: string | null;
  parent_exists: boolean;
  task_id: string;
}
export interface DurableTaskContext {
  tasks: DurableTask[];
  origin: ChildOrigin | null;
}
const EMPTY: DurableTaskContext = { tasks: [], origin: null };

/** Read-only UI inspection never acknowledges a terminal task result. */
export function useDurableTasks(
  threadId: string | null | undefined,
): DurableTaskContext {
  const [value, setValue] = useState<{
    thread: string;
    context: DurableTaskContext;
  } | null>(null);
  useEffect(() => {
    if (!threadId) return;
    let disposed = false;
    let revision = 0;
    const reload = () => {
      const current = ++revision;
      void invoke<DurableTaskContext>("delegation_local_context", { threadId })
        .then((context) => {
          if (!disposed && current === revision)
            setValue({ thread: threadId, context });
        })
        .catch(() => {});
    };
    const unlisten = listen<{ parent_thread_id: string }>(
      "delegation-changed",
      (event) => {
        if (event.payload.parent_thread_id === threadId) reload();
      },
    );
    void unlisten
      .then(() => {
        if (!disposed) reload();
      })
      .catch(() => reload());
    return () => {
      disposed = true;
      void unlisten.then((stop) => stop()).catch(() => {});
    };
  }, [threadId]);
  return value && value.thread === threadId ? value.context : EMPTY;
}
