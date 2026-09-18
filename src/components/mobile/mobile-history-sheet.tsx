import { useEffect, useRef, useState, type RefObject } from "react";
import { Search } from "lucide-react";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "@/components/ui/sheet";
import { toast } from "@/lib/toast";
import type { TrailEntry } from "@/components/chat/message-trail";
import {
  visibleMobileHistory,
  type MobileHistorySource,
} from "./mobile-history";

/** Mounted only on request, keeping large history lists out of the chat render path. */
export function MobileHistorySheet({
  workspaceId,
  onClose,
  returnFocusRef,
}: {
  workspaceId: string;
  onClose: () => void;
  returnFocusRef: RefObject<HTMLButtonElement | null>;
}) {
  const [source, setSource] = useState<MobileHistorySource>();
  const [entries, setEntries] = useState<TrailEntry[]>([]);
  const [query, setQuery] = useState("");
  const [limit, setLimit] = useState(50);
  const [busy, setBusy] = useState(false);
  const title = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    // Wait until the Tools sheet/panel is closed and the conversation is visible.
    const active = visibleMobileHistory(workspaceId);
    setSource(active);
    setEntries(active?.entries().slice().reverse() ?? []);
  }, [workspaceId]);
  const filtered = entries.filter((entry) =>
    entry.userText.toLowerCase().includes(query.toLowerCase().trim()),
  );
  const jump = async (entry: TrailEntry) => {
    if (!source || busy) return;
    setBusy(true);
    try {
      if (visibleMobileHistory(workspaceId) !== source)
        throw new Error(
          "The conversation changed. Reopen History to continue.",
        );
      await source.jump(entry.messageId);
      onClose();
    } catch (error) {
      toast.error("Could not jump to this turn", {
        description: String(error),
      });
    } finally {
      setBusy(false);
    }
  };
  return (
    <Sheet
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <SheetContent
        side="bottom"
        className="mobile-bottom-sheet mobile-history-sheet"
        onOpenAutoFocus={(event) => {
          event.preventDefault();
          title.current?.focus();
        }}
        onCloseAutoFocus={(event) => {
          event.preventDefault();
          returnFocusRef.current?.focus({ preventScroll: true });
        }}
      >
        <SheetHeader>
          <SheetTitle ref={title} tabIndex={-1}>
            History
          </SheetTitle>
          <SheetDescription>
            Tap a prompt to return to that turn. Newest first.
          </SheetDescription>
        </SheetHeader>
        {entries.length > 0 && (
          <label className="mobile-history-search">
            <Search size={17} aria-hidden />
            <input
              aria-label="Find a prompt"
              placeholder="Find a prompt…"
              value={query}
              onChange={(event) => {
                setQuery(event.target.value);
                setLimit(50);
              }}
            />
          </label>
        )}
        <div
          className="mobile-history-list"
          aria-label="Conversation history"
          aria-busy={busy}
        >
          {filtered.slice(0, limit).map((entry) => (
            <button
              key={entry.messageId}
              disabled={busy}
              aria-label={`Jump to turn ${entry.turnIndex + 1}: ${entry.userText || "Image or attachment"}`}
              onClick={() => void jump(entry)}
            >
              <span className="text-label text-muted-foreground">
                Turn {entry.turnIndex + 1}
              </span>
              <span className="line-clamp-3 font-medium">
                {entry.userText || "Image or attachment"}
              </span>
              {entry.replySnippet && (
                <span className="line-clamp-1 text-label text-muted-foreground">
                  {entry.replySnippet}
                </span>
              )}
            </button>
          ))}
          {filtered.length > limit && (
            <button
              className="mobile-history-more"
              onClick={() => setLimit((value) => value + 50)}
            >
              Show older prompts
            </button>
          )}
          {filtered.length === 0 && (
            <p className="px-2 py-8 text-center text-body text-muted-foreground">
              {entries.length
                ? "No matching prompts."
                : source
                  ? "No prompts in this conversation yet."
                  : "Open an agent conversation to view its history."}
            </p>
          )}
        </div>
      </SheetContent>
    </Sheet>
  );
}
