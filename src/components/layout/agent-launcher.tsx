import { useRef, useState } from "react";
import { Pin, PinOff, Plus, Terminal, Globe, Settings, ExternalLink } from "lucide-react";

import {
  Command,
  CommandEmpty,
  CommandGroup,
  CommandInput,
  CommandItem,
  CommandList,
  CommandSeparator,
} from "@/components/ui/command";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { PresetIcon } from "@/components/icons/preset-icon";
import { usePresetStore } from "@/hooks/use-preset-store";
import { launchDraftWithPreset } from "@/lib/agent-chat/draft-preset-launch";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import type { ChatDraft } from "@/stores/chat-draft-store";
import { useTitlebarPinsStore } from "@/stores/titlebar-pins-store";
import { useUIStore } from "@/stores/ui-store";
import {
  agentChatCreatePane,
  applyPreset,
  createBrowserPane,
  createTab,
} from "@/tauri/commands";
import type { TerminalPreset, WorkspaceSnapshot } from "@/tauri/types";

/**
 * Hover-revealed pin toggle rendered inside a GUI/CLI preset row in the
 * launcher. Opts a preset id into `useTitlebarPinsStore` — the separate,
 * user-controlled set that drives `PinnedPresetTiles` in title-bar.tsx
 * (default empty; unrelated to `preset.pinned`, which only affects the
 * legacy PresetBar). Must never launch the row: every pointer handler
 * stops propagation so cmdk's `CommandItem` never sees the click and
 * fires `onSelect`.
 */
function TitlebarPinToggle({ presetId }: { presetId: string }) {
  const pinned = useTitlebarPinsStore((s) => s.pinnedIds.includes(presetId));

  const stop = (e: React.SyntheticEvent) => {
    e.stopPropagation();
  };

  return (
    <button
      type="button"
      aria-label={pinned ? "Unpin from title bar" : "Pin to title bar"}
      title={pinned ? "Unpin from title bar" : "Pin to title bar"}
      data-testid={`launcher-pin-toggle-${presetId}`}
      onPointerDown={stop}
      onMouseDown={stop}
      onClick={(e) => {
        e.stopPropagation();
        e.preventDefault();
        useTitlebarPinsStore.getState().toggleTitlebarPin(presetId);
      }}
      className={cn(
        "ml-1 flex h-5 w-5 shrink-0 items-center justify-center rounded transition-colors",
        pinned
          ? "text-accent-ember opacity-100 hover:text-accent-ember/80"
          : "text-muted-foreground opacity-0 group-hover/command-item:opacity-100 hover:text-foreground",
      )}
    >
      {pinned ? (
        <PinOff className="size-3.5" />
      ) : (
        <Pin className="size-3.5" />
      )}
    </button>
  );
}

interface AgentLauncherProps {
  workspace: WorkspaceSnapshot;
}

/** Extract a human error string from a Tauri reject (string | Error). */
function errorMessage(err: unknown): string {
  return typeof err === "string"
    ? err
    : err instanceof Error
      ? err.message
      : String(err);
}

/**
 * The `+` launcher popover in GUI chrome. Replaces the always-present
 * preset strip: every agent still launches from here (GUI chat presets +
 * CLI agents), plus Terminal / Browser panes and a "Manage presets"
 * shortcut. Preset data comes from the live preset store snapshot.
 */
export function AgentLauncher({ workspace }: AgentLauncherProps) {
  const [open, setOpen] = useState(false);
  const presetStore = usePresetStore();
  // Track whether Shift was held when a CLI row was chosen so keyboard
  // and mouse both reach the split path (cmdk's onSelect drops the event).
  // Kept in sync from the live pointer/key event right before onSelect and
  // reset whenever the popover toggles, so an aborted mousedown (press then
  // release off the row, no select) can't leak a stale `true` into a later
  // keyboard selection.
  const shiftHeld = useRef(false);

  const handleOpenChange = (next: boolean) => {
    shiftHeld.current = false;
    setOpen(next);
  };

  const workspaceId = workspace.workspace_id;

  const presets = presetStore?.presets ?? [];
  // GUI section — native chat presets (spawn an agent_chat pane).
  const chatPresets = presets.filter((p) => p.kind === "chat_agent");
  // CLI section — pinned first, then unpinned, matching the preset bar's
  // left-to-right priority.
  const cliPresets = presets
    .filter((p) => p.kind === "cli")
    .sort((a, b) => Number(b.pinned) - Number(a.pinned));

  const launchChat = () => {
    setOpen(false);
    agentChatCreatePane(workspaceId, "claude", null, "new_tab").catch((err) => {
      toast.error(`Chat Agent: ${errorMessage(err)}`);
      console.error("[agent-launcher] chat launch failed:", err);
    });
  };

  const launchCli = (preset: TerminalPreset) => {
    const split = shiftHeld.current;
    shiftHeld.current = false;
    setOpen(false);
    const modelSelection = preset.launch_config?.model_selection ?? null;
    applyPreset(
      workspaceId,
      preset.id,
      split ? "split_pane" : "new_tab",
      null,
      modelSelection,
    ).catch((err) => {
      toast.error(`${preset.name}: ${errorMessage(err)}`);
      console.error("[agent-launcher] applyPreset failed:", err);
    });
  };

  const createTerminal = () => {
    setOpen(false);
    createTab(workspaceId, "terminal").catch(console.error);
  };

  const createBrowser = () => {
    setOpen(false);
    const surface = workspace.surfaces.find(
      (s) => s.surface_id === workspace.active_surface_id,
    );
    if (surface) createBrowserPane(surface.active_pane_id).catch(console.error);
  };

  const managePresets = () => {
    setOpen(false);
    useUIStore.getState().setShowSettings(true, "presets");
  };

  return (
    <Popover open={open} onOpenChange={handleOpenChange}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Launch an agent"
          data-testid="agent-launcher-trigger"
          className={cn(
            "flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-colors",
            open
              ? "bg-accent text-foreground"
              : "text-muted-foreground hover:bg-accent hover:text-foreground",
          )}
        >
          <Plus className="h-4 w-4" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        sideOffset={6}
        className="w-80 overflow-hidden p-0"
        data-testid="agent-launcher-popover"
      >
        <Command>
          <CommandInput
            placeholder="Launch an agent…"
            // cmdk fires onSelect without the DOM event, so mirror the live
            // Shift state here: on Enter this handler runs (bubble target)
            // before cmdk's root keydown triggers onSelect, and it clears any
            // Shift left over from an aborted mousedown so a keyboard select
            // never inherits it.
            onKeyDown={(e) => {
              shiftHeld.current = e.shiftKey;
            }}
          />
          <CommandList className="max-h-80">
            <CommandEmpty>No matches.</CommandEmpty>
            {chatPresets.length > 0 && (
              <CommandGroup heading="GUI">
                {chatPresets.map((preset) => (
                  <CommandItem
                    key={preset.id}
                    value={`gui ${preset.name}`}
                    onSelect={launchChat}
                    data-testid={`launcher-gui-${preset.id}`}
                  >
                    <PresetIcon icon={preset.icon} className="h-4 w-4" />
                    <span className="flex-1 truncate">{preset.name}</span>
                    {preset.pinned && (
                      <span className="text-[9.5px] font-bold tracking-wide text-muted-foreground">
                        PINNED
                      </span>
                    )}
                    <TitlebarPinToggle presetId={preset.id} />
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            {cliPresets.length > 0 && (
              <CommandGroup heading="CLI agents">
                {cliPresets.map((preset) => (
                  <CommandItem
                    key={preset.id}
                    value={`cli ${preset.name}`}
                    onMouseDown={(e) => {
                      shiftHeld.current = e.shiftKey;
                    }}
                    onSelect={() => launchCli(preset)}
                    data-testid={`launcher-cli-${preset.id}`}
                  >
                    <PresetIcon icon={preset.icon} className="h-4 w-4" />
                    <span className="flex-1 truncate">{preset.name}</span>
                    <span className="font-mono text-[10px] text-muted-foreground/70">
                      ↗ terminal
                    </span>
                    <TitlebarPinToggle presetId={preset.id} />
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            <CommandGroup heading="Panes">
              <CommandItem value="pane terminal" onSelect={createTerminal}>
                <Terminal className="h-4 w-4" />
                <span className="flex-1">Terminal</span>
              </CommandItem>
              <CommandItem value="pane browser" onSelect={createBrowser}>
                <Globe className="h-4 w-4" />
                <span className="flex-1">Browser</span>
              </CommandItem>
            </CommandGroup>
            <CommandSeparator />
            <CommandGroup>
              <CommandItem value="manage presets" onSelect={managePresets}>
                <Settings className="h-4 w-4" />
                <span className="flex-1">Manage presets…</span>
                <ExternalLink className="h-3 w-3 text-muted-foreground/70" />
              </CommandItem>
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}

interface DraftAgentLauncherProps {
  draft: ChatDraft;
}

/**
 * The `+` launcher for the GUI-chrome DRAFT titlebar variant (see
 * `useDraftGuiChrome` in src/hooks/use-gui-chrome.ts). Replaces the
 * legacy draft `PresetBar` row: picking a preset here materialises the
 * draft via `launchDraftWithPreset` — committing the composed prompt
 * (possibly empty) into a fresh workspace spawning only that preset's
 * pane — instead of launching onto an existing workspace.
 *
 * Mirrors the legacy draft PresetBar's rules: hidden on a Home-target
 * draft (CLI presets need project context, and a Chat Agent click
 * would duplicate the chat being composed — picking a project via
 * Thread Scope makes it appear), and disabled while a materialise is
 * in flight so presets can't be swapped mid-flight. No Panes section
 * — there is no live surface to split or add tabs to yet.
 */
export function DraftAgentLauncher({ draft }: DraftAgentLauncherProps) {
  const [open, setOpen] = useState(false);
  const presetStore = usePresetStore();

  const presets = presetStore?.presets ?? [];
  const chatPresets = presets.filter((p) => p.kind === "chat_agent");
  const cliPresets = presets
    .filter((p) => p.kind === "cli")
    .sort((a, b) => Number(b.pinned) - Number(a.pinned));

  // Same gate as the legacy draft PresetBar (`isHomeDraft return null`).
  if (draft.target.kind === "home") return null;

  const launch = (preset: TerminalPreset) => {
    setOpen(false);
    void launchDraftWithPreset(draft.draftId, preset).then((result) => {
      if (!result.success) {
        toast.error(`${preset.name}: ${result.error ?? "Send failed"}`);
      }
    });
  };

  const managePresets = () => {
    setOpen(false);
    useUIStore.getState().setShowSettings(true, "presets");
  };

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Launch an agent"
          data-testid="draft-agent-launcher-trigger"
          disabled={draft.promoting}
          className={cn(
            "flex h-7 w-7 shrink-0 items-center justify-center rounded-md transition-colors",
            open
              ? "bg-accent text-foreground"
              : "text-muted-foreground hover:bg-accent hover:text-foreground",
            draft.promoting && "opacity-40 pointer-events-none",
          )}
        >
          <Plus className="h-4 w-4" />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        sideOffset={6}
        className="w-80 overflow-hidden p-0"
        data-testid="draft-agent-launcher-popover"
      >
        <Command>
          <CommandInput placeholder="Start with an agent…" />
          <CommandList className="max-h-80">
            <CommandEmpty>No matches.</CommandEmpty>
            {chatPresets.length > 0 && (
              <CommandGroup heading="GUI">
                {chatPresets.map((preset) => (
                  <CommandItem
                    key={preset.id}
                    value={`gui ${preset.name}`}
                    onSelect={() => launch(preset)}
                    data-testid={`draft-launcher-gui-${preset.id}`}
                  >
                    <PresetIcon icon={preset.icon} className="h-4 w-4" />
                    <span className="flex-1 truncate">{preset.name}</span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            {cliPresets.length > 0 && (
              <CommandGroup heading="CLI agents">
                {cliPresets.map((preset) => (
                  <CommandItem
                    key={preset.id}
                    value={`cli ${preset.name}`}
                    onSelect={() => launch(preset)}
                    data-testid={`draft-launcher-cli-${preset.id}`}
                  >
                    <PresetIcon icon={preset.icon} className="h-4 w-4" />
                    <span className="flex-1 truncate">{preset.name}</span>
                    <span className="font-mono text-[10px] text-muted-foreground/70">
                      ↗ terminal
                    </span>
                  </CommandItem>
                ))}
              </CommandGroup>
            )}
            <CommandSeparator />
            <CommandGroup>
              <CommandItem value="manage presets" onSelect={managePresets}>
                <Settings className="h-4 w-4" />
                <span className="flex-1">Manage presets…</span>
                <ExternalLink className="h-3 w-3 text-muted-foreground/70" />
              </CommandItem>
            </CommandGroup>
          </CommandList>
        </Command>
      </PopoverContent>
    </Popover>
  );
}
