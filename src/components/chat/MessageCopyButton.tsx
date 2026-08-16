import { useCallback, useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";

import { copyToClipboard } from "@/lib/clipboard";
import { cn } from "@/lib/utils";

import { MESSAGE_ACTION_CLASS } from "./message-action";

/** How long the checkmark stays up before reverting to the copy glyph. */
const COPIED_RESET_MS = 1200;

/**
 * The transcript's per-message copy action: a 10px icon-and-label chip in the
 * footer strip under a message, flipping to "Copied" for a beat to confirm.
 * Reveal behaviour is shared with the Revert action it sits beside — see
 * `MESSAGE_ACTION_CLASS`.
 *
 * It copies the message's *raw* text — the original prompt, or the assistant's
 * markdown source rather than its rendered DOM — so pasting elsewhere keeps
 * the formatting.
 */
export function MessageCopyButton({
  text,
  label = "Copy message",
  className,
}: {
  /** Raw text placed on the clipboard. */
  text: string;
  /** Accessible name / tooltip shown before the copy succeeds. */
  label?: string;
  /** Extra classes from the owning message (alignment, spacing). */
  className?: string;
}) {
  const [copied, setCopied] = useState(false);
  const resetTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    },
    [],
  );

  const handleCopy = useCallback(async () => {
    // Ignore repeat clicks while the check is up: re-copying identical text is
    // a no-op, and restarting the timer makes the confirmation feel stuck.
    if (!text || copied) return;
    if (!(await copyToClipboard(text))) return;
    setCopied(true);
    if (resetTimer.current !== null) window.clearTimeout(resetTimer.current);
    resetTimer.current = window.setTimeout(() => {
      setCopied(false);
      resetTimer.current = null;
    }, COPIED_RESET_MS);
  }, [text, copied]);

  return (
    <button
      type="button"
      aria-label={copied ? "Copied" : label}
      title={label}
      data-testid="message-copy-button"
      onClick={() => void handleCopy()}
      className={cn(
        MESSAGE_ACTION_CLASS,
        // Hold the confirmation visible even if the pointer has already left
        // the message, so a copy made on the way out still reads as confirmed.
        copied && "pointer-events-auto text-status-open opacity-100",
        className,
      )}
    >
      {copied ? (
        <Check className="h-3 w-3" aria-hidden />
      ) : (
        <Copy className="h-3 w-3" aria-hidden />
      )}
      {copied ? "Copied" : "Copy"}
    </button>
  );
}
