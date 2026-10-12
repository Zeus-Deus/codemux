import { useState, useEffect, useId, useMemo, useRef, type RefObject } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  ArrowLeft,
  ArrowRight,
  RotateCw,
  Loader2,
  Crosshair,
  X,
  Ellipsis,
  ExternalLink,
} from "lucide-react";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { agentBrowserRun } from "@/tauri/commands";
import { selectActiveWorkspaceId, useAppStore } from "@/stores/app-store";
import { toast } from "@/lib/toast";
import type { PortInfoSnapshot } from "@/tauri/types";
import {
  BROWSER_VIEWPORT_PRESETS,
  normalizeBrowserUrl,
  runBrowserNav,
  type BrowserNavAction,
  type BrowserViewportPresetId,
} from "./browser-nav";
import { PanelHeader } from "@/components/ui/panel-header";
import { isRemoteClient } from "@/components/remote/is-remote-client";

interface Props {
  browserId: string;
  sessionId?: string;
  currentUrl: string;
  onUrlChange: (url: string) => void;
  /** The stream itself is still coming up — there is no page to act on. */
  loading: boolean;
  inspectorActive: boolean;
  onInspectorToggle: () => void;
  /** Lets the pane focus the address bar for Ctrl+L. */
  urlInputRef?: RefObject<HTMLInputElement | null>;
  viewportPreset?: BrowserViewportPresetId;
  /** Omitted when the viewer must not resize the shared browser. */
  onViewportPresetChange?: (preset: BrowserViewportPresetId) => void;
}

const NAV_VERB: Record<BrowserNavAction, string> = {
  back: "go back",
  forward: "go forward",
  reload: "reload",
};

const EMPTY_PORTS: PortInfoSnapshot[] = [];

function canOpenExternally(url: string): boolean {
  return /^https?:\/\//i.test(url);
}

export function BrowserToolbar({
  browserId,
  sessionId,
  currentUrl,
  onUrlChange,
  loading,
  inspectorActive,
  onInspectorToggle,
  urlInputRef,
  viewportPreset = "fit",
  onViewportPresetChange,
}: Props) {
  const cmdId = sessionId ?? browserId;
  const [urlInput, setUrlInput] = useState(currentUrl);
  const [navigating, setNavigating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Bumped by each new navigation so a stale `open` that resolves late
  // cannot clear a newer spinner or overwrite the URL.
  const navTokenRef = useRef(0);
  // The navigation Stop was pressed on. Agent-browser may run commands
  // one at a time, so `window.stop()` can land after `open` finished: the
  // page still arrives then, and the address has to follow it.
  const stoppedTokenRef = useRef(-1);
  const errorId = useId();
  const portsListId = useId();

  const activeWorkspaceId = useAppStore(selectActiveWorkspaceId);
  const detectedPorts = useAppStore((s) => s.appState?.detected_ports ?? EMPTY_PORTS);
  const portSuggestions = useMemo(() => {
    const ports = new Set<number>();
    for (const p of detectedPorts) {
      if (p.workspace_id && p.workspace_id === activeWorkspaceId) ports.add(p.port);
    }
    return [...ports].sort((a, b) => a - b).map((port) => `http://localhost:${port}`);
  }, [detectedPorts, activeWorkspaceId]);

  // Sync URL bar when currentUrl prop changes (e.g., agent navigation).
  useEffect(() => {
    setUrlInput(currentUrl);
  }, [currentUrl]);

  const navigate = async (raw: string) => {
    if (!raw.trim()) return;
    const normalized = normalizeBrowserUrl(raw);
    const token = ++navTokenRef.current;
    setError(null);
    setNavigating(true);
    try {
      await agentBrowserRun(cmdId, "open", { url: normalized });
      if (token !== navTokenRef.current) return;
      onUrlChange(normalized);
      setUrlInput(normalized);
    } catch (err) {
      // An abort after Stop is what the user asked for, not a failure.
      if (token !== navTokenRef.current || token === stoppedTokenRef.current) return;
      setError(`Couldn't open ${normalized}: ${String(err)}`);
    } finally {
      if (token === navTokenRef.current) setNavigating(false);
    }
  };

  const stop = () => {
    stoppedTokenRef.current = navTokenRef.current;
    setNavigating(false);
    agentBrowserRun(cmdId, "eval", { script: "window.stop()" }).catch(() => {});
  };

  const runNav = (action: BrowserNavAction) => {
    setError(null);
    runBrowserNav(cmdId, action).catch((err) => {
      setError(`Couldn't ${NAV_VERB[action]}: ${String(err)}`);
    });
  };

  const openInSystemBrowser = () => {
    openUrl(currentUrl).catch((err) => {
      toast.error("Couldn't open the page", { description: String(err) });
    });
  };

  // A remote viewer's system browser runs on another machine, where a
  // localhost URL points somewhere else entirely.
  const canOpenInSystemBrowser = !isRemoteClient();
  const showOverflow = canOpenInSystemBrowser || !!onViewportPresetChange;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      navigate(urlInput);
    }
  };

  return (
    <>
      <PanelHeader className="gap-0.5 bg-card px-1">
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Back"
          title="Back (Alt+Left)"
          onClick={() => runNav("back")}
        >
          <ArrowLeft className="size-3" />
        </Button>
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Forward"
          title="Forward (Alt+Right)"
          onClick={() => runNav("forward")}
        >
          <ArrowRight className="size-3" />
        </Button>
        {navigating ? (
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label="Stop"
            title="Stop loading"
            onClick={stop}
          >
            <X className="size-3" />
          </Button>
        ) : (
          <Button
            variant="ghost"
            size="icon-xs"
            aria-label={loading ? "Connecting" : "Refresh"}
            title={loading ? "Connecting to the browser" : "Refresh (Ctrl+R)"}
            disabled={loading}
            onClick={() => runNav("reload")}
          >
            {loading ? (
              <Loader2 className="size-3 motion-safe:animate-spin" />
            ) : (
              <RotateCw className="size-3" />
            )}
          </Button>
        )}
        <Button
          variant="ghost"
          size="icon-xs"
          aria-label="Element Inspector"
          title="Element Inspector (Ctrl+Shift+I)"
          className={inspectorActive ? "bg-primary/20 text-primary" : ""}
          onClick={onInspectorToggle}
        >
          <Crosshair className="size-3" />
        </Button>
        <Input
          ref={urlInputRef}
          value={urlInput}
          onChange={(e) => {
            setUrlInput(e.target.value);
            if (error) setError(null);
          }}
          onKeyDown={handleKeyDown}
          onFocus={(e) => e.target.select()}
          placeholder="Search or enter address"
          aria-label="Address"
          aria-invalid={error ? true : undefined}
          aria-describedby={error ? errorId : undefined}
          list={portSuggestions.length > 0 ? portsListId : undefined}
          className="h-6 flex-1 text-label bg-background border-none px-2 aria-invalid:ring-1 [&::-webkit-calendar-picker-indicator]:hidden"
        />
        {portSuggestions.length > 0 && (
          <datalist id={portsListId}>
            {portSuggestions.map((url) => (
              <option key={url} value={url} />
            ))}
          </datalist>
        )}
        {showOverflow && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="icon-xs" aria-label="More browser actions">
                <Ellipsis className="size-3" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-56">
              {canOpenInSystemBrowser && (
                <DropdownMenuItem
                  disabled={!canOpenExternally(currentUrl)}
                  onSelect={openInSystemBrowser}
                >
                  <ExternalLink className="size-3.5" />
                  Open in system browser
                </DropdownMenuItem>
              )}
              {onViewportPresetChange && (
                <>
                  {canOpenInSystemBrowser && <DropdownMenuSeparator />}
                  <DropdownMenuLabel>Viewport</DropdownMenuLabel>
                  <DropdownMenuRadioGroup
                    value={viewportPreset}
                    onValueChange={(value) => {
                      const preset = BROWSER_VIEWPORT_PRESETS.find((p) => p.id === value);
                      if (preset) onViewportPresetChange(preset.id);
                    }}
                  >
                    {BROWSER_VIEWPORT_PRESETS.map((preset) => (
                      <DropdownMenuRadioItem key={preset.id} value={preset.id}>
                        <span className="flex-1">{preset.label}</span>
                        {preset.size && (
                          <span className="text-label tabular-nums text-muted-foreground">
                            {preset.size.width}×{preset.size.height}
                          </span>
                        )}
                      </DropdownMenuRadioItem>
                    ))}
                  </DropdownMenuRadioGroup>
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </PanelHeader>
      {error && (
        <p
          id={errorId}
          role="alert"
          title={error}
          className="shrink-0 truncate border-b border-hairline bg-card px-2.5 py-1 text-label text-destructive"
        >
          {error}
        </p>
      )}
    </>
  );
}
