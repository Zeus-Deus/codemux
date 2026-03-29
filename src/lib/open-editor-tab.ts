import { useEditorStore } from "@/stores/editor-store";
import { createTab, activateTab, renameTab } from "@/tauri/commands";
import type { TabSnapshot } from "@/tauri/types";

export async function openEditorTab(
  workspaceId: string,
  tabs: TabSnapshot[],
  filePath: string,
): Promise<string> {
  const store = useEditorStore.getState();
  const existing = tabs.find(
    (t) => t.kind === "editor" && store.getTab(t.tab_id)?.filePath === filePath,
  );
  if (existing) {
    await activateTab(workspaceId, existing.tab_id);
    return existing.tab_id;
  }

  const tabId = await createTab(workspaceId, "editor");
  useEditorStore.getState().initTab(tabId, { filePath });

  const filename = filePath.split("/").pop() ?? "Editor";
  await renameTab(workspaceId, tabId, filename);

  return tabId;
}
