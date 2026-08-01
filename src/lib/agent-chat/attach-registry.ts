/**
 * Per-thread registry of in-flight `attach_agent_chat_output` calls.
 *
 * The backend routes a thread's live events to whatever channels are
 * attached AT EMIT TIME and drops them otherwise — there is no
 * replay-on-attach. So a row persisted between the hydrate's tail read
 * and the attach landing is delivered to nobody, and the next live event
 * advances the slice's cursor past it: the row is skipped permanently.
 *
 * The fix is read-after-attach ordering, which needs the hydrate (a
 * module-level helper) to observe a promise owned by the subscription hook
 * (a React effect). This registry is that seam, and nothing more: the hook
 * publishes its attach promise on mount and clears it on teardown; the
 * hydrate awaits it before reading.
 *
 * A thread with no registered attach resolves immediately — a pane that
 * never subscribed has no window to protect.
 */

/** Ceiling on how long a hydrate waits for its attach. A wedged IPC call
 *  must not pin the transcript behind a hold forever; past this we read
 *  anyway and accept the (pre-existing) drop window. */
export const ATTACH_WAIT_TIMEOUT_MS = 2_000;

const attaching = new Map<string, Promise<unknown>>();

/** Publish a thread's in-flight attach. A newer attach replaces the
 *  previous one (remount): the hydrate should wait on the current pane's
 *  subscription, not a dead one's. */
export function registerAgentChatAttach(
  threadId: string,
  attached: Promise<unknown>,
): void {
  attaching.set(threadId, attached);
}

/** Retract an attach on teardown — identity-checked so a stale pane's
 *  cleanup cannot unregister the newer pane's promise. */
export function clearAgentChatAttach(
  threadId: string,
  attached: Promise<unknown>,
): void {
  if (attaching.get(threadId) === attached) attaching.delete(threadId);
}

/** Resolve once this thread's channel is attached (or the wait times out,
 *  or nothing is attaching). Never rejects: a failed attach means no live
 *  stream at all, which the read handles on its own. */
export async function waitForAgentChatAttach(
  threadId: string,
  timeoutMs: number = ATTACH_WAIT_TIMEOUT_MS,
): Promise<void> {
  const attached = attaching.get(threadId);
  if (!attached) return;
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    await Promise.race([
      attached.then(
        () => undefined,
        () => undefined,
      ),
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, timeoutMs);
      }),
    ]);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
}
