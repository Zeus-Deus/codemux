import { useEffect, useState } from "react";
import { agentChatAcknowledgeDelivery, agentChatDeliveryAttention, agentChatRetryDelivery, type MessageDeliveryAttention } from "@/tauri/commands";
import { useTranscriptTopInset } from "./transcript-top-inset";
import { InlineButton } from "@/components/ui/button";

/** The database snapshot survives startup before any pane subscribes. */
export function DeliveryAttention({threadId}: {threadId: string}) {
  const topInset = useTranscriptTopInset();
  const [rows, setRows] = useState<MessageDeliveryAttention[]>([]);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    let alive = true;
    const refresh = () => {
      void agentChatDeliveryAttention(threadId).then(value => {
        if (alive) setRows(Array.isArray(value) ? value : []);
      }).catch(() => { /* Retain the last durable snapshot on a transient disconnect. */ });
    };
    setRows([]);
    setError(null);
    refresh();
    const timer = setInterval(refresh, 5000);
    return () => { alive = false; clearInterval(timer); };
  }, [threadId]);
  const act = async (row: MessageDeliveryAttention, retry: boolean) => {
    try {
      if (retry) await agentChatRetryDelivery(threadId, row.id);
      else await agentChatAcknowledgeDelivery(threadId, row.id);
      setRows(current => current.filter(item => item.id !== row.id));
      setError(null);
    } catch (cause) { setError(String(cause)); }
  };
  if (rows.length === 0 && !error) return null;
  return <section aria-label="Message delivery attention" style={{marginTop:topInset}} className="max-h-[35%] shrink-0 overflow-y-auto border-b border-hairline bg-surface-1 px-4 py-2 text-label">
    {rows.map(row => <div key={row.id} className="flex items-start gap-3 py-1">
      <div className="min-w-0 flex-1">
        <p className="font-medium">{row.can_retry ? "Message was not sent" : "Message delivery needs attention"}</p>
        {row.display_text && <p className="truncate text-muted-foreground">{row.display_text}</p>}
        <p className="text-muted-foreground">{row.reason}</p>
      </div>
      {row.can_retry && <InlineButton onClick={() => void act(row, true)}>Retry</InlineButton>}
      <InlineButton onClick={() => void act(row, false)}>Acknowledge</InlineButton>
    </div>)}
    {error && <p role="alert">{error}</p>}
  </section>;
}
