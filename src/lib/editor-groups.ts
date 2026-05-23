import type { EditorInfo } from "@/tauri/types";

/**
 * Editors are partitioned into families so the launcher dropdown and
 * workspace context menu can render them as labelled sections (e.g. all
 * JetBrains products together). The Rust candidate list owns the canonical
 * order — this module only adds a layer of grouping on top of whatever the
 * backend returned, preserving relative order within each group.
 *
 * If a new editor id is added on the backend without a group mapping here,
 * it lands in the "Other" bucket — safe default, no UI crash.
 */
export type EditorGroupId = "vscode" | "modern" | "jetbrains" | "other";

export interface EditorGroup {
  id: EditorGroupId;
  label: string;
  editors: EditorInfo[];
}

const GROUP_LABEL: Record<EditorGroupId, string> = {
  vscode: "VS Code family",
  modern: "Modern editors",
  jetbrains: "JetBrains",
  other: "Other",
};

const GROUP_ORDER: EditorGroupId[] = ["vscode", "modern", "jetbrains", "other"];

const ID_TO_GROUP: Record<string, EditorGroupId> = {
  // VS Code family — forks/derivatives that share the VS Code UX
  code: "vscode",
  cursor: "vscode",
  windsurf: "vscode",
  trae: "vscode",
  codium: "vscode",
  // Modern standalone editors
  zed: "modern",
  lapce: "modern",
  // JetBrains family (Fleet included — shares Toolbox + visual language)
  fleet: "jetbrains",
  idea: "jetbrains",
  pycharm: "jetbrains",
  phpstorm: "jetbrains",
  webstorm: "jetbrains",
  goland: "jetbrains",
  rubymine: "jetbrains",
  clion: "jetbrains",
  rider: "jetbrains",
  datagrip: "jetbrains",
  studio: "jetbrains",
  // Other
  sublime_text: "other",
};

export function groupForEditorId(id: string): EditorGroupId {
  return ID_TO_GROUP[id] ?? "other";
}

/**
 * Partition the detected editors into ordered groups. Empty groups are
 * omitted so the UI doesn't render an "JetBrains" header above nothing.
 */
export function groupEditors(editors: EditorInfo[]): EditorGroup[] {
  const buckets: Record<EditorGroupId, EditorInfo[]> = {
    vscode: [],
    modern: [],
    jetbrains: [],
    other: [],
  };
  for (const editor of editors) {
    buckets[groupForEditorId(editor.id)].push(editor);
  }
  return GROUP_ORDER.filter((id) => buckets[id].length > 0).map((id) => ({
    id,
    label: GROUP_LABEL[id],
    editors: buckets[id],
  }));
}
