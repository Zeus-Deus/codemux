/**
 * WebKit cannot paint CSS Custom Highlight ranges for text represented by an
 * anonymous flex/grid/table layout item (WebKit #307455). Giving prose text a
 * real inline box avoids that renderer path and keeps every selected fragment
 * on CodeMux's single, precisely colored Custom Highlight layer.
 */

export const CHAT_SELECTION_TEXT_ATTRIBUTE = "data-codemux-selection-text";

interface HastNode {
  type: string;
  tagName?: string;
  value?: string;
  properties?: Record<string, unknown>;
  children?: HastNode[];
}

function wrapProseText(node: HastNode): void {
  if (!node.children) return;

  node.children = node.children.map((child) => {
    if (child.type === "text" && child.value?.trim()) {
      return {
        type: "element",
        tagName: "span",
        properties: { [CHAT_SELECTION_TEXT_ATTRIBUTE]: "" },
        children: [child],
      };
    }
    wrapProseText(child);
    return child;
  });
}

/** Rehype plugin entry point. Whitespace-only scaffolding stays as bare text
 * so inserting wrappers cannot create anonymous inline rows between blocks. */
export function rehypeSelectionSafeText() {
  return (tree: HastNode) => wrapProseText(tree);
}
