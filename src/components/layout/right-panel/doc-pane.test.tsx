/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render } from "@testing-library/react";

import { useEditorStore } from "@/stores/editor-store";
import type { WorkspaceSnapshot } from "@/tauri/types";

import { DocPane } from "./doc-pane";
import { docEditorTabId } from "./pane-registry";

vi.mock("@/components/editor/EditorPane", async () => {
  const actual = await vi.importActual<typeof import("@/components/editor/EditorPane")>(
    "@/components/editor/EditorPane",
  );
  return {
    ...actual,
    EditorPane: ({ viewMode }: { viewMode?: string }) => (
      <div data-testid="editor-pane" data-view-mode={viewMode} />
    ),
  };
});

vi.mock("@/components/workspace/file-tree-panel", () => ({
  FileTreePanel: () => <div data-testid="file-tree" />,
}));

const WORKSPACE = { workspace_id: "ws-1" } as WorkspaceSnapshot;
const MARKDOWN = "/work/codemux/docs/INDEX.md";
const SOURCE = "/work/codemux/src/lib/types.ts";

function renderPane(filePath: string, raw = false, onRequestRawView = vi.fn()) {
  const result = render(
    <DocPane
      workspace={WORKSPACE}
      filePath={filePath}
      raw={raw}
      wrap
      treeOpen={false}
      onOpenFile={vi.fn()}
      onSearchFiles={vi.fn()}
      onRequestRawView={onRequestRawView}
      treeRefreshKey={0}
    />,
  );
  return { ...result, onRequestRawView };
}

beforeEach(() => useEditorStore.setState({ tabs: {} }));
afterEach(cleanup);

describe("DocPane — source-reference reveals", () => {
  it("asks the deck for the source view when a markdown line is requested", () => {
    const tabId = docEditorTabId(WORKSPACE.workspace_id, MARKDOWN);
    useEditorStore.getState().initTab(tabId, { filePath: MARKDOWN });
    useEditorStore.getState().requestReveal(tabId, 41);

    const { onRequestRawView } = renderPane(MARKDOWN);
    expect(onRequestRawView).toHaveBeenCalledTimes(1);
  });

  it("does not replay the request after a remount consumed it", () => {
    const tabId = docEditorTabId(WORKSPACE.workspace_id, MARKDOWN);
    useEditorStore.getState().initTab(tabId, { filePath: MARKDOWN });
    useEditorStore.getState().requestReveal(tabId, 41);
    useEditorStore.getState().clearReveal(tabId, 1);

    const { onRequestRawView } = renderPane(MARKDOWN);
    expect(onRequestRawView).not.toHaveBeenCalled();
  });

  it("leaves a non-markdown pane alone — it already renders source", () => {
    const tabId = docEditorTabId(WORKSPACE.workspace_id, SOURCE);
    useEditorStore.getState().initTab(tabId, { filePath: SOURCE });
    useEditorStore.getState().requestReveal(tabId, 42);

    const { onRequestRawView } = renderPane(SOURCE);
    expect(onRequestRawView).not.toHaveBeenCalled();
  });

  it("leaves a pane the user already switched to source alone", () => {
    const tabId = docEditorTabId(WORKSPACE.workspace_id, MARKDOWN);
    useEditorStore.getState().initTab(tabId, { filePath: MARKDOWN });
    useEditorStore.getState().requestReveal(tabId, 41);

    const { onRequestRawView, getByTestId } = renderPane(MARKDOWN, true);
    expect(onRequestRawView).not.toHaveBeenCalled();
    expect(getByTestId("editor-pane")).toHaveAttribute("data-view-mode", "raw");
  });
});
