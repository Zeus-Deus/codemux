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
 * keyed per provider/directory and, for ACP providers, conversation.
 * Home resolves to the user's home directory on the backend.
 *
 * Provider catalogues expire after one minute. An ACP
 * runtime can replace its backend snapshot at any point in a session, so the
 * composer force-refreshes this inexpensive IPC read whenever its popup
 * reopens. A stale cache clears on `invalidate()` (used by tests) or an app
 * restart.
 */
interface ProviderCommandsEntry {
  commands: ProviderSlashCommand[];
  loaded: boolean;
  loadedAt?: number;
  loading: boolean;
  error: string | null;
}

interface ProviderCommandsState {
  /** Provider + directory + optional ACP conversation. */
  entries: Record<string, ProviderCommandsEntry>;
  generation: number;

  loadCommands: (
    provider: AgentChatProviderKind,
    cwd: string | null,
    force?: boolean,
    threadId?: string | null,
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

/**
 * Whether this provider's catalogue is pushed by a live agent session.
 *
 * For these providers an empty answer only means "no session has published
 * yet", not "there are no commands", so it must not be memoised as final —
 * otherwise the first read, taken before the session starts, hides the real
 * catalogue for the rest of the app's lifetime. Re-reading is cheap: the
 * backend answers from its in-memory snapshot without spawning anything.
 */
export function catalogueFollowsSession(
  provider: AgentChatProviderKind,
): boolean {
  return provider === "grok" || provider === "cursor" || provider === "hermes";
}

export const commandsKey = (
  provider: AgentChatProviderKind,
  cwd: string | null,
  threadId: string | null = null,
): string => `${provider}\n${cwd ?? "<home>"}${catalogueFollowsSession(provider) ? `\n${threadId ?? ""}` : ""}`;

export const useProviderCommandsStore = create<ProviderCommandsState>()(
  (set, get) => ({
    entries: {},
    generation: 0,

    loadCommands: async (provider, cwd, force = false, threadId = null) => {
      // Home uses the backend's home directory, just like skill discovery.
      const key = commandsKey(provider, cwd, threadId);
      const entry = get().entries[key] ?? EMPTY_ENTRY;

      if (entry.loading) return;
      if (entry.loaded && !entry.error && !force && Date.now() - (entry.loadedAt ?? 0) < 60_000) return;
      const generation = get().generation;

      set((s) => ({
        entries: {
          ...s.entries,
          [key]: { ...entry, loading: true, error: null },
        },
      }));
      try {
        const commands = await (catalogueFollowsSession(provider)
          ? listChatSlashCommands(provider, cwd, force, threadId)
          : listChatSlashCommands(provider, cwd, force));
        if (get().generation !== generation) return;
        set((s) => ({
          entries: {
            ...s.entries,
            [key]: {
              commands,
              loaded: true,
              loadedAt: Date.now(),
              loading: false,
              error: null,
            },
          },
        }));
      } catch (err) {
        if (get().generation !== generation) return;
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
      set((s) => ({ entries: {}, generation: s.generation + 1 }));
    },
  }),
);

/** Selector factory: the entry for a provider + cwd pair, or the
 *  stable empty entry when nothing has been fetched yet. */
export const selectProviderCommands =
  (provider: AgentChatProviderKind, cwd: string | null, threadId: string | null = null) =>
  (s: ProviderCommandsState): ProviderCommandsEntry => {
    return s.entries[commandsKey(provider, cwd, threadId)] ?? EMPTY_ENTRY;
  };
