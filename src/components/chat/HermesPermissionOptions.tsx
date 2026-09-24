import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import type { ApprovalDecision } from "@/tauri/events";

export function isHermesPermission(payload: unknown): boolean {
  return !!payload && typeof payload === "object" && (payload as { _codemuxProvider?: string })._codemuxProvider === "hermes";
}
export function HermesPermissionOptions({ payload, onDecide }: { payload: unknown; onDecide: (decision: ApprovalDecision) => void }) {
  const submitted = useRef(false);
  const [disabled, setDisabled] = useState(false);
  const p = payload as { options?: Array<{ optionId: string; name: string; kind: string }>; toolCall?: { title?: string; rawInput?: unknown } };
  const choose = (decision: ApprovalDecision) => { if (submitted.current) return; submitted.current = true; setDisabled(true); onDecide(decision); };
  return <div className="space-y-2 border-t border-border/60 p-3 text-label">
    <p>{p.toolCall?.title ?? "Hermes requests permission"}</p>
    {p.toolCall?.rawInput != null && <pre className="max-h-40 overflow-auto whitespace-pre-wrap text-muted-foreground">{JSON.stringify(p.toolCall.rawInput, null, 2)}</pre>}
    <div className="flex flex-wrap gap-2">{(p.options ?? []).filter(o => typeof o.optionId === "string" && typeof o.name === "string").map(o => <Button type="button" variant="outline" size="sm" disabled={disabled} key={o.optionId} onClick={() => choose({ decision: "provider_option", option_id: o.optionId })}>{o.name}</Button>)}<Button type="button" variant="ghost" size="sm" disabled={disabled} onClick={() => choose({ decision: "cancel" })}>Cancel</Button></div>
  </div>;
}

export function hermesPermissionAllowed(payload: unknown, optionId: string): boolean {
  if (!isHermesPermission(payload)) return false;
  const p = payload as { options?: Array<{ optionId: string; kind: string }> };
  return (p.options ?? []).some(o => o.optionId === optionId && (o.kind === "allow_once" || o.kind === "allow_always"));
}
