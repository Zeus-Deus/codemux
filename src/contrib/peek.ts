import type { ComponentType, ReactNode } from "react";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { useBrowserPeekStore, type PeekSize, type PeekStatusKind, type PluginPeekSnapshot } from "@/stores/browser-peek-store";
import { PEEK_AREA } from "@/sdk/areas";
import { registry } from "./registry";
import type { Owner } from "./lifetime";

export type { PeekSize, PeekStatusKind, PluginPeekSnapshot };
export interface PeekData {
  icon?: ComponentType<{ className?: string }>;
  /** Full id of a right-panel contribution owned by the same plugin. */
  dockPaneId?: string;
}
export interface PeekRenderProps {
  workspaceId: string;
  size: PeekSize;
  close: () => void;
  onStatusChange: (status: PeekStatusKind, label?: string) => void;
}
export type PeekBody = (props: PeekRenderProps) => ReactNode;

// Contributions own their views: unloading must dismiss even while Settings
// has unmounted the overlay. A same-id replacement remains available.
registry.subscribeArea(PEEK_AREA, () => {
  const state = useBrowserPeekStore.getState();
  if (state.pluginPeek && !registry.getArea(PEEK_AREA).some((c) => c.id === state.pluginPeek?.id && c.render)) {
    state.closePlugin(state.pluginPeek.id);
  }
});

export function createPeekHost(
  ownerNow: () => Owner | null,
  subscription: <A extends unknown[]>(fn: (...args: A) => void, attach: (run: (...args: A) => void) => () => void) => () => void,
) {
  const isOpen = (id: string) => useBrowserPeekStore.getState().pluginPeek?.id === id;
  const open = (id: string, options?: { workspaceId?: string }): boolean => {
    if (ownerNow()?.ended) return false;
    const active = selectActiveWorkspaceId(useAppStore.getState());
    const workspaceId = options?.workspaceId ?? active;
    if (!workspaceId || workspaceId !== active || !registry.getArea(PEEK_AREA).some((c) => c.id === id && c.render)) return false;
    const current = useBrowserPeekStore.getState().pluginPeek;
    if (current?.id !== id || current.workspaceId !== workspaceId) {
      useBrowserPeekStore.getState().openPlugin(id, workspaceId);
    }
    return true;
  };
  return {
    open,
    close: (id: string) => {
      if (!ownerNow()?.ended) useBrowserPeekStore.getState().closePlugin(id);
    },
    toggle: (id: string, options?: { workspaceId?: string }): boolean => {
      if (ownerNow()?.ended) return false;
      if (isOpen(id)) { useBrowserPeekStore.getState().closePlugin(id); return true; }
      return open(id, options);
    },
    isOpen,
    /** Plugin peek state; opening the browser reports null. */
    subscribe: (fn: (peek: PluginPeekSnapshot | null) => void) => subscription(fn, (run) =>
      useBrowserPeekStore.subscribe((state, before) => {
        const next = state.pluginPeek;
        const prev = before.pluginPeek;
        if (next?.id !== prev?.id || next?.workspaceId !== prev?.workspaceId) {
          run(next ? { id: next.id, workspaceId: next.workspaceId } : null);
        }
      }),
    ),
  };
}
