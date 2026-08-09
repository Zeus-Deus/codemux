import {
  docEditorTabId,
  docPaneId,
} from "@/components/layout/right-panel/pane-registry";
import { useEditorStore } from "@/stores/editor-store";
import { useUIStore } from "@/stores/ui-store";

/** Open or focus one source file in the existing right-panel doc deck. */
export function openRightPanelDoc(
  workspaceId: string,
  filePath: string,
  line?: number,
  column?: number,
) {
  const tabId = docEditorTabId(workspaceId, filePath);
  const editor = useEditorStore.getState();
  editor.initTab(tabId, { filePath });
  if (line != null) editor.requestReveal(tabId, line, column);
  useUIStore.getState().setRightPanelTab(workspaceId, docPaneId(filePath));
}
