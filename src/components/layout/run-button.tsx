import { useState, useEffect, useRef } from "react";
import { ChevronDown, Play, Settings } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { useActiveWorkspaceProjectRoot } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import { cn } from "@/lib/utils";
import {
  getProjectScripts,
  getWorkspaceConfig,
  runProjectDevCommand,
} from "@/tauri/commands";

interface RunButtonProps {
  workspaceId: string;
  /** Rendering shape. `"legacy"` (default) is the original ghost
   *  [Run/Set Run + badge] button + standalone gear pair — used by the
   *  legacy (flag-off) PresetBar, which must stay byte-identical per the
   *  gui-chrome contract (docs/features/gui-chrome.md). `"split"` is the
   *  GUI-chrome split button: main segment runs (green play glyph), caret
   *  segment configures, no standalone gear. Only the GUI branch of
   *  `title-bar.tsx` passes `"split"`. */
  variant?: "legacy" | "split";
}

export function RunButton({ workspaceId, variant = "legacy" }: RunButtonProps) {
  const [runCommand, setRunCommand] = useState<string | null>(null);
  // Subscribe to the primitive project_root, not the whole workspace
  // object — full-snapshot rebuilds on every backend tick churn the
  // workspace ref and would re-render this button on every tick.
  const projectRoot = useActiveWorkspaceProjectRoot();
  const showSettings = useUIStore((s) => s.showSettings);
  const prevShowSettings = useRef(showSettings);

  useEffect(() => {
    // Re-fetch when settings closes (user may have edited the run command)
    const settingsJustClosed = prevShowSettings.current && !showSettings;
    prevShowSettings.current = showSettings;
    if (showSettings) return; // Don't fetch while settings is open

    if (!projectRoot) {
      setRunCommand(null);
      return;
    }

    // Only fetch on mount or when settings closes
    if (!settingsJustClosed && runCommand !== null) return;

    let cancelled = false;
    Promise.all([
      getWorkspaceConfig(projectRoot).catch(() => null),
      getProjectScripts(projectRoot).catch(() => null),
    ]).then(([fileConfig, dbScripts]) => {
      if (cancelled) return;
      // File config takes precedence, matching backend resolution
      const cmd = fileConfig?.run ?? dbScripts?.run ?? null;
      setRunCommand(cmd && cmd.trim() ? cmd.trim() : null);
    });

    return () => { cancelled = true; };
  }, [projectRoot, showSettings]);

  const setShowSettings = useUIStore.getState().setShowSettings;

  const handleRun = () => {
    runProjectDevCommand(workspaceId).catch(console.error);
  };

  const handleConfigure = () => {
    setShowSettings(true, "projects");
  };

  const shortcutBadge = (
    <kbd className="ml-1 text-[10px] leading-none bg-muted px-1 py-0.5 rounded border border-border text-muted-foreground font-sans">
      Ctrl+Shift+G
    </kbd>
  );

  const isConfigured = !!runCommand;

  if (variant === "split") {
    // GUI-chrome split button: the main segment runs (or opens configure
    // when nothing's set yet, same as before); the caret segment always
    // opens configure. The old standalone gear affordance is gone here —
    // configure lives only behind the caret, matching the IDE launcher's
    // [icon][caret] shape.
    return (
      <div className="flex items-center shrink-0">
        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={isConfigured ? handleRun : handleConfigure}
              className={cn(
                "flex items-center gap-1 h-6 px-2 rounded-l-md border border-r-0 border-border/60 bg-secondary/50 text-xs font-medium",
                "transition-colors duration-150",
                "hover:bg-secondary hover:border-border",
                !isConfigured && "text-muted-foreground",
              )}
            >
              <Play className="size-3.5 shrink-0 text-status-open" fill="currentColor" />
              <span>{isConfigured ? "Run" : "Set Run"}</span>
              {shortcutBadge}
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4}>
            {isConfigured ? runCommand : "Configure run command"}
          </TooltipContent>
        </Tooltip>

        <Tooltip>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={handleConfigure}
              aria-label="Configure run command"
              className={cn(
                "flex items-center justify-center h-6 w-5 rounded-r-md border border-border/60 bg-secondary/50 text-muted-foreground",
                "transition-colors duration-150",
                "hover:bg-secondary hover:border-border hover:text-foreground",
              )}
            >
              <ChevronDown className="h-3 w-3" />
            </button>
          </TooltipTrigger>
          <TooltipContent side="bottom" sideOffset={4}>
            Configure run command
          </TooltipContent>
        </Tooltip>
      </div>
    );
  }

  return (
    <div className="flex items-center shrink-0 gap-0.5">
      {/* Run button — primary action */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button
            variant="ghost"
            size="xs"
            className={`gap-1 ${!isConfigured ? "text-muted-foreground" : ""}`}
            onClick={isConfigured ? handleRun : handleConfigure}
          >
            <Play className="size-3.5" />
            <span>{isConfigured ? "Run" : "Set Run"}</span>
            {shortcutBadge}
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {isConfigured ? runCommand : "Configure run command"}
        </TooltipContent>
      </Tooltip>

      {/* Gear button — opens settings, always available */}
      <Tooltip>
        <TooltipTrigger asChild>
          <Button variant="ghost" size="icon-xs" onClick={handleConfigure}>
            <Settings className="size-4" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {isConfigured ? "Edit run command" : "Configure run command"}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}
