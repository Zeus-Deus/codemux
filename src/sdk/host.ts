/**
 * `host`: the app's API for plugins. Plugins have full authority; the members
 * here are the supported surface. `host.invoke` / `host.listen` are the raw
 * Tauri door and are UNSTABLE: commands may change between releases.
 */
import type { ComponentType, ReactNode } from "react";
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { toast } from "@/lib/toast";
import {
  getActiveTheme,
  subscribeActiveTheme,
  THEME_ROLE_VARIABLES,
  type ThemeDefinition,
  type ThemeRole,
} from "@/lib/themes";
import { selectReduceMotion, useSettingsStore, SETTINGS_DEFAULTS } from "@/stores/settings-store";
import { useUIStore } from "@/stores/ui-store";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useAgentChatStore } from "@/stores/agent-chat-store";
import {
  agentChatGetSession,
  agentChatSendTurn,
  type AgentChatSessionRecord,
} from "@/tauri/commands";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { bindOwner, currentOwner, trackAmbient } from "@/contrib/lifetime";
import { focusedComposer } from "@/contrib/composer-registry";
import { subscribeChatEvents, type PluginChatEvent } from "@/contrib/chat-events";

// ── Area payload types ────────────────────────────────────────────────

type Icon = ComponentType<{ className?: string }>;

export interface PaletteCommandData {
  label: string;
  icon?: Icon;
  keywords?: string;
  /** Hidden when there is no active workspace. */
  requiresWorkspace?: boolean;
  run: (ctx: { workspaceId: string | null }) => void;
}

export interface KeybindData {
  label: string;
  /** Settings → Shortcuts group; defaults to "general". */
  category?: "general" | "search" | "workspaces" | "tabs" | "panes" | "chat";
  /** e.g. "Ctrl+Alt+H". Users can rebind it. */
  defaultKeys: string;
  description?: string;
  /** Return false to let the key fall through. */
  run: (event?: KeyboardEvent) => boolean | void;
}

export interface ComposerActionData {
  label: string;
  description?: string;
  icon?: Icon;
  run: (ctx: { workspaceId: string | null; threadId: string | null; composer: ComposerApi }) => void;
}

export interface ToolCallView {
  tool_name: string;
  tool_use_id: string;
  input: unknown;
  status: "running" | "done" | "error" | "unconfirmed";
  result_content: unknown | null;
}

export interface ToolRendererData {
  /** Tool name to match, e.g. "Bash" or "mcp__my_server__thing". */
  tool: string;
  render: (props: { call: ToolCallView }) => ReactNode;
}

export interface RightPaneData {
  icon?: Icon;
}

export interface LandingBackdropProps {
  /** Register a handler for left clicks on empty landing space. Returns an unsubscribe. */
  onPulse: (handler: (x: number, y: number) => void) => () => void;
  /** The headline block's rect in viewport pixels (for dimming behind text), or null. */
  calmRect: () => DOMRect | null;
}

// ── host ──────────────────────────────────────────────────────────────

export type NotifyKind = "info" | "success" | "warning" | "error";

export interface ComposerApi {
  insertText(text: string): boolean;
  getDraft(): string | null;
  focus(): boolean;
}

const composer: ComposerApi = {
  insertText(text) {
    const target = focusedComposer();
    if (!target) return false;
    target.insertText(text);
    return true;
  },
  getDraft() {
    return focusedComposer()?.getDraft() ?? null;
  },
  focus() {
    const target = focusedComposer();
    target?.focus();
    return !!target;
  },
};

function cssVar(role: ThemeRole): string {
  return THEME_ROLE_VARIABLES[role];
}

/**
 * A plugin's subscription callback, isolated and owned: a throw is reported
 * against the plugin and never unwinds the core code that notified it (the
 * theme's layout effect, a store update, the other listeners), and host
 * calls made inside run as that plugin.
 */
function guarded<A extends unknown[]>(fn: (...args: A) => void): (...args: A) => void {
  const owner = currentOwner();
  const run = bindOwner(owner, fn);
  return (...args: A) => {
    try {
      run(...args);
    } catch (error) {
      console.error(`[plugins] ${owner?.pluginId ?? "plugin"}: subscription callback threw`, error);
    }
  };
}

/** Subscribe helper that also detaches when the calling plugin is turned off. */
function tracked(unsubscribe: () => void): () => void {
  let done = false;
  const once = () => {
    if (done) return;
    done = true;
    unsubscribe();
  };
  return trackAmbient(once);
}

function currentWorkspace(): WorkspaceSnapshot | null {
  const state = useAppStore.getState();
  const id = selectActiveWorkspaceId(state);
  return state.appState?.workspaces.find((w) => w.workspace_id === id) ?? null;
}

export interface ThreadSnapshot {
  threadId: string;
  /** Live reducer state when the thread is mounted, else null. */
  live: unknown | null;
  /** Persisted session record (unmounted threads too), when it exists. */
  session: AgentChatSessionRecord | null;
}

export const host = {
  theme: {
    get: (): ThemeDefinition => getActiveTheme(),
    subscribe: (fn: (theme: ThemeDefinition) => void) => {
      const run = guarded(fn);
      return tracked(subscribeActiveTheme(() => run(getActiveTheme())));
    },
    /** CSS variable name for a theme role, e.g. `--cm-theme-background`. */
    cssVar,
  },

  reducedMotion: (): boolean => selectReduceMotion(useSettingsStore.getState()),

  notify({ kind = "info", message, description }: { kind?: NotifyKind; message: string; description?: string }) {
    toast[kind](message, description ? { description } : undefined);
  },

  /** Open a plugin page by its full contribution id (`<pluginId>:<localId>`). */
  openPage(id: string) {
    useUIStore.getState().setShowPluginPage(id);
  },
  openSettings(section?: string) {
    useUIStore.getState().setShowSettings(true, section ?? null);
  },
  /** Open a right-panel pane by its full contribution id, in the active workspace. */
  openRightPane(id: string): boolean {
    const ws = currentWorkspace();
    if (!ws) return false;
    useUIStore.getState().setRightPanelTab(ws.workspace_id, `plugin:${id}` as never);
    return true;
  },

  workspace: {
    current: currentWorkspace,
    subscribe: (fn: (ws: WorkspaceSnapshot | null) => void) => {
      const run = guarded(fn);
      let last = currentWorkspace();
      return tracked(
        useAppStore.subscribe(() => {
          const next = currentWorkspace();
          if (next !== last) {
            last = next;
            run(next);
          }
        }),
      );
    },
  },

  settings: {
    get: (key: string): string =>
      useSettingsStore.getState().settings[key] ?? SETTINGS_DEFAULTS[key] ?? "",
    subscribe: (key: string, fn: (value: string) => void) => {
      const run = guarded(fn);
      let last = host.settings.get(key);
      return tracked(
        useSettingsStore.subscribe(() => {
          const next = host.settings.get(key);
          if (next !== last) {
            last = next;
            run(next);
          }
        }),
      );
    },
  },

  composer,

  chat: {
    /**
     * Every thread's provider events, including background threads with no
     * pane open, plus `usage_recorded` (with `cacheWriteTtl`) and
     * `plan_usage_updated`. The first subscriber turns the native tap on;
     * the last one leaving turns it off.
     */
    onEvent: (fn: (event: PluginChatEvent) => void) => tracked(subscribeChatEvents(guarded(fn))),
    async getThread(threadId: string): Promise<ThreadSnapshot> {
      const live = useAgentChatStore.getState().threads[threadId] ?? null;
      let session: AgentChatSessionRecord | null = null;
      try {
        session = await agentChatGetSession(threadId);
      } catch {
        session = null;
      }
      return { threadId, live, session };
    },
    /** Threads with live state in this window. */
    listThreads: (): string[] => Object.keys(useAgentChatStore.getState().threads),
    /** Same path as pressing send. Queues behind an active turn. */
    async sendTurn(threadId: string, text: string) {
      const session = await agentChatGetSession(threadId);
      if (!session) throw new Error(`Unknown thread ${threadId}`);
      return agentChatSendTurn(session.provider, {
        thread_id: threadId,
        text,
        model_override: null,
      });
    },
  },

  /** UNSTABLE raw IPC: any app command. May change between releases. */
  invoke: <T = unknown>(cmd: string, args?: Record<string, unknown>) => tauriInvoke<T>(cmd, args),
  /** UNSTABLE raw app event subscription. Removed when the plugin turns off. */
  listen<T = unknown>(event: string, fn: (payload: T) => void): () => void {
    const run = guarded(fn);
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    void tauriListen<T>(event, (e) => run(e.payload)).then((u) => {
      if (cancelled) u();
      else unlisten = u;
    });
    return tracked(() => {
      cancelled = true;
      unlisten?.();
    });
  },
};

