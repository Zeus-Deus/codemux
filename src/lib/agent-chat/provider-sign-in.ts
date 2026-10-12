import { toast } from "@/lib/toast";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import {
  activateWorkspace,
  getOrCreateHomeWorkspace,
  openProviderLoginTerminal,
} from "@/tauri/commands";
import type { AgentChatProviderKind } from "@/tauri/types";

/**
 * Start `provider`'s sign-in in a Codemux terminal tab and bring it into
 * view. `workspaceId` is where the tab opens; without one the home
 * workspace hosts it, since a first chat may have no workspace yet.
 *
 * The draft (if one is showing) stays in the sidebar with its text, so the
 * user can return to it once signed in. When no terminal can be opened
 * (a paired web client, say), the command is copied instead so the user
 * still leaves with something that works. `loginCommand` comes from the
 * health report and is only that fallback; the backend owns the command
 * it runs.
 */
export async function signInToProvider(
  provider: AgentChatProviderKind,
  loginCommand: string | null | undefined,
  workspaceId: string | null,
): Promise<void> {
  try {
    const target = workspaceId ?? (await getOrCreateHomeWorkspace());
    await openProviderLoginTerminal(target, provider);
    useChatDraftStore.getState().setActiveDraft(null);
    await activateWorkspace(target);
    return;
  } catch (error) {
    console.warn(`[provider-sign-in] terminal for ${provider} failed:`, error);
  }
  if (!loginCommand) {
    toast.error("Couldn't open a terminal to sign in");
    return;
  }
  try {
    await navigator.clipboard.writeText(loginCommand);
    toast.success(`Copied — run \`${loginCommand}\` in a terminal`);
  } catch {
    toast.error(`Run \`${loginCommand}\` in a terminal to sign in`);
  }
}
