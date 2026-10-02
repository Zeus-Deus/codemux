import { createContext, useContext } from "react";

/**
 * Height of window chrome floating over the top of the transcript viewport.
 *
 * A lone chat runs its transcript to the window's top edge, under the
 * floating titlebar band, so text scrolls behind the band the same way it
 * scrolls behind the docked composer. The viewport itself is not inset;
 * instead every "park this row near the top" target (send anchor, search
 * and trail jumps) adds this much, so a row the reader asked to see never
 * lands behind the band's controls. Zero everywhere the band has in-flow
 * room of its own (splits, mobile, legacy chrome).
 */
export const TranscriptTopInsetContext = createContext(0);

export function useTranscriptTopInset(): number {
  return useContext(TranscriptTopInsetContext);
}
