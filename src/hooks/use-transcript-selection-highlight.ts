import { useEffect } from "react";

export const TRANSCRIPT_SELECTION_HIGHLIGHT =
  "codemux-transcript-selection";
export const TRANSCRIPT_SELECTION_CLASS =
  "transcript-selection-highlight";

const TRANSCRIPT_SELECTOR = '[data-slot="transcript-list"]';

function rangeIntersectsNode(range: Range, node: Node): boolean {
  try {
    return range.intersectsNode(node);
  } catch {
    return false;
  }
}

function isSelectableText(node: Text, transcript: Element): boolean {
  let element = node.parentElement;
  while (element) {
    const style = getComputedStyle(element);
    const values = [
      style.userSelect,
      (style as CSSStyleDeclaration & { webkitUserSelect?: string })
        .webkitUserSelect,
    ];
    if (values.includes("none")) return false;
    if (values.includes("text") || values.includes("all")) return true;
    if (element === transcript) return false;
    element = element.parentElement;
  }
  return false;
}

/**
 * Split the browser's native selection into text-only ranges inside Agent
 * Chat transcripts. Structural whitespace nodes are deliberately omitted:
 * WebKitGTK expands those nodes to their virtualized row boxes when painting
 * one range across multiple messages.
 */
export function collectTranscriptSelectionRanges(
  selection: Selection,
  scope: ParentNode = document,
): Range[] {
  if (selection.isCollapsed || selection.rangeCount === 0) return [];

  const pieces: Range[] = [];
  const transcripts = scope.querySelectorAll(TRANSCRIPT_SELECTOR);

  for (let rangeIndex = 0; rangeIndex < selection.rangeCount; rangeIndex += 1) {
    const selectionRange = selection.getRangeAt(rangeIndex);

    for (const transcript of transcripts) {
      if (!rangeIntersectsNode(selectionRange, transcript)) continue;

      const walker = transcript.ownerDocument.createTreeWalker(
        transcript,
        NodeFilter.SHOW_TEXT,
      );
      let current = walker.nextNode();
      while (current) {
        const text = current as Text;
        if (
          text.data.trim().length > 0 &&
          rangeIntersectsNode(selectionRange, text) &&
          isSelectableText(text, transcript)
        ) {
          const start =
            text === selectionRange.startContainer
              ? selectionRange.startOffset
              : 0;
          const end =
            text === selectionRange.endContainer
              ? selectionRange.endOffset
              : text.length;

          if (start < end) {
            const piece = transcript.ownerDocument.createRange();
            piece.setStart(text, start);
            piece.setEnd(text, end);
            pieces.push(piece);
          }
        }
        current = walker.nextNode();
      }
    }
  }

  return pieces;
}

/**
 * Paint transcript selections from exact text-node ranges while leaving the
 * native Selection untouched for copy, keyboard interaction, and assistive
 * technology. Browsers without the Custom Highlight API retain their normal
 * native selection because the CSS suppression class is never installed.
 */
export function useTranscriptSelectionHighlight(): void {
  useEffect(() => {
    if (
      typeof CSS === "undefined" ||
      !("highlights" in CSS) ||
      typeof Highlight === "undefined"
    ) {
      return;
    }

    const root = document.documentElement;
    let frame = 0;

    const paint = () => {
      frame = 0;
      const selection = document.getSelection();
      const ranges = selection
        ? collectTranscriptSelectionRanges(selection)
        : [];

      if (ranges.length === 0) {
        CSS.highlights.delete(TRANSCRIPT_SELECTION_HIGHLIGHT);
        return;
      }
      CSS.highlights.set(
        TRANSCRIPT_SELECTION_HIGHLIGHT,
        new Highlight(...ranges),
      );
    };

    const schedulePaint = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = requestAnimationFrame(paint);
    };

    root.classList.add(TRANSCRIPT_SELECTION_CLASS);
    document.addEventListener("selectionchange", schedulePaint);
    schedulePaint();

    return () => {
      document.removeEventListener("selectionchange", schedulePaint);
      if (frame) cancelAnimationFrame(frame);
      CSS.highlights.delete(TRANSCRIPT_SELECTION_HIGHLIGHT);
      root.classList.remove(TRANSCRIPT_SELECTION_CLASS);
    };
  }, []);
}
