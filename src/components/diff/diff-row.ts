import type { ReactNode } from "react";
import type { DiffLine } from "@/lib/diff-parser";

/**
 * How one diff row is painted.
 *
 * Lifted out of the two views rather than left duplicated in both: the
 * add/delete/conflict colouring was written twice, character for
 * character, and a review surface that adds a third state to it would
 * have written it a third time.
 */
export interface DiffRowStyle {
  bgClass: string;
  prefixChar: string;
  prefixColor: string;
  isOursMarker: boolean;
  isTheirsMarker: boolean;
}

export function diffRowStyle(line: DiffLine): DiffRowStyle {
  // Conflict markers are sniffed from content — the parser has no
  // opinion about them, and a conflicted file is still a diff.
  const isOursMarker = line.content.startsWith("<<<<<<<");
  const isSeparator =
    line.content.startsWith("=======") && !line.content.startsWith("========");
  const isTheirsMarker = line.content.startsWith(">>>>>>>");
  const isConflictMarker = isOursMarker || isSeparator || isTheirsMarker;

  return {
    bgClass: isOursMarker
      ? "bg-primary/15 border-l-2 border-primary"
      : isTheirsMarker
        ? "bg-accent-violet/15 border-l-2 border-accent-violet"
        : isSeparator
          ? "bg-muted/40 border-l-2 border-muted-foreground"
          : line.type === "add"
            ? "bg-success/10"
            : line.type === "del"
              ? "bg-danger/10"
              : "",
    prefixChar: line.type === "add" ? "+" : line.type === "del" ? "-" : " ",
    prefixColor: isConflictMarker
      ? "text-muted-foreground"
      : line.type === "add"
        ? "text-success"
        : line.type === "del"
          ? "text-danger"
          : "text-muted-foreground",
    isOursMarker,
    isTheirsMarker,
  };
}

/** Which file a note on this row belongs to. */
export type DiffRowSide = "LEFT" | "RIGHT";

/**
 * A deleted line is addressable against the old file; everything else
 * against the new one. This is the host's own rule, and getting it
 * wrong puts a note about code you removed onto code you added.
 */
export function sideOf(line: DiffLine): DiffRowSide {
  return line.type === "del" ? "LEFT" : "RIGHT";
}

export function lineNumberOn(line: DiffLine, side: DiffRowSide): number | null {
  return side === "LEFT" ? line.oldLine : line.newLine;
}

/**
 * Optional line-review behaviour for a diff view.
 *
 * Every field is optional to the views' existing callers — the Changes
 * pane passes none of it and renders exactly what it rendered before.
 */
export interface DiffSelection {
  isSelected: (line: DiffLine, side: DiffRowSide) => boolean;
  onSelect: (line: DiffLine, side: DiffRowSide, shiftKey: boolean) => void;
  /** Rendered directly beneath a row: the composer, or a pending note. */
  renderUnder?: (line: DiffLine, side: DiffRowSide) => ReactNode;
}

/** Ember tint, ember left rule, ember line numbers — the selected row. */
export const SELECTED_ROW_CLASS =
  "bg-accent-ember/[0.13] border-l-2 border-accent-ember";
export const SELECTED_NUMBER_CLASS = "text-accent-ember";
