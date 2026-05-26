import { useState } from "react";

import { ArrowUpRight } from "lucide-react";

import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import type { HostView } from "@/tauri/commands";

/**
 * Confirmation dialog shown before a "Push workspace to device" action
 * actually fires. The push itself is destructive-ish (moves the live
 * editing location, alters runtime state, kicks off SSH + rsync), so
 * giving the user a brief "this will…" preview is the Phase-4 data-
 * safety guardrail.
 *
 * Power-user escape hatch: the "Don't ask again for X" checkbox sets
 * a per-host localStorage flag (`codemux.push.dontAskAgain.<hostId>`)
 * that the caller checks before opening this dialog. So the
 * confirmation step can be bypassed permanently per device once
 * trust is established.
 */

const DONT_ASK_AGAIN_KEY_PREFIX = "codemux.push.dontAskAgain.";

export function dontAskAgainKey(hostId: number): string {
  return `${DONT_ASK_AGAIN_KEY_PREFIX}${hostId}`;
}

/** Synchronous check the caller uses BEFORE opening the dialog so
 *  the right-click → push → submenu pick stays a single tap for
 *  users who flipped the toggle. */
export function shouldSkipPushConfirm(hostId: number): boolean {
  try {
    return localStorage.getItem(dontAskAgainKey(hostId)) === "1";
  } catch {
    return false;
  }
}

interface Props {
  open: boolean;
  workspaceTitle: string;
  host: HostView | null;
  onConfirm: () => void;
  onOpenChange: (open: boolean) => void;
}

export function ConfirmPushDialog({
  open,
  workspaceTitle,
  host,
  onConfirm,
  onOpenChange,
}: Props) {
  const [dontAskAgain, setDontAskAgain] = useState(false);

  if (!host) return null;

  const handleConfirm = () => {
    if (dontAskAgain) {
      try {
        localStorage.setItem(dontAskAgainKey(host.id), "1");
      } catch {
        // localStorage disabled — confirmation will keep prompting,
        // which is the safe default.
      }
    }
    onConfirm();
    onOpenChange(false);
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        showCloseButton={false}
        className="sm:max-w-[420px] bg-popover p-0 gap-0 overflow-hidden"
      >
        <DialogHeader className="px-5 pt-4 pb-2">
          <DialogTitle className="text-[14px] font-semibold">
            Push to {host.name}?
          </DialogTitle>
          <DialogDescription className="text-[12px] text-muted-foreground/80">
            Send{" "}
            <span className="font-medium text-foreground">{workspaceTitle}</span>{" "}
            to {host.name}.
          </DialogDescription>
        </DialogHeader>

        <div className="px-5 pb-4 space-y-3">
          <ul className="rounded-md border border-border/60 bg-muted/30 px-3 py-2 text-[11.5px] text-muted-foreground/85 leading-relaxed space-y-1">
            <li>
              • Copies the workspace files to{" "}
              <span className="font-medium text-foreground/90">
                {host.name}
              </span>{" "}
              via rsync.
            </li>
            <li>
              • The live editing location moves to {host.name}. Your
              local copy stays in place but goes idle.
            </li>
            <li>
              • You can pull it back anytime from the workspace menu
              (or with Undo, for 10 seconds).
            </li>
          </ul>

          <label className="flex items-center gap-2 cursor-pointer select-none">
            <input
              type="checkbox"
              checked={dontAskAgain}
              onChange={(e) => setDontAskAgain(e.target.checked)}
              className="rounded border-border"
            />
            <span className="text-[11.5px] text-muted-foreground">
              Don't ask again for {host.name}
            </span>
          </label>

          <div className="flex justify-end gap-2 pt-1 border-t border-border/40">
            <Button
              variant="ghost"
              size="sm"
              className="h-7 px-3 text-[12px]"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button
              variant="secondary"
              size="sm"
              className="h-7 gap-1.5 px-3 text-[12px]"
              onClick={handleConfirm}
            >
              <ArrowUpRight className="size-3" />
              Push
            </Button>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
