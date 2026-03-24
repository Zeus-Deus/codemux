import { create } from "zustand";
import {
  createResolverBranch,
  resolveConflictsWithAgent,
  applyResolution,
  abortResolution,
  getResolutionDiff,
} from "@/tauri/commands";
import type { ConflictFile } from "@/tauri/types";

export interface ResolverState {
  status: "idle" | "creating_branch" | "resolving" | "review" | "applying" | "error";
  tempBranch: string | null;
  originalBranch: string | null;
  targetBranch: string | null;
  conflictingFiles: ConflictFile[];
  agentOutput: string | null;
  resolutionDiff: string | null;
  error: string | null;
}

const IDLE: ResolverState = {
  status: "idle",
  tempBranch: null,
  originalBranch: null,
  targetBranch: null,
  conflictingFiles: [],
  agentOutput: null,
  resolutionDiff: null,
  error: null,
};

interface AiMergeStore {
  resolvers: Record<string, ResolverState>;
  getResolver: (workspaceId: string) => ResolverState;
  startResolution: (
    workspaceId: string,
    cwd: string,
    targetBranch: string,
    cli: string,
    model: string | null,
    strategy: string,
  ) => void;
  approveResolution: (workspaceId: string, cwd: string, message: string) => Promise<void>;
  rejectResolution: (workspaceId: string, cwd: string) => Promise<void>;
  clearResolver: (workspaceId: string) => void;
}

export const useAiMergeStore = create<AiMergeStore>()((set, get) => ({
  resolvers: {},

  getResolver: (workspaceId) => get().resolvers[workspaceId] ?? IDLE,

  startResolution: (workspaceId, cwd, targetBranch, cli, model, strategy) => {
    // Set creating_branch status
    set((s) => ({
      resolvers: {
        ...s.resolvers,
        [workspaceId]: { ...IDLE, status: "creating_branch" },
      },
    }));

    // Step 1: Create temp branch and start merge
    createResolverBranch(cwd, targetBranch)
      .then((info) => {
        const files = info.conflicting_files.map((f) => f.path);

        set((s) => ({
          resolvers: {
            ...s.resolvers,
            [workspaceId]: {
              ...IDLE,
              status: "resolving",
              tempBranch: info.temp_branch,
              originalBranch: info.original_branch,
              targetBranch: info.target_branch,
              conflictingFiles: info.conflicting_files,
            },
          },
        }));

        // Step 2: Spawn agent to resolve
        return resolveConflictsWithAgent(cwd, cli, model, strategy, files);
      })
      .then((output) => {
        const state = get().resolvers[workspaceId];
        if (!state || state.status !== "resolving") return;

        // Fetch the resolution diff
        return getResolutionDiff(state.tempBranch ? cwd : cwd)
          .catch(() => "")
          .then((diff) => {
            set((s) => ({
              resolvers: {
                ...s.resolvers,
                [workspaceId]: {
                  ...s.resolvers[workspaceId],
                  status: "review",
                  agentOutput: output,
                  resolutionDiff: diff,
                },
              },
            }));
          });
      })
      .catch((err) => {
        set((s) => ({
          resolvers: {
            ...s.resolvers,
            [workspaceId]: {
              ...s.resolvers[workspaceId],
              status: "error",
              error: String(err),
            },
          },
        }));
      });
  },

  approveResolution: async (workspaceId, cwd, message) => {
    const state = get().resolvers[workspaceId];
    if (!state?.tempBranch || !state?.originalBranch) return;

    set((s) => ({
      resolvers: {
        ...s.resolvers,
        [workspaceId]: { ...s.resolvers[workspaceId], status: "applying" },
      },
    }));

    try {
      await applyResolution(cwd, state.tempBranch, state.originalBranch, message);
      set((s) => {
        const { [workspaceId]: _, ...rest } = s.resolvers;
        return { resolvers: rest };
      });
    } catch (err) {
      set((s) => ({
        resolvers: {
          ...s.resolvers,
          [workspaceId]: {
            ...s.resolvers[workspaceId],
            status: "error",
            error: String(err),
          },
        },
      }));
    }
  },

  rejectResolution: async (workspaceId, cwd) => {
    const state = get().resolvers[workspaceId];
    if (!state?.tempBranch || !state?.originalBranch) return;

    try {
      await abortResolution(cwd, state.tempBranch, state.originalBranch);
    } catch (err) {
      console.error("Failed to abort resolution:", err);
    }

    set((s) => {
      const { [workspaceId]: _, ...rest } = s.resolvers;
      return { resolvers: rest };
    });
  },

  clearResolver: (workspaceId) =>
    set((s) => {
      const { [workspaceId]: _, ...rest } = s.resolvers;
      return { resolvers: rest };
    }),
}));
