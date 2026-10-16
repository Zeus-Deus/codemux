/**
 * The plugin chat tap: every thread's provider events, for `host.chat.onEvent`.
 * The native side emits `plugin_agent_event` only while the tap flag is on;
 * the first subscriber turns it on and the last one leaving turns it off, so
 * with no listener the app does no extra work at all.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { ProviderRuntimeEvent } from "@/tauri/events";

export interface PluginChatEvent {
  threadId: string;
  event: ProviderRuntimeEvent;
}

type Listener = (event: PluginChatEvent) => void;

const listeners = new Set<Listener>();
let unlisten: Promise<UnlistenFn> | null = null;

function dispatch(payload: { thread_id: string; event: ProviderRuntimeEvent }) {
  const event = { threadId: payload.thread_id, event: payload.event };
  for (const listener of [...listeners]) {
    try {
      listener(event);
    } catch (error) {
      console.error("[plugins] chat event listener threw", error);
    }
  }
}

function setTap(on: boolean) {
  void invoke("plugins_set_event_tap", { on }).catch((error) =>
    console.warn("[plugins] could not switch the chat event tap", error),
  );
}

export function subscribeChatEvents(listener: Listener): () => void {
  listeners.add(listener);
  if (listeners.size === 1) {
    unlisten = listen<{ thread_id: string; event: ProviderRuntimeEvent }>("plugin_agent_event", (e) =>
      dispatch(e.payload),
    );
    setTap(true);
  }
  return () => {
    if (!listeners.delete(listener)) return;
    if (listeners.size === 0) {
      setTap(false);
      const pending = unlisten;
      unlisten = null;
      void pending?.then((u) => u());
    }
  };
}

export function chatListenerCount(): number {
  return listeners.size;
}
