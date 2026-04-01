import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ArrowLeft, ArrowRight, RotateCw, Loader2, Crosshair } from "lucide-react";
import { agentBrowserRun } from "@/tauri/commands";

interface Props {
  browserId: string;
  sessionId?: string;
  currentUrl: string;
  onUrlChange: (url: string) => void;
  loading: boolean;
  inspectorActive: boolean;
  onInspectorToggle: () => void;
}

export function BrowserToolbar({ browserId, sessionId, currentUrl, onUrlChange, loading, inspectorActive, onInspectorToggle }: Props) {
  const cmdId = sessionId ?? browserId;
  const [urlInput, setUrlInput] = useState(currentUrl);
  const [navigating, setNavigating] = useState(false);

  const navigate = async (url: string) => {
    const normalized = url.startsWith("http") ? url : `https://${url}`;
    setNavigating(true);
    try {
      await agentBrowserRun(cmdId, "open", { url: normalized });
      onUrlChange(normalized);
      setUrlInput(normalized);
    } catch (err) {
      console.error("[BrowserToolbar] Navigation failed:", err);
    } finally {
      setNavigating(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      e.preventDefault();
      navigate(urlInput);
    }
  };

  return (
    <div className="flex h-7 shrink-0 items-center gap-0.5 border-b border-border/50 bg-card px-1">
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Back"
        onClick={() => agentBrowserRun(cmdId, "back", {}).catch(console.error)}
      >
        <ArrowLeft className="h-3 w-3" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Forward"
        onClick={() => agentBrowserRun(cmdId, "forward", {}).catch(console.error)}
      >
        <ArrowRight className="h-3 w-3" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Refresh"
        onClick={() => agentBrowserRun(cmdId, "reload", {}).catch(console.error)}
      >
        {navigating || loading ? (
          <Loader2 className="h-3 w-3 animate-spin" />
        ) : (
          <RotateCw className="h-3 w-3" />
        )}
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Element Inspector"
        title="Element Inspector (Ctrl+Shift+I)"
        className={inspectorActive ? "bg-primary/20 text-primary" : ""}
        onClick={onInspectorToggle}
      >
        <Crosshair className="h-3 w-3" />
      </Button>
      <Input
        value={urlInput}
        onChange={(e) => setUrlInput(e.target.value)}
        onKeyDown={handleKeyDown}
        onFocus={(e) => e.target.select()}
        placeholder="Enter URL..."
        className="h-6 flex-1 text-xs bg-background border-none px-2"
      />
    </div>
  );
}
