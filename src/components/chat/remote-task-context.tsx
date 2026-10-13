import { createContext, useContext } from "react";
import type { LocalTask } from "@/lib/delegation";
import { RemoteTaskCard } from "./RemoteTaskCard";

/** Published inside the transcript portal: the pane's DOM ancestry is not authority. */
export interface RemoteTaskScope {
  paneId: string;
  writable: boolean;
  onOpen: (taskId: string) => void;
}
export const RemoteTaskContext = createContext<RemoteTaskScope | null>(null);

export function RemoteTaskRow({ task }: { task: LocalTask }) {
  const scope = useContext(RemoteTaskContext);
  return (
    <RemoteTaskCard
      task={task}
      paneId={scope?.paneId ?? ""}
      writable={scope?.writable ?? false}
      onOpen={task => scope?.onOpen(task.id)}
    />
  );
}
