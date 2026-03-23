import { invoke } from '@tauri-apps/api/core';
import type { GitFileStatus, GitDiffStat, GitBranchInfo, WorktreeInfo } from './types';

export async function getGitStatus(path: string): Promise<GitFileStatus[]> {
    return invoke<GitFileStatus[]>('get_git_status', { path });
}

export async function getGitDiff(path: string, file: string, staged: boolean): Promise<string> {
    return invoke<string>('get_git_diff', { path, file, staged });
}

export async function getGitDiffStat(path: string): Promise<GitDiffStat> {
    return invoke<GitDiffStat>('get_git_diff_stat', { path });
}

export async function stageFiles(path: string, files: string[]): Promise<void> {
    return invoke('git_stage_files', { path, files });
}

export async function unstageFiles(path: string, files: string[]): Promise<void> {
    return invoke('git_unstage_files', { path, files });
}

export async function commitChanges(path: string, message: string): Promise<void> {
    return invoke('git_commit_changes', { path, message });
}

export async function pushChanges(path: string): Promise<void> {
    return invoke('git_push_changes', { path });
}

export async function getGitBranchInfo(path: string): Promise<GitBranchInfo> {
    return invoke<GitBranchInfo>('get_git_branch_info', { path });
}

export async function listBranches(path: string, remote: boolean): Promise<string[]> {
    return invoke<string[]>('list_branches', { path, remote });
}

export async function listWorktrees(path: string): Promise<WorktreeInfo[]> {
    return invoke<WorktreeInfo[]>('list_worktrees', { path });
}
