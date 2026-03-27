import { useState, useEffect, useRef } from "react";
import { Play, Settings } from "lucide-react";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Button } from "@/components/ui/button";
import { useActiveWorkspace } from "@/stores/app-store";
import { useUIStore } from "@/stores/ui-store";
import {
  getProjectScripts,
  getWorkspaceConfig,
  runProjectDevCommand,
} from "@/tauri/commands";

interface RunButtonProps {
  workspaceId: string;
}

export function RunButton({ workspaceId }: RunButtonProps) {
  const [runCommand, setRunCommand] = useState<string | null>(null);
  const workspace = useActiveWorkspace();
  const showSettings = useUIStore((s) => s.showSettings);
  const prevShowSettings = useRef(showSettings);
  const projectRoot = workspace?.project_root ?? null;

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
            <Play className="h-3 w-3" />
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
            <Settings className="h-3 w-3" />
          </Button>
        </TooltipTrigger>
        <TooltipContent side="bottom" sideOffset={4}>
          {isConfigured ? "Edit run command" : "Configure run command"}
        </TooltipContent>
      </Tooltip>
    </div>
  );
}
