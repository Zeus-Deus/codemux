import { writable } from 'svelte/store';

export type OverlayKind = 'none' | 'command-palette' | 'file-search' | 'keyword-search';

export const activeOverlay = writable<OverlayKind>('none');

export function openOverlay(kind: OverlayKind) {
    activeOverlay.set(kind);
}

export function closeOverlay() {
    activeOverlay.set('none');
}

export function toggleOverlay(kind: OverlayKind) {
    activeOverlay.update(current => current === kind ? 'none' : kind);
}
