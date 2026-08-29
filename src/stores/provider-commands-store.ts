import { create } from "zustand";

import {
  listChatSlashCommands,
  type ProviderSlashCommand,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * Provider-native slash commands (Claude Code's `/compact`, `/init`,
 * `/review`, custom `.claude/commands` entries, …), lazily discovered
 * by calling the Rust `list_chat_slash_commands` command the first
 * time the composer's slash popup opens for a provider + cwd pair.
 *
 * Sibling of `skills-store` — same lazy-load-on-popup-open shape, but
 * keyed per `(provider, cwd)` because project-scoped custom commands
 * are cwd-sensitive and each provider reports its own vocabulary.
 *
 * Static provider catalogues stay cached for the app's lifetime. Grok's ACP
 * runtime can replace its backend snapshot, so the composer force-refreshes
 * this inexpensive IPC read whenever its popup reopens. A stale cache clears
 * on `invalidate()` (used by tests) or an app restart.
 */
interface ProviderCommandsEntry {
  commands: ProviderSlashCommand[];
  loaded: boolean;
  loading: boolean;
  error: string | null;
}

interface ProviderCommandsState {
  /** Keyed by `${provider}\n${cwd}`. */
  entries: Record<string, ProviderCommandsEntry>;

  loadCommands: (
    provider: AgentChatProviderKind,
    cwd: string | null,
    force?: boolean,
  ) => Promise<void>;
  /** Drop every cached entry. Next `loadCommands` refetches. */
  invalidate: () => void;
}

const EMPTY_ENTRY: ProviderCommandsEntry = {
  commands: [],
  loaded: false,
  loading: false,
  error: null,
};

export const commandsKey = (
  provider: AgentChatProviderKind,
  cwd: string,
): string => `${provider}\n${cwd}`;

export const useProviderCommandsStore = create<ProviderCommandsState>()(
  (set, get) => ({
    entries: {},

    loadCommands: async (provider, cwd, force = false) => {
      // No cwd (Home draft with no project anchored) → nothing to
      // probe against; project commands are cwd-relative.
      if (!cwd) return;
      const key = commandsKey(provider, cwd);
      const entry = get().entries[key] ?? EMPTY_ENTRY;

      if (entry.loading) return;
      if (entry.loaded && !force) return;

      set((s) => ({
        entries: {
          ...s.entries,
          [key]: { ...entry, loading: true, error: null },
        },
      }));
      try {
        const commands = await listChatSlashCommands(provider, cwd);
        set((s) => ({
          entries: {
            ...s.entries,
            [key]: {
              commands,
              loaded: true,
              loading: false,
              error: null,
            },
          },
        }));
      } catch (err) {
        set((s) => ({
          entries: {
            ...s.entries,
            [key]: {
              ...(s.entries[key] ?? EMPTY_ENTRY),
              loading: false,
              error: err instanceof Error ? err.message : String(err),
            },
          },
        }));
      }
    },

    invalidate: () => {
      set({ entries: {} });
    },
  }),
);

/** Selector factory: the entry for a provider + cwd pair, or the
 *  stable empty entry when nothing has been fetched yet. */
export const selectProviderCommands =
  (provider: AgentChatProviderKind, cwd: string | null) =>
  (s: ProviderCommandsState): ProviderCommandsEntry => {
    if (!cwd) return EMPTY_ENTRY;
    return s.entries[commandsKey(provider, cwd)] ?? EMPTY_ENTRY;
  };
