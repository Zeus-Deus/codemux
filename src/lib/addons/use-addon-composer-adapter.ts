import { useLayoutEffect, useMemo, useRef, useState } from "react";
import { randomUUID } from "@/lib/uuid";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useAddonsStore } from "@/stores/addons-store";
import { addonInvoke } from "./bridge";
import { appendAddonText, registerAddonComposer } from "./composer-registry";
import { addonError, addonMessage } from "./types";
import { activeAddonWorkspace } from "./platform";
export interface AddonComposerBinding {
  /** The registered composer ID; empty until the broker accepted it. */
  id: string;
  registered: boolean;
  /** Why add-on actions cannot use this draft; null while registered or
   *  while the registration is still in flight. */
  unavailable: string | null;
}
/**
 * Binds one composer instance's draft to the add-on broker.
 *
 * Every (workspace, thread) target gets its own registration, and a
 * registration can only ever write to the draft it was created for. It is
 * retired in a layout-effect cleanup, which runs in the same commit that
 * points `latest` at the next thread's draft, so a late append from the old
 * target can never reach the new thread.
 */
export function useAddonComposerAdapter(
  workspaceId: string | null,
  threadId: string | null,
  draft: string,
  onDraftChange: (text: string) => void,
): AddonComposerBinding {
  const enabled = useFeatureFlags((s) => s.enableAgentChat);
  // Identity of the current target; a new object whenever it changes.
  const target = useMemo(
    () => ({ workspaceId, threadId, enabled }),
    [workspaceId, threadId, enabled],
  );
  const [status, setStatus] = useState<
    (AddonComposerBinding & { target: typeof target }) | null
  >(null);
  const latest = useRef({ target, draft, onDraftChange });
  const revision = useRef(0);
  useLayoutEffect(() => {
    latest.current = { target, draft, onDraftChange };
    revision.current++;
  }, [target, draft, onDraftChange]);
  useLayoutEffect(() => {
    const { workspaceId, enabled } = target;
    if (!workspaceId || !enabled) return;
    // A fresh ID per registration, so a cleanup that reaches the broker late
    // can only close its own registration, never a newer one.
    const id = randomUUID();
    let disposed = false;
    let unregister = () => {};
    void addonInvoke("addon_composer_register", { composerId: id, workspaceId })
      .then(() => {
        if (disposed) {
          void addonInvoke("addon_composer_closed", { composerId: id }).catch(
            () => {},
          );
          return;
        }
        unregister = registerAddonComposer(id, {
          workspaceId,
          append(text) {
            if (
              disposed ||
              latest.current.target !== target ||
              activeAddonWorkspace() !== workspaceId
            )
              throw addonError("CONTEXT_STALE", "The draft target changed");
            const next = appendAddonText(latest.current.draft, text);
            // Update immediately, including multiple appends before React commits.
            latest.current.draft = next;
            latest.current.onDraftChange(next);
            return ++revision.current;
          },
        });
        setStatus({ target, id, registered: true, unavailable: null });
      })
      .catch((cause) => {
        if (!disposed)
          setStatus({
            target,
            id: "",
            registered: false,
            unavailable: addonMessage(cause),
          });
      });
    return () => {
      disposed = true;
      unregister();
      if (useAddonsStore.getState().accessory?.composerId === id)
        useAddonsStore.setState({ accessory: null });
      void addonInvoke("addon_composer_closed", { composerId: id }).catch(
        () => {},
      );
    };
  }, [target]);
  if (!enabled)
    return { id: "", registered: false, unavailable: "Chat GUI is disabled" };
  if (!workspaceId)
    return {
      id: "",
      registered: false,
      unavailable: "Open a local workspace to use add-on actions",
    };
  if (status?.target !== target)
    return { id: "", registered: false, unavailable: null };
  const { id, registered, unavailable } = status;
  return { id, registered, unavailable };
}
