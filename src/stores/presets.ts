import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writable } from 'svelte/store';
import type { PresetStoreSnapshot, LaunchMode } from './types';

export const presetStore = writable<PresetStoreSnapshot | null>(null);

export async function syncPresets() {
    const snapshot = await invoke<PresetStoreSnapshot>('get_presets');
    presetStore.set(snapshot);
    return snapshot;
}

export async function initPresets() {
    try {
        await syncPresets();
    } catch (error) {
        console.error('Failed to fetch presets:', error);
    }

    await listen<PresetStoreSnapshot>('presets-changed', (event) => {
        presetStore.set(event.payload);
    });
}

export async function createPreset(options: {
    name: string;
    description?: string;
    commands: string[];
    working_directory?: string;
    launch_mode: LaunchMode;
    pinned: boolean;
}) {
    const id = await invoke<string>('create_preset', {
        name: options.name,
        description: options.description ?? null,
        commands: options.commands,
        workingDirectory: options.working_directory ?? null,
        launchMode: options.launch_mode,
        pinned: options.pinned,
    });
    await syncPresets();
    return id;
}

export async function updatePreset(id: string, updates: {
    name?: string;
    description?: string;
    commands?: string[];
    working_directory?: string;
    launch_mode?: LaunchMode;
    pinned?: boolean;
    icon?: string;
}) {
    await invoke('update_preset', {
        id,
        name: updates.name ?? null,
        description: updates.description ?? null,
        commands: updates.commands ?? null,
        workingDirectory: updates.working_directory ?? null,
        launchMode: updates.launch_mode ?? null,
        pinned: updates.pinned ?? null,
        icon: updates.icon ?? null,
    });
    await syncPresets();
}

export async function deletePreset(id: string) {
    await invoke('delete_preset', { id });
    await syncPresets();
}

export async function setPresetPinned(id: string, pinned: boolean) {
    await invoke('set_preset_pinned', { id, pinned });
    await syncPresets();
}

export async function applyPreset(
    workspaceId: string,
    presetId: string,
    overrideMode?: 'new_tab' | 'split_pane' | 'current_terminal' | 'existing_panes',
) {
    await invoke('apply_preset', {
        workspaceId,
        presetId,
        overrideMode: overrideMode ?? null,
    });
}

export async function setPresetBarVisible(visible: boolean) {
    await invoke('set_preset_bar_visible', { visible });
    await syncPresets();
}
