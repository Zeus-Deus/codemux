/**
 * Doc pane — a file opened as a pane in the right-panel deck.
 *
 * The body is the existing `EditorPane`, mounted `embedded` so the deck's
 * shared pane bar (not a second toolbar) carries the source toggle, wrap,
 * copy and file-tree controls. Editing, saving (Ctrl+S), syntax
 * highlighting, image and rendered-markdown views are the same code paths
 * the main-area editor tab uses — this pane changes where the file shows
 * up, not what a file view can do.
 *
 * Pane state lives in the deck, keyed by pane id, so switching tabs and
 * coming back preserves raw/wrap/tree exactly as you left it.
 */
import { useEffect, useRef } from "react";
import { Search } from "lucide-react";

import { EditorPane, isMarkdownFile } from "@/components/editor/EditorPane";
import { FileTreePanel } from "@/components/workspace/file-tree-panel";
import { useEditorStore } from "@/stores/editor-store";
import type { WorkspaceSnapshot } from "@/tauri/types";
import { docEditorTabId } from "./pane-registry";

export function DocPane({
  workspace,
  filePath,
  raw,
  wrap,
  treeOpen,
  onOpenFile,
  onSearchFiles,
  onRequestRawView,
  treeRefreshKey,
}: {
  workspace: WorkspaceSnapshot;
  filePath: string;
  raw: boolean;
  wrap: boolean;
  treeOpen: boolean;
  onOpenFile: (filePath: string) => void;
  onSearchFiles: () => void;
  /** Asks the deck to show source — a source reference targeting a line in a
   *  markdown file has nothing to reveal in the rendered view. */
  onRequestRawView?: () => void;
  treeRefreshKey: number;
}) {
  const tabId = docEditorTabId(workspace.workspace_id, filePath);
  const storedPath = useEditorStore((s) => s.tabs[tabId]?.filePath ?? null);
  const revealNonce = useEditorStore((s) => s.tabs[tabId]?.revealRequest?.nonce);

  // The deck seeds this when it opens the pane; this is the reload path
  // (persisted pane list restored before the editor store is consulted).
  useEffect(() => {
    if (storedPath !== filePath) {
      useEditorStore.getState().initTab(tabId, { filePath });
    }
  }, [tabId, filePath, storedPath]);

  // A citation with a line number has to land in the source view: a markdown
  // doc opens rendered, where the CodeMirror container is hidden and the
  // reveal would be invisible. Flip the deck's own raw flag rather than
  // overriding it locally, so the pane bar's rendered/source toggle keeps
  // matching what the pane shows. The nonce guard makes this fire once per
  // request — a remount after the reveal was consumed leaves it alone.
  const revealedNonceRef = useRef<number | undefined>(undefined);
  useEffect(() => {
    if (revealNonce == null || revealNonce === revealedNonceRef.current) return;
    revealedNonceRef.current = revealNonce;
    if (!raw && isMarkdownFile(filePath)) onRequestRawView?.();
  }, [filePath, onRequestRawView, raw, revealNonce]);

  return (
    <div className="flex h-full min-h-0">
      <div className="min-w-0 flex-1">
        <EditorPane
          tabId={tabId}
          embedded
          viewMode={raw ? "raw" : "rendered"}
          wrap={wrap}
        />
      </div>
      {treeOpen && (
        <div
          data-testid="doc-pane-tree"
          className="flex w-[196px] shrink-0 flex-col border-l border-border/60 bg-foreground/[0.02]"
        >
          <button
            type="button"
            onClick={onSearchFiles}
            className="flex h-[31px] shrink-0 items-center gap-[7px] border-b border-border/60 px-2.5 text-[11.5px] text-muted-foreground transition-colors hover:bg-foreground/6 hover:text-foreground/80"
          >
            <Search className="size-3" strokeWidth={1.6} aria-hidden />
            Search files
          </button>
          <div className="min-h-0 flex-1">
            <FileTreePanel
              workspace={workspace}
              onOpenFile={onOpenFile}
              refreshKey={treeRefreshKey}
              selectedPath={filePath}
            />
          </div>
        </div>
      )}
    </div>
  );
}
