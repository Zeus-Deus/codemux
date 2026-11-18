/// <reference types="@testing-library/jest-dom/vitest" />
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { EditorView } from "@codemirror/view";
import { undo } from "@codemirror/commands";

// A one-file fake disk: `signature` moves with every write, like size+mtime.
const disk = { content: "", version: 0 };
const writeFile = vi.fn(async (_path: string, content: string) => {
  disk.content = content;
  disk.version++;
});

vi.mock("@/tauri/commands", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/tauri/commands")>()),
  readFile: vi.fn(async () => disk.content),
  fileSignature: vi.fn(async () => `v${disk.version}`),
  writeFile: (path: string, content: string) => writeFile(path, content),
}));

const toastError = vi.fn();
vi.mock("sonner", () => ({ toast: { error: (...args: unknown[]) => toastError(...args) } }));

vi.mock("@/lib/editor-languages", async () => ({
  ...(await vi.importActual<typeof import("@/lib/editor-languages")>("@/lib/editor-languages")),
  loadLanguage: async () => null,
}));

import { EditorPane } from "./EditorPane";
import { useEditorStore } from "@/stores/editor-store";
import { fileSignature, readFile } from "@/tauri/commands";

// CodeMirror measures selection rectangles; jsdom has no layout for ranges.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

const TAB = "tab-1";
const PATH = "/repo/src/app.ts";

function agentWrites(content: string) {
  disk.content = content;
  disk.version++;
}

function delayNextWrite() {
  let finish!: () => void;
  const wait = new Promise<void>((done) => { finish = done; });
  writeFile.mockImplementationOnce(async (_path, content) => {
    await wait;
    agentWrites(content);
  });
  return finish;
}

/** Window focus runs an immediate disk check, so no timers are needed. */
async function checkDisk() {
  await act(async () => {
    window.dispatchEvent(new Event("focus"));
  });
}

async function openEditor(embedded = false, expectedContent = disk.content) {
  const { container } = render(<EditorPane tabId={TAB} embedded={embedded} />);
  await waitFor(() => {
    const el = container.querySelector(".cm-editor");
    expect(el && EditorView.findFromDOM(el as HTMLElement)?.state.doc.toString()).toBe(
      expectedContent,
    );
  });
  return EditorView.findFromDOM(container.querySelector(".cm-editor") as HTMLElement)!;
}

function type(view: EditorView, text: string) {
  act(() => {
    view.dispatch({ changes: { from: view.state.doc.length, insert: text } });
  });
}

function save(view: EditorView) {
  act(() => {
    fireEvent.keyDown(view.contentDOM, { key: "s", code: "KeyS", ctrlKey: true });
  });
}

beforeEach(() => {
  disk.content = "const a = 1;\n";
  disk.version = 1;
  writeFile.mockClear();
  toastError.mockClear();
  useEditorStore.setState({ tabs: { [TAB]: { filePath: PATH, baselineContent: "", isDirty: false } } });
});

afterEach(() => cleanup());

describe("EditorPane disk sync", () => {
  it("restores unsaved edits after switching away and lets the user save them", async () => {
    const before = await openEditor();
    type(before, "// unfinished\n");
    const draft = before.state.doc.toString();
    cleanup();

    const restored = await openEditor(false, draft);
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(true);
    expect(localStorage.getItem("codemux-editor")).not.toContain("unfinished");
    save(restored);
    await waitFor(() => expect(disk.content).toBe(draft));
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);
  });

  it("restores an empty draft and detects an outside edit made while the pane was hidden", async () => {
    const before = await openEditor();
    act(() => before.dispatch({ changes: { from: 0, to: before.state.doc.length } }));
    cleanup();
    agentWrites("// agent edit\n");

    const restored = await openEditor(false, "");
    expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/i);
    save(restored);
    await act(async () => {});
    expect(writeFile).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    expect(restored.state.doc.toString()).toBe("// agent edit\n");
  });

  it("saves an empty raw Markdown draft after switching directly between documents", async () => {
    const markdownTab = "readme";
    const markdownPath = "/repo/README.md";
    disk.content = "# Readme\n";
    useEditorStore.getState().initTab(markdownTab, { filePath: markdownPath });

    const pane = (id: string) => <EditorPane tabId={id} embedded viewMode="raw" />;
    const { container, rerender } = render(pane(markdownTab));
    const view = () => EditorView.findFromDOM(container.querySelector(".cm-editor") as HTMLElement)!;
    await waitFor(() => expect(view().state.doc.toString()).toBe("# Readme\n"));
    act(() => view().dispatch({ changes: { from: 0, to: view().state.doc.length } }));

    vi.mocked(readFile).mockResolvedValueOnce("const a = 1;\n");
    rerender(pane(TAB));
    await waitFor(() => expect(view().state.doc.toString()).toBe("const a = 1;\n"));

    rerender(pane(markdownTab));
    save(view());
    await waitFor(() => expect(writeFile).toHaveBeenCalledWith(markdownPath, ""));
  });

  it("ignores an earlier file read after another file has loaded", async () => {
    let finishOldRead!: (value: string) => void;
    vi.mocked(readFile).mockImplementationOnce(() => new Promise((done) => { finishOldRead = done; }));
    const { container } = render(<EditorPane tabId={TAB} />);
    vi.mocked(readFile).mockResolvedValueOnce("const newer = 2;\n");
    act(() => { useEditorStore.getState().setFilePath(TAB, "/repo/src/newer.ts"); });
    const view = EditorView.findFromDOM(container.querySelector(".cm-editor") as HTMLElement)!;
    await waitFor(() => expect(view.state.doc.toString()).toBe("const newer = 2;\n"));
    await act(async () => { finishOldRead("const older = 1;\n"); });
    expect(view.state.doc.toString()).toBe("const newer = 2;\n");
    expect(useEditorStore.getState().tabs[TAB].baselineContent).toBe("const newer = 2;\n");
  });

  it("ignores an earlier file error after another file has loaded", async () => {
    let failOldRead!: (reason: Error) => void;
    vi.mocked(readFile).mockImplementationOnce(() => new Promise((_done, fail) => { failOldRead = fail; }));
    const { container } = render(<EditorPane tabId={TAB} />);
    vi.mocked(readFile).mockResolvedValueOnce("const newer = 2;\n");
    act(() => { useEditorStore.getState().setFilePath(TAB, "/repo/src/newer.ts"); });
    const view = EditorView.findFromDOM(container.querySelector(".cm-editor") as HTMLElement)!;
    await waitFor(() => expect(view.state.doc.toString()).toBe("const newer = 2;\n"));
    await act(async () => { failOldRead(new Error("old file unavailable")); });
    expect(screen.queryByText(/old file unavailable/)).not.toBeInTheDocument();
  });

  it("does not recreate a removed tab when its file read resolves", async () => {
    let finishRead!: (value: string) => void;
    vi.mocked(readFile).mockImplementationOnce(() => new Promise((done) => { finishRead = done; }));
    const { unmount } = render(<EditorPane tabId={TAB} />);
    unmount();
    useEditorStore.getState().removeTab(TAB);
    await act(async () => { finishRead("late content"); });
    expect(useEditorStore.getState().getTab(TAB)).toBeUndefined();
  });

  it("keeps edits typed during a save marked dirty", async () => {
    const view = await openEditor();
    type(view, "// saved edit\n");
    const saved = view.state.doc.toString();
    const finishWrite = delayNextWrite();
    save(view);
    await waitFor(() => expect(writeFile).toHaveBeenCalledWith(PATH, saved));
    type(view, "// unsaved edit\n");
    await act(async () => { finishWrite(); });
    expect(view.state.doc.toString()).toBe(`${saved}// unsaved edit\n`);
    expect(useEditorStore.getState().tabs[TAB].baselineContent).toBe(saved);
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(true);
  });

  it.each([false, true])("honors a save across a pane switch during the disk check (outside edit: %s)", async (changed) => {
    const original = await openEditor();
    type(original, "// save before switching\n");
    const saving = original.state.doc.toString();
    let finishStat!: (signature: string) => void;
    vi.mocked(fileSignature).mockImplementationOnce(() => new Promise((done) => { finishStat = done; }));
    save(original);
    cleanup();
    const current = await openEditor(false, saving);
    if (changed) agentWrites("// external edit\n");
    await act(async () => { finishStat(`v${disk.version}`); });
    if (changed) {
      expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/i);
      expect(writeFile).not.toHaveBeenCalled();
      expect(current.state.doc.toString()).toBe(saving);
    } else {
      expect(disk.content).toBe(saving);
      expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);
    }
  });

  it("does not reset a remounted pane when the previous pane's write finishes", async () => {
    const original = await openEditor();
    type(original, "// previous pane edit\n");
    const saving = original.state.doc.toString();
    const finishWrite = delayNextWrite();
    save(original);
    await waitFor(() => expect(writeFile).toHaveBeenCalledWith(PATH, saving));
    cleanup();
    const current = await openEditor(false, saving);
    type(current, "// new pane edit\n");
    await act(async () => { finishWrite(); });
    expect(disk.content).toBe(saving);
    expect(current.state.doc.toString()).toBe(`${saving}// new pane edit\n`);
    expect(useEditorStore.getState().tabs[TAB].baselineContent).toBe(saving);
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(true);
    await checkDisk();
    expect(screen.queryByRole("alert")).toBeNull();
    save(current);
    await waitFor(() => expect(disk.content).toBe(`${saving}// new pane edit\n`));
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);
  });

  it("saves an undo back to the old baseline after the previous pane finishes writing", async () => {
    const original = await openEditor();
    const baseline = disk.content;
    type(original, "// first save\n");
    const saving = original.state.doc.toString();
    const finishWrite = delayNextWrite();
    save(original);
    await waitFor(() => expect(writeFile).toHaveBeenCalledOnce());
    act(() => { undo(original); });
    expect(original.state.doc.toString()).toBe(baseline);
    cleanup();

    const current = await openEditor(false, baseline);
    save(current);
    expect(writeFile).toHaveBeenCalledTimes(1);
    cleanup();
    await act(async () => { finishWrite(); });
    await waitFor(() => expect(writeFile.mock.calls).toEqual([[PATH, saving], [PATH, baseline]]));
    expect(disk.content).toBe(baseline);
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);
  });

  it("keeps a completed write from changing a replacement tab with the same file", async () => {
    const original = await openEditor();
    type(original, "// closed tab\n");
    const finishWrite = delayNextWrite();
    save(original);
    await waitFor(() => expect(writeFile).toHaveBeenCalledOnce());
    cleanup();
    const editor = useEditorStore.getState();
    editor.removeTab(TAB);
    editor.initTab(TAB, { filePath: PATH });
    const current = await openEditor();
    const baseline = disk.content;
    type(current, "// replacement tab\n");
    await act(async () => { finishWrite(); });

    expect(editor.getTab(TAB)?.baselineContent).toBe(baseline);
    expect(editor.getTab(TAB)?.isDirty).toBe(true);
    await checkDisk();
    expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/i);
    expect(current.state.doc.toString()).toBe(`${baseline}// replacement tab\n`);
  });

  it("reloads a clean buffer in place when the file changes on disk", async () => {
    const view = await openEditor();
    act(() => view.dispatch({ selection: { anchor: 3 } }));

    agentWrites("const a = 1;\nconst b = 2;\n");
    await checkDisk();

    await waitFor(() => expect(view.state.doc.toString()).toBe("const a = 1;\nconst b = 2;\n"));
    expect(view.state.selection.main.anchor).toBe(3);
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);
    expect(screen.getByRole("status")).toHaveTextContent("Reloaded from disk");
  });

  it("keeps a silent reload out of the undo history", async () => {
    const view = await openEditor();
    agentWrites("const a = 1;\nconst b = 2;\n");
    await checkDisk();
    await waitFor(() => expect(view.state.doc.toString()).toBe("const a = 1;\nconst b = 2;\n"));

    act(() => {
      undo(view);
    });

    expect(view.state.doc.toString()).toBe("const a = 1;\nconst b = 2;\n");
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);
  });

  it("shows the reload and save notices in an embedded pane, whose header is hidden", async () => {
    const view = await openEditor(true);
    agentWrites("const a = 1;\nconst b = 2;\n");
    await checkDisk();
    expect(await screen.findByRole("status")).toHaveTextContent("Reloaded from disk");
    expect(screen.getByRole("status").closest(".hidden")).toBeNull();

    type(view, "// mine\n");
    save(view);
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Saved"));
    expect(screen.getByRole("status").closest(".hidden")).toBeNull();
  });

  it("ignores a disk read that started before a save and resolved after it", async () => {
    const view = await openEditor();
    type(view, "// mine\n");

    let finishRead!: (content: string) => void;
    vi.mocked(readFile).mockImplementationOnce(
      () => new Promise<string>((resolve) => (finishRead = resolve)),
    );
    await checkDisk();
    save(view);
    await waitFor(() => expect(disk.content).toBe("const a = 1;\n// mine\n"));
    await act(async () => finishRead("const a = 1;\n"));

    expect(view.state.doc.toString()).toBe("const a = 1;\n// mine\n");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("asks before touching unsaved edits, and Reload takes the disk version", async () => {
    const view = await openEditor();
    type(view, "// mine\n");

    agentWrites("// theirs\n");
    await checkDisk();

    expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/i);
    expect(view.state.doc.toString()).toBe("const a = 1;\n// mine\n");

    fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    expect(view.state.doc.toString()).toBe("// theirs\n");
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("Keep mine leaves the edits and lets the next save replace the disk version", async () => {
    const view = await openEditor();
    type(view, "// mine\n");
    agentWrites("// theirs\n");
    await checkDisk();

    fireEvent.click(await screen.findByRole("button", { name: "Keep mine" }));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(view.state.doc.toString()).toBe("const a = 1;\n// mine\n");
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(true);

    save(view);
    await waitFor(() => expect(writeFile).toHaveBeenCalledWith(PATH, "const a = 1;\n// mine\n"));
  });

  it("refuses to save over a newer disk version it has not seen", async () => {
    const view = await openEditor();
    type(view, "// mine\n");
    agentWrites("// theirs\n");

    save(view);

    expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/i);
    expect(writeFile).not.toHaveBeenCalled();
    expect(disk.content).toBe("// theirs\n");
  });

  it("does not save over the disk version while the conflict bar is showing", async () => {
    const view = await openEditor();
    type(view, "// mine\n");
    agentWrites("// theirs\n");
    await checkDisk();
    expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/i);

    save(view);
    await act(async () => {});

    expect(writeFile).not.toHaveBeenCalled();
    expect(disk.content).toBe("// theirs\n");
    expect(screen.getByRole("alert")).toHaveTextContent(/changed on disk/i);
  });

  it("a second Ctrl+S after a refused save still does not overwrite", async () => {
    const view = await openEditor();
    type(view, "// mine\n");
    agentWrites("// theirs\n");

    save(view);
    expect(await screen.findByRole("alert")).toHaveTextContent(/changed on disk/i);
    save(view);
    await act(async () => {});

    expect(writeFile).not.toHaveBeenCalled();
    expect(disk.content).toBe("// theirs\n");
  });

  it("saves when the disk is unchanged and confirms it", async () => {
    const view = await openEditor();
    type(view, "// mine\n");

    save(view);

    await waitFor(() => expect(writeFile).toHaveBeenCalledWith(PATH, "const a = 1;\n// mine\n"));
    expect(await screen.findByRole("status")).toHaveTextContent("Saved");
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(false);

    // Its own write is not mistaken for an outside change.
    await checkDisk();
    expect(screen.queryByRole("alert")).toBeNull();
    expect(view.state.doc.toString()).toBe("const a = 1;\n// mine\n");
  });

  it("reports a failed save instead of failing silently", async () => {
    const view = await openEditor();
    type(view, "// mine\n");
    writeFile.mockRejectedValueOnce("Permission denied");

    save(view);

    await waitFor(() =>
      expect(toastError).toHaveBeenCalledWith("Couldn't save app.ts", {
        description: "Permission denied",
      }),
    );
    expect(useEditorStore.getState().tabs[TAB].isDirty).toBe(true);
  });
});
