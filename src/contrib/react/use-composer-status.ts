import { useCallback, useSyncExternalStore } from "react";
import { COMPOSER_STATUS_AREA } from "@/sdk/areas";
import type { ComposerStatusData, ComposerStatusProps } from "@/sdk/host";
import type { Contribution } from "../types";
import { useContributions } from "./use-contributions";

function statusData(contribution: Contribution): ComposerStatusData | null {
  const data = contribution.data;
  if (typeof data !== "object" || data === null) return null;
  if (!("subscribe" in data) || typeof data.subscribe !== "function") return null;
  if (!("isVisible" in data) || typeof data.isVisible !== "function") return null;
  return data as ComposerStatusData;
}

function shows(contribution: Contribution, props: ComposerStatusProps): boolean {
  if (!contribution.render) return false;
  const data = statusData(contribution);
  if (!data) return true;
  // A throwing predicate counts as shown: the slot's error boundary then
  // reports the plugin instead of the section silently disappearing.
  try {
    return data.isVisible(props);
  } catch {
    return true;
  }
}

/**
 * Whether any `COMPOSER_STATUS_AREA` chip currently draws something for this
 * workspace and thread. Follows each chip's `ComposerStatusData`, so a
 * section can drop its heading while every chip is idle.
 */
export function useHasComposerStatus(props: ComposerStatusProps): boolean {
  const items = useContributions(COMPOSER_STATUS_AREA);
  const subscribe = useCallback(
    (onChange: () => void) => {
      const offs = items.map((c) => statusData(c)?.subscribe(onChange));
      return () => {
        for (const off of offs) off?.();
      };
    },
    [items],
  );
  const { workspaceId, threadId } = props;
  const getSnapshot = useCallback(
    () => items.some((c) => shows(c, { workspaceId, threadId })),
    [items, workspaceId, threadId],
  );
  return useSyncExternalStore(subscribe, getSnapshot, getSnapshot);
}
