import { Bug } from "lucide-react";

import { Button } from "@/components/ui/button";

interface Props {
  /** Click triggers the cleanup turn (synthetic user prompt that
   *  asks Claude to grep + remove all CODEMUX_DEBUG lines). */
  onCleanup: () => void;
  /** When true the button shows a spinner-style label and is
   *  disabled. The parent flips this while the cleanup turn is
   *  in-flight so a double-click can't queue two cleanup runs. */
  busy?: boolean;
}

/** Banner rendered above the composer when the slice is in Debug
 *  mode AND the background grep has confirmed CODEMUX_DEBUG markers
 *  exist in the project. The Clean up button fires the synthetic
 *  cleanup turn — see `triggerDebugCleanup` in AgentChatPane. */
export function DebugCleanupBanner({ onCleanup, busy = false }: Props) {
  return (
    <div className="px-4 pb-2">
      <div className="mx-auto flex w-full max-w-2xl items-center gap-2 rounded-md border border-danger/20 bg-danger/10 px-3 py-2 text-xs">
        <Bug className="size-3.5 text-danger" aria-hidden />
        <span>Debug markers detected in this project.</span>
        <Button
          size="sm"
          variant="outline"
          className="ml-auto h-6 text-xs"
          onClick={onCleanup}
          disabled={busy}
        >
          {busy ? "Cleaning…" : "Clean up"}
        </Button>
      </div>
    </div>
  );
}
