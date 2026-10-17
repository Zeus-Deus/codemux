import { create } from "zustand";
import { generateAiCommitMessage } from "@/tauri/commands";

interface GenerationEntry {
  status: "generating" | "done" | "error";
  message?: string;
  error?: string;
}

interface AiCommitStore {
  generations: Record<string, GenerationEntry>;
  getGeneration: (workspaceId: string) => GenerationEntry | undefined;
  requestGeneration: (
    workspaceId: string,
    cwd: string,
    cli: string | null,
    model: string | null,
  ) => void;
  consumeMessage: (workspaceId: string) => string | undefined;
  clearGeneration: (workspaceId: string) => void;
}

export const useAiCommitStore = create<AiCommitStore>()((set, get) => ({
  generations: {},

  getGeneration: (workspaceId) => get().generations[workspaceId],

  requestGeneration: (workspaceId, cwd, cli, model) => {
    // The entry owns this request; clearing or replacing it invalidates late replies.
    const request: GenerationEntry = { status: "generating" };
    set((s) => ({
      generations: {
        ...s.generations,
        [workspaceId]: request,
      },
    }));

    generateAiCommitMessage(cwd, cli, model)
      .then((message) => {
        set((s) => {
          if (s.generations[workspaceId] !== request) return s;
          return {
            generations: { ...s.generations, [workspaceId]: { status: "done", message } },
          };
        });
      })
      .catch((err) => {
        set((s) => {
          if (s.generations[workspaceId] !== request) return s;
          return {
            generations: { ...s.generations, [workspaceId]: { status: "error", error: String(err) } },
          };
        });
      });
  },

  consumeMessage: (workspaceId) => {
    const entry = get().generations[workspaceId];
    if (entry?.status === "done" && entry.message) {
      const msg = entry.message;
      set((s) => {
        const { [workspaceId]: _, ...rest } = s.generations;
        return { generations: rest };
      });
      return msg;
    }
    return undefined;
  },

  clearGeneration: (workspaceId) =>
    set((s) => {
      const { [workspaceId]: _, ...rest } = s.generations;
      return { generations: rest };
    }),
}));
