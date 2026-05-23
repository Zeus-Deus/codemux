import { describe, expect, it } from "vitest";

import { groupEditors, groupForEditorId } from "./editor-groups";
import type { EditorInfo } from "@/tauri/types";

const makeEditor = (id: string, name = id): EditorInfo => ({
  id,
  name,
  command: `/usr/bin/${id}`,
});

describe("groupForEditorId", () => {
  it("maps known editors to their family", () => {
    expect(groupForEditorId("code")).toBe("vscode");
    expect(groupForEditorId("cursor")).toBe("vscode");
    expect(groupForEditorId("windsurf")).toBe("vscode");
    expect(groupForEditorId("zed")).toBe("modern");
    expect(groupForEditorId("idea")).toBe("jetbrains");
    expect(groupForEditorId("pycharm")).toBe("jetbrains");
    expect(groupForEditorId("studio")).toBe("jetbrains");
    expect(groupForEditorId("sublime_text")).toBe("other");
  });

  it("falls back to 'other' for unknown ids — keeps the UI working when the backend adds a candidate before the frontend mapping catches up", () => {
    expect(groupForEditorId("brand-new-editor-9000")).toBe("other");
  });
});

describe("groupEditors", () => {
  it("omits empty groups so the UI doesn't render headers with no items", () => {
    const result = groupEditors([makeEditor("code"), makeEditor("cursor")]);
    expect(result).toHaveLength(1);
    expect(result[0].id).toBe("vscode");
    expect(result[0].editors.map((e) => e.id)).toEqual(["code", "cursor"]);
  });

  it("preserves the order each editor came in within its group", () => {
    // Simulates the backend returning candidates in its declared order
    const detected = [
      makeEditor("code"),
      makeEditor("cursor"),
      makeEditor("idea"),
      makeEditor("pycharm"),
      makeEditor("sublime_text"),
    ];
    const result = groupEditors(detected);
    expect(result.map((g) => g.id)).toEqual(["vscode", "jetbrains", "other"]);
    expect(result[0].editors.map((e) => e.id)).toEqual(["code", "cursor"]);
    expect(result[1].editors.map((e) => e.id)).toEqual(["idea", "pycharm"]);
    expect(result[2].editors.map((e) => e.id)).toEqual(["sublime_text"]);
  });

  it("emits groups in the canonical order (vscode → modern → jetbrains → other)", () => {
    // Provide them out-of-order on input to confirm the function imposes
    // the canonical group order rather than echoing input order across
    // groups
    const detected = [
      makeEditor("sublime_text"),
      makeEditor("idea"),
      makeEditor("zed"),
      makeEditor("code"),
    ];
    const result = groupEditors(detected);
    expect(result.map((g) => g.id)).toEqual([
      "vscode",
      "modern",
      "jetbrains",
      "other",
    ]);
  });

  it("returns an empty array when no editors are detected", () => {
    expect(groupEditors([])).toEqual([]);
  });

  it("routes unknown editor ids into 'other' rather than dropping them", () => {
    const result = groupEditors([
      makeEditor("code"),
      makeEditor("totally-new-thing"),
    ]);
    expect(result).toHaveLength(2);
    expect(result[1].id).toBe("other");
    expect(result[1].editors.map((e) => e.id)).toEqual(["totally-new-thing"]);
  });
});
