import { createContext } from "react";
import type { DurableTaskContext } from "@/lib/agent-chat/durable-tasks";
export const DurableTasksContext = createContext<DurableTaskContext>({
  tasks: [],
  origin: null,
});
