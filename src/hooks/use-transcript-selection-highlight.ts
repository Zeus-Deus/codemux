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

const INLINE_DISPLAY = /^(inline|ruby)/;

/** Does this sibling share an inline formatting context with the whitespace
 *  next to it? Text always does; elements only when they lay out inline. */
function rendersInline(node: ChildNode | null): boolean {
  if (!node) return false;
  if (node.nodeType === Node.TEXT_NODE) {
    return (node as Text).data.trim().length > 0;
  }
  if (node.nodeType !== Node.ELEMENT_NODE) return false;
  if ((node as Element).tagName === "BR") return false;
  const display = getComputedStyle(node as Element).display;
  // A tree with no default style sheet reports no display at all. Treat that
  // as inline so prose spacing still paints instead of silently dropping out.
  return display === "" || INLINE_DISPLAY.test(display);
}

/**
 * Whitespace-only text nodes are two different things wearing one face. The
 * newline between two block rows of rendered markup is scaffolding — painting
 * it is what makes WebKitGTK blow the highlight up to the whole virtualized
 * row box. The space between two inline elements (`<em>a</em> <em>b</em>`) is
 * real prose, and dropping it punches a hole in the middle of a selection.
 * Inline neighbours on both sides separate the two cases without needing to
 * know anything about the surrounding row scaffolding.
 */
function isPaintableWhitespace(text: Text): boolean {
  return rendersInline(text.previousSibling) && rendersInline(text.nextSibling);
}

// A renderer may keep a hard break and its source indentation inside the same
// text node as visible prose (`<br>\n    next line`). Checking only whether the
// whole node is whitespace therefore misses it. WebKitGTK paints a Range that
// contains that hidden break as the gap between the two line boxes. Limit the
// trim to structural edges: a newline between inline prose is a real collapsed
// space, while whitespace in preformatted content must remain selectable.
const LEADING_STRUCTURAL_BREAK =
  /^[ \t\f\v]*(?:(?:\r\n?|\n|\u2028|\u2029)[ \t\f\v]*)+/;
const TRAILING_STRUCTURAL_BREAK =
  /(?:[ \t\f\v]*(?:\r\n?|\n|\u2028|\u2029))+[ \t\f\v]*$/;
const PRESERVED_WHITESPACE = /^(?:pre|pre-wrap|break-spaces)$/;

function appendTextRange(
  pieces: Range[],
  text: Text,
  rawStart: number,
  rawEnd: number,
): void {
  let start = rawStart;
  let end = rawEnd;
  const whitespace = text.parentElement
    ? getComputedStyle(text.parentElement).whiteSpace
    : "";

  if (
    !PRESERVED_WHITESPACE.test(whitespace) &&
    !rendersInline(text.previousSibling)
  ) {
    const leading = text.data.match(LEADING_STRUCTURAL_BREAK);
    if (leading) start = Math.max(start, leading[0].length);
  }
  if (
    !PRESERVED_WHITESPACE.test(whitespace) &&
    !rendersInline(text.nextSibling)
  ) {
    const trailing = text.data.match(TRAILING_STRUCTURAL_BREAK);
    if (trailing?.index !== undefined) end = Math.min(end, trailing.index);
  }

  if (start >= end) return;
  const piece = text.ownerDocument.createRange();
  piece.setStart(text, start);
  piece.setEnd(text, end);
  pieces.push(piece);
}

/**
 * Split the browser's native selection into text-only ranges inside Agent
 * Chat transcripts. Structural whitespace nodes are deliberately omitted:
 * WebKitGTK expands those nodes to their virtualized row boxes when painting
 * one range across multiple messages. Whitespace that is part of a line of
 * prose still counts as text — see `isPaintableWhitespace`.
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
          (text.data.trim().length > 0 || isPaintableWhitespace(text)) &&
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

          if (start < end) appendTextRange(pieces, text, start, end);
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

    // Native paint is suppressed while a selection lives, so rows the
    // virtualizer remounts mid-selection arrive with text nodes no range
    // covers and read as unselected until the user moves the caret. Rows come
    // and go through the DOM, so watching for that is enough — the highlight
    // ranges themselves are live and follow scrolling on their own. The
    // observer only runs while something is actually selected.
    const observer = new MutationObserver(() => schedulePaint());

    const watchTranscripts = () => {
      observer.disconnect();
      for (const transcript of document.querySelectorAll(TRANSCRIPT_SELECTOR)) {
        observer.observe(transcript, { childList: true, subtree: true });
      }
    };

    const paint = () => {
      frame = 0;
      const selection = document.getSelection();
      const ranges = selection
        ? collectTranscriptSelectionRanges(selection)
        : [];

      if (selection && !selection.isCollapsed && selection.rangeCount > 0) {
        watchTranscripts();
      } else {
        observer.disconnect();
      }

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
      observer.disconnect();
      if (frame) cancelAnimationFrame(frame);
      CSS.highlights.delete(TRANSCRIPT_SELECTION_HIGHLIGHT);
      root.classList.remove(TRANSCRIPT_SELECTION_CLASS);
    };
  }, []);
}
