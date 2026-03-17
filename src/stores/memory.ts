import { invoke } from '@tauri-apps/api/core';
import { writable } from 'svelte/store';
import type {
    HandoffPacket,
    MemoryEntryKind,
    MemorySource,
    ProjectMemorySnapshot,
    ProjectMemoryUpdate,
} from './types';

export const projectMemory = writable<ProjectMemorySnapshot | null>(null);

export async function syncProjectMemory() {
    const snapshot = await invoke<ProjectMemorySnapshot>('get_project_memory_snapshot');
    projectMemory.set(snapshot);
    return snapshot;
}

export async function initProjectMemory() {
    try {
        await syncProjectMemory();
    } catch (error) {
        console.error('Failed to fetch project memory:', error);
    }
}

export async function updateProjectMemory(update: ProjectMemoryUpdate) {
    const snapshot = await invoke<ProjectMemorySnapshot>('update_project_memory_snapshot', { update });
    projectMemory.set(snapshot);
    return snapshot;
}

export async function addProjectMemoryEntry(
    kind: MemoryEntryKind,
    content: string,
    options?: { source?: MemorySource; tags?: string[]; toolName?: string; sessionLabel?: string }
) {
    const snapshot = await invoke<ProjectMemorySnapshot>('add_project_memory_entry', {
        kind,
        source: options?.source ?? 'human',
        content,
        tags: options?.tags ?? [],
        toolName: options?.toolName,
        sessionLabel: options?.sessionLabel,
    });
    projectMemory.set(snapshot);
    return snapshot;
}

export async function generateProjectHandoff() {
    return invoke<HandoffPacket>('generate_project_handoff');
}
