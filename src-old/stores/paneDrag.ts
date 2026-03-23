import { writable } from 'svelte/store';

export interface PaneDragState {
    sourcePaneId: string;
    sourceTitle: string;
    dragging: boolean;
    targetPaneId: string | null;
    targetTitle: string | null;
}

export const paneDragState = writable<PaneDragState | null>(null);
