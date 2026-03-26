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
          <button
            type="button"
            aria-label="Preset settings"
            className="flex h-6 w-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
          >
            <Settings className="h-3.5 w-3.5" />
          </button>
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
      <div className="h-4 w-px bg-border mx-0.5 shrink-0" />

      {/* Preset buttons */}
      {pinnedPresets.map((preset) => (
        <Tooltip key={preset.id}>
          <TooltipTrigger asChild>
            <button
              type="button"
              onClick={() => handleLaunch(preset)}
              className="flex items-center gap-1.5 h-6 px-2 shrink-0 rounded-md text-xs text-muted-foreground transition-colors duration-150 hover:bg-accent hover:text-foreground"
            >
              <PresetIcon icon={preset.icon} className="h-3.5 w-3.5" />
              <span className="truncate max-w-[120px]">{preset.name}</span>
            </button>
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
