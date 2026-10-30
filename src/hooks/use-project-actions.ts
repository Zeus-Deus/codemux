import { useCallback } from "react";
import {
  dbAddRecentProject,
  gitCloneRepo,
  createEmptyRepo,
  createEmptyWorkspace,
  activateWorkspace,
  listDirectory,
} from "@/tauri/commands";
import { pickFolder } from "@/lib/file-dialog";
import { toast } from "@/lib/toast";
import { useAppStore } from "@/stores/app-store";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { useDraftComposerFocusStore } from "@/stores/draft-composer-focus-store";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useUIStore, type AddProjectTab } from "@/stores/ui-store";
import { basename } from "@/lib/path";

/** True when the first-project legacy onboarding wizard should be
 *  suppressed. Two reasons fire it:
 *
 *  1. The user already saw and dismissed onboarding (`hasSeenOnboarding`
 *     is sticky on `useUIStore`). Closing every workspace or opening a
 *     second project must NOT re-arm the wizard — that's the re-trap
 *     fix from main.
 *  2. The user opted into the chat-agent + lazy-workspace-creation
 *     flow, in which case `useEnsureDraftWhenEmpty` auto-spawns an
 *     agent_chat pane on the freshly created empty project workspace
 *     instead, giving the user the chat UI directly.
 *
 *  Both reads are synchronous via `getState()` so action callbacks
 *  don't need to subscribe.
 */
function shouldSkipOnboarding(): boolean {
  if (useUIStore.getState().hasSeenOnboarding) return true;
  const flags = useFeatureFlags.getState();
  return flags.enableAgentChat && flags.enableLazyWorkspaceCreation;
}

interface OpenProjectResult {
  success: boolean;
  path?: string;
  name?: string;
}

/**
 * Folder-picker "open project" flow as a plain async function (no React
 * hooks) so it can be driven both from the `useProjectActions` hook and from
 * the global keyboard-shortcut dispatcher (Ctrl+O), which runs outside React.
 */
export async function openProjectFlow(): Promise<OpenProjectResult> {
  const folder = await pickFolder("Open project");
  if (!folder) return { success: false };
  return openProjectAtPath(folder);
}

/**
 * Give a folder that is now a project its workspace and switch to it — the
 * shared tail of every way a project gets added. `beforeActivate` runs once
 * the workspace exists but before it is shown, so a caller can decide what
 * appears in it first.
 */
async function addProjectWorkspace(
  folder: string,
  beforeActivate?: (workspaceId: string) => void,
): Promise<void> {
  // Non-git folders are accepted as-is — same as every other add path
  // (sidebar project menu, chat drafts, onboarding). The workspace runs in
  // plain-directory mode and the UI offers an explicit "Initialize Git"
  // action instead of gating project-add on git (we never `git init` a
  // user's folder without an explicit click).
  await dbAddRecentProject(folder, basename(folder));

  // Only show the onboarding wizard for truly first-time users — no existing
  // workspaces AND they have never seen/dismissed onboarding before. The
  // `hasSeenOnboarding` flag persists so that closing all workspaces or
  // opening a second project doesn't re-arm the wizard.
  const hasWorkspaces = (useAppStore.getState().appState?.workspaces.length ?? 0) > 0;
  const wsId = await createEmptyWorkspace(folder);
  beforeActivate?.(wsId);
  await activateWorkspace(wsId);
  if (!hasWorkspaces && !shouldSkipOnboarding()) {
    useUIStore.getState().setOnboardingProjectDir(folder);
  }
}

/**
 * Open a known folder as a project — the second half of `openProjectFlow`,
 * shared with the recent-projects list so reopening a project Codemux has
 * seen before skips the OS folder picker.
 */
export async function openProjectAtPath(
  folder: string,
): Promise<OpenProjectResult> {
  await addProjectWorkspace(folder);
  return { success: true, path: folder, name: basename(folder) };
}

/** Open a folder dropped from the file manager; a dropped file is refused.
 *  Resolves true once the project is open. */
export async function openDroppedFolder(path: string): Promise<boolean> {
  try {
    // Fails for anything that is not a readable directory.
    await listDirectory(path);
  } catch {
    toast.error(`${basename(path)} is not a folder. Drop a project folder to open it.`);
    return false;
  }
  try {
    await openProjectAtPath(path);
    return true;
  } catch (err) {
    toast.error(`Couldn't open ${basename(path)}: ${err instanceof Error ? err.message : String(err)}`);
    return false;
  }
}

/** Clone `url` into the absolute `targetDir` and open it as a project. */
export async function cloneProjectFlow(url: string, targetDir: string) {
  const clonedPath = await gitCloneRepo(url, targetDir);
  await addProjectWorkspace(clonedPath);
  return { path: clonedPath, name: basename(clonedPath) };
}

/**
 * Create `name` under the absolute `location` as a fresh git repository and
 * open it. With `firstMessage`, the workspace opens on a chat whose composer
 * already holds it — focused, not sent — so the user can review the prompt
 * before an agent starts building.
 */
export async function createProjectFlow(
  location: string,
  name: string,
  firstMessage?: string,
) {
  const projectPath = await createEmptyRepo(location, name);
  await addProjectWorkspace(projectPath, firstMessage
    ? (workspaceId) => prefillFirstChat(workspaceId, firstMessage)
    : undefined);
  return { path: projectPath, name: basename(projectPath) };
}

/**
 * Put a chat draft aimed at `workspaceId` on screen with `text` in its
 * composer. Runs before the workspace is activated: an active draft is what
 * stops the empty-workspace hook from launching a second, blank chat there.
 */
function prefillFirstChat(workspaceId: string, text: string): void {
  const drafts = useChatDraftStore.getState();
  const draft = drafts.getOrCreateHomeDraft();
  drafts.updateDraftTarget(draft.draftId, { kind: "existing_workspace", workspaceId });
  // A brand-new repository has nothing to branch away from; the agent
  // builds in the project's own checkout.
  drafts.updateDraftConfig(draft.draftId, { checkoutMode: "current" });
  drafts.updateDraftInput(draft.draftId, text);
  drafts.setActiveDraft(draft.draftId);
  useDraftComposerFocusStore.getState().requestFocus(draft.draftId);
}

/**
 * Point the home draft on screen at a project the user just added, as
 * picking it in the composer's location control does. Adding a project is an
 * explicit choice, so this overrides a home-only lock (the first-run draft
 * carries one); drafts already aimed somewhere are left alone.
 */
export function pointHomeDraftAt(projectPath: string): void {
  const drafts = useChatDraftStore.getState();
  const draft = drafts.activeDraftId ? drafts.draftsById[drafts.activeDraftId] : null;
  if (!draft || draft.target.kind !== "home") return;
  drafts.updateDraftTarget(draft.draftId, { kind: "project", projectPath });
}

export function useProjectActions() {
  const setAddProjectSheet = useUIStore((s) => s.setAddProjectSheet);

  const openProject = useCallback(() => openProjectFlow(), []);

  const openAddProject = useCallback(
    (tab: AddProjectTab) => setAddProjectSheet(tab),
    [setAddProjectSheet],
  );

  return { openProject, openAddProject, cloneProject: cloneProjectFlow };
}
