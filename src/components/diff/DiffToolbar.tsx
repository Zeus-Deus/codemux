import { Button } from "@/components/ui/button";
import {
  Columns2,
  AlignJustify,
  ChevronUp,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  Target,
  X,
} from "lucide-react";
import { useDiffStore, type DiffTabState } from "@/stores/diff-store";
import { closeTab } from "@/tauri/commands";

interface Props {
  tabId: string;
  workspaceId: string;
  tab: DiffTabState;
  fileCount: number;
  fileIndex: number;
  onPrevHunk: () => void;
  onNextHunk: () => void;
  onPrevFile: () => void;
  onNextFile: () => void;
}

export function DiffToolbar({
  tabId,
  workspaceId,
  tab,
  fileCount,
  fileIndex,
  onPrevHunk,
  onNextHunk,
  onPrevFile,
  onNextFile,
}: Props) {
  const setLayout = useDiffStore((s) => s.setLayout);
  const toggleFocusMode = useDiffStore((s) => s.toggleFocusMode);
  const setSection = useDiffStore((s) => s.setSection);

  const handleClose = () => {
    closeTab(workspaceId, tabId).catch(console.error);
  };

  return (
    <div className="flex h-7 shrink-0 items-center gap-1 border-b border-border/30 bg-card px-2">
      {/* File path */}
      <span className="text-xs font-mono text-muted-foreground truncate min-w-0">
        {tab.filePath ?? "No file selected"}
      </span>

      <div className="flex-1" />

      {/* Focus mode section selector */}
      {tab.focusMode && (
        <div className="flex items-center gap-0.5 mr-1">
          {(["staged", "unstaged", "all"] as const).map((s) => (
            <Button
              key={s}
              size="xs"
              variant={tab.section === s ? "secondary" : "ghost"}
              className="h-5 px-1.5 text-[10px]"
              onClick={() => setSection(tabId, s)}
            >
              {s === "all" ? "All" : s.charAt(0).toUpperCase() + s.slice(1)}
            </Button>
          ))}
        </div>
      )}

      {/* Layout toggle */}
      <div className="flex items-center">
        <Button
          size="icon-xs"
          variant={tab.layout === "split" ? "secondary" : "ghost"}
          onClick={() => setLayout(tabId, "split")}
          title="Split view"
        >
          <Columns2 className="h-3 w-3" />
        </Button>
        <Button
          size="icon-xs"
          variant={tab.layout === "unified" ? "secondary" : "ghost"}
          onClick={() => setLayout(tabId, "unified")}
          title="Unified view"
        >
          <AlignJustify className="h-3 w-3" />
        </Button>
      </div>

      {/* Separator */}
      <div className="w-px h-3.5 bg-border/50 mx-0.5" />

      {/* Hunk nav */}
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={onPrevHunk}
        title="Previous change"
      >
        <ChevronUp className="h-3 w-3" />
      </Button>
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={onNextHunk}
        title="Next change"
      >
        <ChevronDown className="h-3 w-3" />
      </Button>

      {/* Separator */}
      <div className="w-px h-3.5 bg-border/50 mx-0.5" />

      {/* File nav */}
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={onPrevFile}
        disabled={fileCount <= 1}
        title="Previous file"
      >
        <ChevronLeft className="h-3 w-3" />
      </Button>
      <span className="text-[10px] tabular-nums text-muted-foreground min-w-[28px] text-center">
        {fileCount > 0 ? `${fileIndex + 1}/${fileCount}` : "0/0"}
      </span>
      <Button
        size="icon-xs"
        variant="ghost"
        onClick={onNextFile}
        disabled={fileCount <= 1}
        title="Next file"
      >
        <ChevronRight className="h-3 w-3" />
      </Button>

      {/* Separator */}
      <div className="w-px h-3.5 bg-border/50 mx-0.5" />

      {/* Focus mode */}
      <Button
        size="icon-xs"
        variant={tab.focusMode ? "default" : "ghost"}
        onClick={() => toggleFocusMode(tabId)}
        title="Focus mode"
      >
        <Target className="h-3 w-3" />
      </Button>

      {/* Close */}
      <Button
        size="icon-xs"
        variant="ghost"
        className="hover:bg-destructive/80"
        onClick={handleClose}
        title="Close diff"
      >
        <X className="h-3 w-3" />
      </Button>
    </div>
  );
}
