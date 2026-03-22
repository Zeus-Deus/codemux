import { invoke } from '@tauri-apps/api/core';
import { writable } from 'svelte/store';
import type { PullRequestInfo, CheckInfo, GhStatus } from './types';

export const ghStatus = writable<GhStatus>({ status: 'NotInstalled' });

export async function refreshGhStatus(): Promise<GhStatus> {
    const result = await invoke<GhStatus>('check_gh_status');
    ghStatus.set(result);
    return result;
}

let ghStatusTimer: ReturnType<typeof setInterval> | null = null;

export async function initGhStatus() {
    try {
        await refreshGhStatus();
    } catch (e) {
        console.error('Failed to check gh status:', e);
    }
    ghStatusTimer = setInterval(() => void refreshGhStatus(), 60_000);
}

export async function checkGhAvailable(): Promise<boolean> {
    return invoke<boolean>('check_gh_available');
}

export async function checkGithubRepo(path: string): Promise<boolean> {
    return invoke<boolean>('check_github_repo', { path });
}

export async function getBranchPr(path: string): Promise<PullRequestInfo | null> {
    return invoke<PullRequestInfo | null>('get_branch_pull_request', { path });
}

export async function createPr(
    path: string,
    title: string,
    body: string,
    base: string | null,
    draft: boolean,
): Promise<PullRequestInfo> {
    return invoke<PullRequestInfo>('create_pull_request', { path, title, body, base, draft });
}

export async function listPrs(path: string, state: string): Promise<PullRequestInfo[]> {
    return invoke<PullRequestInfo[]>('list_pull_requests', { path, state });
}

export async function mergePr(path: string, prNumber: number, method: string): Promise<void> {
    return invoke('merge_pull_request', { path, prNumber, method });
}

export async function getPrChecks(path: string): Promise<CheckInfo[]> {
    return invoke<CheckInfo[]>('get_pull_request_checks', { path });
}
