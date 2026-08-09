import { useMemo, useSyncExternalStore } from "react";
import { createCodePlugin, type CodeHighlighterPlugin } from "@streamdown/code";

import { buildChatCodeThemes } from "@/lib/shiki-chat-theme";
import type { ThemeColors } from "@/tauri/types";
import {
  getSyntaxThemeSnapshot,
  subscribeSyntaxTheme,
} from "./use-theme-colors";

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

const subscribers = new Set<() => void>();
let unsubscribeSyntax: (() => void) | null = null;

function subscribe(onStoreChange: () => void) {
  subscribers.add(onStoreChange);
  if (!unsubscribeSyntax) {
    unsubscribeSyntax = subscribeSyntaxTheme(() => {
      for (const notify of subscribers) notify();
    });
  }
  return () => {
    subscribers.delete(onStoreChange);
  };
}

function getSnapshot(): ThemeColors {
  return getSyntaxThemeSnapshot();
}

/** Test-only: drops the listener and resets the palette to the fallback. */
export function resetChatCodePluginStore() {
  unsubscribeSyntax?.();
  unsubscribeSyntax = null;
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
