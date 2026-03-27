import { useState, useEffect } from "react";
import { Settings } from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuCheckboxItem,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { Separator } from "@/components/ui/separator";
import { Button } from "@/components/ui/button";
import { PresetIcon } from "@/components/icons/preset-icon";
import { useUIStore } from "@/stores/ui-store";
import {
  getPresets,
  applyPreset,
  setPresetBarVisible,
} from "@/tauri/commands";
import { onPresetsChanged } from "@/tauri/events";
import type { PresetStoreSnapshot, TerminalPreset } from "@/tauri/types";

interface PresetBarProps {
  workspaceId: string;
}

export function PresetBar({ workspaceId }: PresetBarProps) {
  const [presetStore, setPresetStore] = useState<PresetStoreSnapshot | null>(null);

  useEffect(() => {
    getPresets().then((s) => setPresetStore(s)).catch(console.error);
    const unlisten = onPresetsChanged((snapshot) => setPresetStore(snapshot));
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  if (!presetStore || !presetStore.bar_visible) return null;

  const pinnedPresets = presetStore.presets.filter((p) => p.pinned);

  const handleLaunch = (preset: TerminalPreset) => {
    applyPreset(workspaceId, preset.id, preset.launch_mode).catch(
      console.error,
    );
  };

  const handleToggleBar = (checked: boolean) => {
    setPresetBarVisible(checked).catch(console.error);
  };

  const setShowSettings = useUIStore.getState().setShowSettings;

  return (
    <div
      className="flex items-center h-8 border-b border-border bg-background px-2 gap-0.5 shrink-0 overflow-x-auto"
      style={{ scrollbarWidth: "none" }}
    >
      {/* Settings gear */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Preset settings"
          >
            <Settings className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-52">
          <DropdownMenuCheckboxItem
            checked={presetStore.bar_visible}
            onCheckedChange={handleToggleBar}
          >
            Show Preset Bar
          </DropdownMenuCheckboxItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setShowSettings(true, "presets")}>
            <Settings className="h-4 w-4" />
            <span>Manage Presets</span>
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      {/* Divider */}
      <Separator orientation="vertical" className="!h-4 !self-auto mx-0.5" />

      {/* Preset buttons */}
      {pinnedPresets.map((preset) => (
        <Tooltip key={preset.id}>
          <TooltipTrigger asChild>
            <Button
              variant="ghost"
              size="xs"
              className="gap-1.5 shrink-0"
              onClick={() => handleLaunch(preset)}
            >
              <PresetIcon icon={preset.icon} className="h-3.5 w-3.5" />
              <span className="truncate max-w-[120px]">{preset.name}</span>
            </Button>
          </TooltipTrigger>
          {preset.description && (
            <TooltipContent side="bottom" sideOffset={4}>
              {preset.description}
            </TooltipContent>
          )}
        </Tooltip>
      ))}
    </div>
  );
}
