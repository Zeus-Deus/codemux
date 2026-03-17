import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writable } from 'svelte/store';
import type { AppStateSnapshot } from './types';

export const appState = writable<AppStateSnapshot | null>(null);

export async function syncAppState() {
    const snapshot = await invoke<AppStateSnapshot>('get_app_state');
    appState.set(snapshot);
    return snapshot;
}

export async function initAppState() {
    try {
        await syncAppState();
    } catch (error) {
        console.error('Failed to fetch app state:', error);
    }

    await listen<AppStateSnapshot>('app-state-changed', (event) => {
        appState.set(event.payload);
    });
}
