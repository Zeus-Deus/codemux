import {
  BookOpen,
  CheckSquare,
  FileText,
  FolderSearch,
  Globe,
  Pencil,
  Search,
  Terminal,
  Wrench,
  type LucideIcon,
} from "lucide-react";

/**
 * Shared tool → icon / category-tint mapping (design D6). One source of
 * truth for both the single tool card and the tool group card so their
 * chips stay consistent.
 */

export type ToolCategory = "search" | "file" | "terminal" | "web" | "other";

const CATEGORY_BY_TOOL: Record<string, ToolCategory> = {
  Grep: "search",
  Glob: "search",
  WebSearch: "search",
  Read: "file",
  Write: "file",
  Edit: "file",
  MultiEdit: "file",
  NotebookEdit: "file",
  Bash: "terminal",
  WebFetch: "web",
};

export function toolCategory(toolName: string): ToolCategory {
  return CATEGORY_BY_TOOL[toolName] ?? "other";
}

const ICON_BY_TOOL: Record<string, LucideIcon> = {
  Read: FileText,
  Write: Pencil,
  Edit: Pencil,
  MultiEdit: Pencil,
  Bash: Terminal,
  Glob: FolderSearch,
  Grep: Search,
  WebFetch: Globe,
  WebSearch: Globe,
  TodoWrite: CheckSquare,
  NotebookEdit: BookOpen,
};

export function toolIcon(toolName: string): LucideIcon {
  return ICON_BY_TOOL[toolName] ?? Wrench;
}

const ICON_BY_CATEGORY: Record<ToolCategory, LucideIcon> = {
  search: Search,
  file: FileText,
  terminal: Terminal,
  web: Globe,
  other: Wrench,
};

export function categoryIcon(category: ToolCategory): LucideIcon {
  return ICON_BY_CATEGORY[category];
}

/** Tailwind classes for the 22px tinted icon chip. Search stays sky, web
 *  goes violet, everything else neutral — a subtle ~15% wash. */
export function categoryTint(category: ToolCategory): string {
  switch (category) {
    case "search":
      return "bg-status-remote/15 text-status-remote";
    case "web":
      return "bg-accent-violet/15 text-accent-violet";
    case "file":
    case "terminal":
    case "other":
      return "bg-foreground/10 text-muted-foreground";
  }
}
