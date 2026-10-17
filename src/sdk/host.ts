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
import { bindOwner, currentOwner, ownedDisposer, type Owner } from "@/contrib/lifetime";
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

function settingValue(key: string): string {
  return useSettingsStore.getState().settings[key] ?? SETTINGS_DEFAULTS[key] ?? "";
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

/**
 * The host API, bound to the plugin that owns it. `ctx.host` is bound to its
 * plugin incarnation; the global `host` export resolves the calling plugin
 * when that is certain (inside `register()` and inside callbacks the host
 * invokes for a plugin). Subscriptions are removed when their plugin turns
 * off; each subscription callback is isolated, so a throw is logged against
 * the plugin and never unwinds the core code that notified it.
 */
export function createHost(bound: Owner | null) {
  const ownerNow = () => bound ?? currentOwner();

  const guarded = <A extends unknown[]>(owner: Owner | null, fn: (...args: A) => void) => {
    const run = bindOwner(owner, fn);
    return (...args: A) => {
      try {
        run(...args);
      } catch (error) {
        console.error(`[plugins] ${owner?.pluginId ?? "plugin"}: subscription callback threw`, error);
      }
    };
  };

  /** Subscribe `fn` through `attach`, owned and isolated. */
  const subscription = <A extends unknown[]>(
    fn: (...args: A) => void,
    attach: (run: (...args: A) => void) => () => void,
  ): (() => void) => {
    const owner = ownerNow();
    if (owner?.ended) return () => {};
    return ownedDisposer(owner, attach(guarded(owner, fn)));
  };

  const api = {
    theme: {
      get: (): ThemeDefinition => getActiveTheme(),
      subscribe: (fn: (theme: ThemeDefinition) => void) =>
        subscription(fn, (run) => subscribeActiveTheme(() => run(getActiveTheme()))),
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
      subscribe: (fn: (ws: WorkspaceSnapshot | null) => void) =>
        subscription(fn, (run) => {
          let last = currentWorkspace();
          return useAppStore.subscribe(() => {
            const next = currentWorkspace();
            if (next !== last) {
              last = next;
              run(next);
            }
          });
        }),
    },

    settings: {
      get: settingValue,
      subscribe: (key: string, fn: (value: string) => void) =>
        subscription(fn, (run) => {
          let last = settingValue(key);
          return useSettingsStore.subscribe(() => {
            const next = settingValue(key);
            if (next !== last) {
              last = next;
              run(next);
            }
          });
        }),
    },

    composer,

    chat: {
      /**
       * Every thread's provider events, including background threads with no
       * pane open, plus `usage_recorded` (with `cacheWriteTtl`) and
       * `plan_usage_updated`. The first subscriber turns the native tap on;
       * the last one leaving turns it off.
       */
      onEvent: (fn: (event: PluginChatEvent) => void) => subscription(fn, subscribeChatEvents),
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
      return subscription(fn, (run) => {
        let unlisten: (() => void) | null = null;
        let cancelled = false;
        void tauriListen<T>(event, (e) => run(e.payload)).then((u) => {
          if (cancelled) u();
          else unlisten = u;
        });
        return () => {
          cancelled = true;
          unlisten?.();
        };
      });
    },
  };
  return api;
}

export type Host = ReturnType<typeof createHost>;

/** The host API. Prefer `ctx.host` for subscriptions made after an `await`. */
export const host: Host = createHost(null);

