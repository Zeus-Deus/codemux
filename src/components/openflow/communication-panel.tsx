import { useState, useRef, useEffect, useCallback } from "react";
import { useActiveCommLog } from "@/stores/openflow-store";
import { injectOrchestratorMessage } from "@/tauri/commands";
import { useOpenFlowStore } from "@/stores/openflow-store";
import {
  filterSystemMarkers,
  MAX_VISIBLE_MESSAGES,
} from "@/lib/openflow-utils";
import { CommLogMessage } from "./comm-log-message";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { ArrowDown, Send } from "lucide-react";

interface CommunicationPanelProps {
  runId: string;
}

export function CommunicationPanel({ runId }: CommunicationPanelProps) {
  const rawCommLog = useActiveCommLog();
  const fetchCommLog = useOpenFlowStore((s) => s.fetchCommLog);

  const [message, setMessage] = useState("");
  const [sending, setSending] = useState(false);
  const [autoFollow, setAutoFollow] = useState(true);
  const scrollRef = useRef<HTMLDivElement>(null);

  // Filter and limit messages
  const filtered = filterSystemMarkers(rawCommLog);
  const messages = filtered.slice(-MAX_VISIBLE_MESSAGES);

  // Track which user injections have been delivered
  const handledCount = rawCommLog
    .filter(
      (e) =>
        e.role === "system" &&
        e.message.trimStart().startsWith("HANDLED_INJECTIONS:"),
    )
    .reduce((max, e) => {
      const n = parseInt(e.message.split(":")[1]?.trim() ?? "0", 10);
      return Math.max(max, n);
    }, 0);

  let injectionIndex = 0;
  const deliveryMap = new Map<number, boolean>();
  for (let i = 0; i < filtered.length; i++) {
    if (
      filtered[i].role.toLowerCase() === "user/inject" ||
      filtered[i].role.toLowerCase().startsWith("user")
    ) {
      injectionIndex++;
      deliveryMap.set(i, injectionIndex <= handledCount);
    }
  }

  // Auto-scroll
  useEffect(() => {
    if (autoFollow && scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }
  }, [messages.length, autoFollow]);

  const handleScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 40;
    setAutoFollow(atBottom);
  }, []);

  const scrollToBottom = useCallback(() => {
    if (scrollRef.current) {
      scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
      setAutoFollow(true);
    }
  }, []);

  const handleSend = async () => {
    const text = message.trim();
    if (!text || sending) return;

    setSending(true);
    try {
      await injectOrchestratorMessage(runId, text);
      setMessage("");
      fetchCommLog(runId);
    } catch (err) {
      console.error("Failed to inject message:", err);
    } finally {
      setSending(false);
    }
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleSend();
    }
  };

  return (
    <div className="flex h-full flex-col">
      {/* Header */}
      <div className="flex items-center gap-2 border-b border-border/50 px-3 py-2">
        <h3 className="text-xs font-semibold text-foreground">Communication</h3>
        <span className="flex items-center gap-1">
          <span className="h-1.5 w-1.5 rounded-full bg-emerald-400 animate-pulse" />
          <span className="text-[10px] text-muted-foreground">Live</span>
        </span>
        <span className="ml-auto text-[10px] text-muted-foreground tabular-nums">
          {messages.length}
        </span>
      </div>

      {/* Messages */}
      <div
        ref={scrollRef}
        className="flex-1 overflow-y-auto min-h-0"
        onScroll={handleScroll}
      >
        {messages.length === 0 ? (
          <div className="flex items-center justify-center h-full text-xs text-muted-foreground">
            Waiting for communication...
          </div>
        ) : (
          <div className="divide-y divide-border/30">
            {messages.map((entry, i) => {
              const filteredIndex = filtered.indexOf(entry);
              return (
                <CommLogMessage
                  key={`${entry.timestamp}-${entry.role}-${i}`}
                  entry={entry}
                  isDelivered={deliveryMap.get(filteredIndex)}
                />
              );
            })}
          </div>
        )}
      </div>

      {/* Jump to latest */}
      {!autoFollow && (
        <div className="flex justify-center -mt-8 relative z-10 pointer-events-none">
          <Button
            variant="secondary"
            size="sm"
            className="h-6 text-[10px] pointer-events-auto shadow-lg"
            onClick={scrollToBottom}
          >
            <ArrowDown className="h-3 w-3 mr-1" />
            Latest
          </Button>
        </div>
      )}

      {/* Injection input */}
      <div className="border-t border-border/50 p-2">
        <div className="flex gap-1.5">
          <Input
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Send message to orchestrator..."
            className="h-7 text-xs"
            disabled={sending}
          />
          <Button
            size="sm"
            className="h-7 w-7 shrink-0 p-0"
            onClick={handleSend}
            disabled={!message.trim() || sending}
          >
            <Send className="h-3 w-3" />
          </Button>
        </div>
      </div>
    </div>
  );
}
