import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { randomUUID } from "@/lib/uuid";
import { useFeatureFlags } from "@/stores/feature-flags";
import { useAddonsStore } from "@/stores/addons-store";
import { addonInvoke } from "./bridge";
import { appendAddonText, registerAddonComposer } from "./composer-registry";
import { addonError } from "./types";
import { activeAddonWorkspace } from "./platform";
export function useAddonComposerAdapter(
  workspaceId: string | null,
  threadId: string | null,
  draft: string,
  onDraftChange: (text: string) => void,
) {
  const enabled = useFeatureFlags((s) => s.enableAgentChat);
  const id = useMemo(() => randomUUID(), [workspaceId, threadId, enabled]);
  const [registered, setRegistered] = useState(false);
  const latest = useRef({ draft, onDraftChange });
  const revision = useRef(0);
  useLayoutEffect(() => {
    latest.current = { draft, onDraftChange };
    revision.current++;
  }, [draft, onDraftChange]);
  useEffect(() => {
    setRegistered(false);
    if (!workspaceId || !enabled) return;
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
            if (disposed || activeAddonWorkspace() !== workspaceId)
              throw addonError("CONTEXT_STALE", "The draft target changed");
            const next = appendAddonText(latest.current.draft, text);
            // Update immediately, including multiple appends before React commits.
            latest.current.draft = next;
            latest.current.onDraftChange(next);
            return ++revision.current;
          },
        });
        setRegistered(true);
      })
      .catch(() => {});
    return () => {
      disposed = true;
      unregister();
      if (useAddonsStore.getState().accessory?.composerId === id)
        useAddonsStore.setState({ accessory: null });
      void addonInvoke("addon_composer_closed", { composerId: id }).catch(
        () => {},
      );
    };
  }, [id, workspaceId, threadId, enabled]);
  return { id, registered };
}
