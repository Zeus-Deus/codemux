import { useEffect, useRef, type RefObject } from "react";
import type { LegendListRef } from "@legendapp/list/react";
import {
  buildTrailEntries,
  type TrailEntry,
} from "@/components/chat/message-trail";
import type { TranscriptSlot } from "@/components/chat/transcript-slots";

export interface MobileHistorySource {
  workspaceId: string;
  viewport: () => HTMLElement | null | undefined;
  entries: () => TrailEntry[];
  jump: (messageId: string) => Promise<void>;
}
// Ephemeral view bindings only; never persist transcript text or list instances.
const sources = new Set<MobileHistorySource>();
export function registerMobileHistory(source: MobileHistorySource) {
  sources.add(source);
  return () => {
    sources.delete(source);
  };
}
export function visibleMobileHistory(workspaceId: string) {
  return [...sources].find((source) => {
    if (source.workspaceId !== workspaceId) return false;
    const node = source.viewport();
    return (
      node &&
      node.clientWidth > 0 &&
      node.clientHeight > 0 &&
      !node.closest("[inert]") &&
      getComputedStyle(node).visibility !== "hidden"
    );
  });
}

export function useMobileHistory({
  enabled,
  workspaceId,
  threadKey,
  slots,
  listRef,
  onNavigate,
}: {
  enabled: boolean;
  workspaceId?: string | null;
  threadKey?: string | null;
  slots: TranscriptSlot[];
  listRef: RefObject<LegendListRef | null>;
  onNavigate: () => void;
}) {
  const current = useRef({ slots, onNavigate });
  current.current = { slots, onNavigate };
  useEffect(() => {
    if (!enabled || !workspaceId) return;
    return registerMobileHistory({
      workspaceId,
      viewport: () => listRef.current?.getScrollableNode(),
      // Read on demand, not on each streaming character.
      entries: () => buildTrailEntries(current.current.slots),
      jump: async (messageId) => {
        const entry = buildTrailEntries(current.current.slots).find(
          (e) => e.messageId === messageId,
        );
        if (!entry || !listRef.current)
          throw new Error("This turn is no longer available.");
        current.current.onNavigate();
        await listRef.current.scrollToIndex({
          index: entry.slotIndex,
          animated: false,
          viewOffset: 10,
        });
      },
    });
  }, [enabled, workspaceId, threadKey, listRef]);
}
