/**
 * Attach-time image staging (Bug 1 fix).
 *
 * Pasted screenshots used to marshal across Tauri IPC as a JSON
 * `number[]` on the send path (~4 bytes of JSON per image byte), so a
 * handful of screenshots turned the first send into a multi-minute
 * stall. Instead we now write each image's bytes to a backend staging
 * file the moment it's attached (`agent_chat_stage_image`, raw-body
 * invoke) and let the turn reference the staged file by path. The upload
 * cost overlaps the user still typing rather than blocking Enter.
 *
 * This module owns the in-flight staging promises (keyed by attachment
 * id) so a send can `await` only the stragglers that haven't landed yet,
 * and patches the chip with `stagedImage` (or a `metadata.error` on
 * failure) via the agent-chat store.
 */

import { discardStagedChatImage, stageChatImage } from "@/tauri/commands";
import { useAgentChatStore } from "@/stores/agent-chat-store";

/** In-flight staging promises, keyed by attachment id. Cleared when the
 *  staging settles (success or failure). Module-level so both the pane
 *  and the draft surface share one registry regardless of which mounted. */
const stagingPromises = new Map<string, Promise<void>>();

function errMessage(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  return "Unknown error";
}

/**
 * Kick off staging `bytes` to a backend staging file for the given chip.
 * Patches the chip with `stagedImage` on success, or with a
 * `metadata.error` on failure (which the chip surfaces and which blocks
 * the chip from being sent — we never silently fall back to the deleted
 * `number[]` path). Registers the in-flight promise so `awaitImageStaging`
 * can wait on stragglers at send time. Fire-and-forget: callers don't
 * await this.
 */
export function beginImageStaging(
  threadId: string,
  attachmentId: string,
  bytes: Uint8Array,
  mime: string,
): void {
  const promise = (async () => {
    try {
      const staged = await stageChatImage(bytes, mime);
      useAgentChatStore
        .getState()
        .updateStagedAttachment(threadId, attachmentId, {
          stagedImage: { path: staged.path, mediaType: staged.media_type },
        });
    } catch (err) {
      const existing = useAgentChatStore
        .getState()
        .threads[threadId]?.stagedAttachments.find((a) => a.id === attachmentId);
      useAgentChatStore
        .getState()
        .updateStagedAttachment(threadId, attachmentId, {
          metadata: {
            ...(existing?.metadata ?? { label: "image" }),
            isLoading: false,
            error: `Failed to stage image: ${errMessage(err)}`,
          },
        });
    } finally {
      stagingPromises.delete(attachmentId);
    }
  })();
  stagingPromises.set(attachmentId, promise);
}

/**
 * Await any still-in-flight staging for the given attachment ids. Normally
 * a no-op at send time (staging started at attach time and has long since
 * landed), but covers the paste-then-immediately-Enter race.
 */
export async function awaitImageStaging(
  attachmentIds: string[],
): Promise<void> {
  const pending = attachmentIds
    .map((id) => stagingPromises.get(id))
    .filter((p): p is Promise<void> => p !== undefined);
  if (pending.length > 0) await Promise.all(pending);
}

/** Fire-and-forget best-effort delete of a staged file (chip removal /
 *  draft discard). Errors are swallowed — a leaked staging file is
 *  harmless and reaped by the backend's startup sweep
 *  (`sweep_stale_staged_images`). */
export function discardStagedImage(path: string): void {
  void discardStagedChatImage(path).catch(() => {
    /* best-effort */
  });
}

/** Test-only: clear the in-flight registry between cases. */
export function __resetImageStagingForTests(): void {
  stagingPromises.clear();
}
