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
import { readFile } from "@/tauri/commands";

// CodeMirror measures selection rectangles; jsdom has no layout for ranges.
Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect = () => new DOMRect();

const TAB = "tab-1";
const PATH = "/repo/src/app.ts";

function agentWrites(content: string) {
  disk.content = content;
  disk.version++;
}

/** Window focus runs an immediate disk check, so no timers are needed. */
async function checkDisk() {
  await act(async () => {
    window.dispatchEvent(new Event("focus"));
  });
}

async function openEditor(embedded = false) {
  const { container } = render(<EditorPane tabId={TAB} embedded={embedded} />);
  await waitFor(() => {
    const el = container.querySelector(".cm-editor");
    expect(el && EditorView.findFromDOM(el as HTMLElement)?.state.doc.toString()).toBe(
      disk.content,
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

  it("ignores a disk check that read the file while a save was in flight", async () => {
    const view = await openEditor();
    type(view, "// mine\n");

    // Hold the write open, start a check that reads the old file, then let
    // the write land before the check's read resolves.
    let finishWrite!: () => void;
    writeFile.mockImplementationOnce(
      (_path: string, content: string) =>
        new Promise<void>((resolve) => {
          finishWrite = () => {
            disk.content = content;
            disk.version++;
            resolve();
          };
        }),
    );
    save(view);
    await waitFor(() => expect(writeFile).toHaveBeenCalled());

    let finishRead!: (content: string) => void;
    vi.mocked(readFile).mockImplementationOnce(
      () => new Promise<string>((resolve) => (finishRead = resolve)),
    );
    agentWrites(disk.content); // the truncate step of the write moves the stat
    await checkDisk();
    await act(async () => finishWrite());
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
