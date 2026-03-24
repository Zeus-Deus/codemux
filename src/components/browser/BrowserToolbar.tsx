import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ArrowLeft, ArrowRight, RotateCw, Loader2 } from "lucide-react";
import { agentBrowserRun } from "@/tauri/commands";

interface Props {
  browserId: string;
  currentUrl: string;
  onUrlChange: (url: string) => void;
  loading: boolean;
}

export function BrowserToolbar({ browserId, currentUrl, onUrlChange, loading }: Props) {
  const [urlInput, setUrlInput] = useState(currentUrl);
  const [navigating, setNavigating] = useState(false);

  const navigate = async (url: string) => {
    const normalized = url.startsWith("http") ? url : `https://${url}`;
    setNavigating(true);
    try {
      await agentBrowserRun(browserId, "open", { url: normalized });
      onUrlChange(normalized);
      setUrlInput(normalized);
    } catch (err) {
      console.error("Navigation failed:", err);
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
    <div className="flex h-8 shrink-0 items-center gap-1 border-b border-border bg-card px-1.5">
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Back"
        onClick={() => agentBrowserRun(browserId, "back", {}).catch(console.error)}
      >
        <ArrowLeft className="h-3 w-3" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Forward"
        onClick={() => agentBrowserRun(browserId, "forward", {}).catch(console.error)}
      >
        <ArrowRight className="h-3 w-3" />
      </Button>
      <Button
        variant="ghost"
        size="icon-xs"
        aria-label="Refresh"
        onClick={() => agentBrowserRun(browserId, "reload", {}).catch(console.error)}
      >
        {navigating || loading ? (
          <Loader2 className="h-3 w-3 animate-spin" />
        ) : (
          <RotateCw className="h-3 w-3" />
        )}
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
