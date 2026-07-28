import { useMemo, useSyncExternalStore } from "react";
import { createCodePlugin, type CodeHighlighterPlugin } from "@streamdown/code";

import { buildChatCodeThemes } from "@/lib/shiki-chat-theme";
import { getCurrentTheme } from "@/tauri/commands";
import { onThemeChanged } from "@/tauri/events";
import type { ThemeColors } from "@/tauri/types";
import { fallbackTheme } from "./use-theme-colors";

/**
 * Supplies the Shiki code-highlighting plugin for chat markdown, colored by
 * the active terminal palette.
 *
 * Deliberately a module-level store rather than `useThemeColors()`: a
 * transcript mounts one `ChatMarkdown` per assistant message, and a per-hook
 * implementation would fire one `get_current_theme` IPC call and register one
 * `theme-changed` listener *per message*. Here every consumer shares a single
 * fetch, a single listener, and a single plugin instance.
 *
 * The plugin is memoized on palette identity so a streaming transcript hands
 * every `ChatMarkdown` the same plugin object across renders. (`@streamdown/code`
 * keeps its highlighter and token caches in module-level maps, so those survive
 * a rebuild — what the memo avoids is the churn of a new plugin identity
 * propagating through the markdown tree on every keystroke.)
 */

let currentTheme: ThemeColors = fallbackTheme;
const subscribers = new Set<() => void>();
let unlisten: (() => void) | null = null;
let started = false;

function setTheme(next: ThemeColors) {
  currentTheme = next;
  for (const notify of subscribers) notify();
}

function start() {
  if (started) return;
  started = true;
  // Both are best-effort: outside Tauri (or before the backend is ready) the
  // fallback palette renders fine, it just isn't the user's terminal theme.
  getCurrentTheme()
    .then(setTheme)
    .catch(() => {});
  onThemeChanged(setTheme)
    .then((fn) => {
      unlisten = fn;
    })
    .catch(() => {});
}

function subscribe(onStoreChange: () => void) {
  subscribers.add(onStoreChange);
  start();
  return () => {
    subscribers.delete(onStoreChange);
  };
}

function getSnapshot(): ThemeColors {
  return currentTheme;
}

/** Test-only: drops the listener and resets the palette to the fallback. */
export function resetChatCodePluginStore() {
  unlisten?.();
  unlisten = null;
  started = false;
  currentTheme = fallbackTheme;
  cachedPlugin = null;
  subscribers.clear();
}

let cachedPlugin: { key: string; plugin: CodeHighlighterPlugin } | null = null;

export function useChatCodePlugin(): CodeHighlighterPlugin {
  const theme = useSyncExternalStore(subscribe, getSnapshot, getSnapshot);

  return useMemo(() => {
    const themes = buildChatCodeThemes(theme);
    // Theme names embed a hash of the palette, so this key changes exactly
    // when the colors change.
    const key = String(themes[0].name);
    if (cachedPlugin?.key === key) return cachedPlugin.plugin;
    const plugin = createCodePlugin({ themes });
    cachedPlugin = { key, plugin };
    return plugin;
  }, [theme]);
}
