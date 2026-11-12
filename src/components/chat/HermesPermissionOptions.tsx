import { CodeBlock, CodeBlockContent } from "@/components/ui/code-block";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import type { ApprovalDecision } from "@/tauri/events";

export function isHermesPermission(payload: unknown): boolean {
  return !!payload && typeof payload === "object" && (payload as { _codemuxProvider?: string })._codemuxProvider === "hermes";
}

/** A permission request answered with one of the agent's own options, sent
 *  as its exact id: every Hermes request, and an Antigravity request that
 *  offers no "allow once" (a generic Allow could not be honoured there). */
export function isExactOptionPermission(payload: unknown): boolean {
  return isHermesPermission(payload) ||
    (!!payload && typeof payload === "object" && (payload as { _codemuxExactOptions?: boolean })._codemuxExactOptions === true);
}
export function HermesPermissionOptions({ payload, onDecide }: { payload: unknown; onDecide: (decision: ApprovalDecision) => void }) {
  const submitted = useRef(false);
  const [disabled, setDisabled] = useState(false);
  const p = payload as { options?: Array<{ optionId: string; name: string; kind: string }>; toolCall?: { title?: string; rawInput?: unknown } };
  const hermes = isHermesPermission(payload);
  // Outside Hermes these options come from an agent whose "always" choice
  // can be saved in its own settings, beyond this chat.
  const lasting = !hermes && (p.options ?? []).some(o => o.kind === "allow_always");
  const choose = (decision: ApprovalDecision) => { if (submitted.current) return; submitted.current = true; setDisabled(true); onDecide(decision); };
  return <div className="space-y-2 border-t border-hairline p-3 text-label">
    <p>{p.toolCall?.title ?? (hermes ? "Hermes requests permission" : "Permission requested")}</p>
    {p.toolCall?.rawInput != null && <CodeBlock><CodeBlockContent className="max-h-40 whitespace-pre-wrap pt-3 text-muted-foreground">{JSON.stringify(p.toolCall.rawInput, null, 2)}</CodeBlockContent></CodeBlock>}
    <div className="flex flex-wrap gap-2">{(p.options ?? []).filter(o => typeof o.optionId === "string" && typeof o.name === "string").map(o => <Button type="button" variant="outline" size="sm" disabled={disabled} key={o.optionId} onClick={() => choose({ decision: "provider_option", option_id: o.optionId })}>{o.name}</Button>)}<Button type="button" variant="ghost" size="sm" disabled={disabled} onClick={() => choose({ decision: "cancel" })}>Cancel</Button></div>
    {lasting && <p className="text-caption text-muted-foreground">The agent offers no one-time approval here. An “always” choice can be saved in the agent’s own settings and apply beyond this chat.</p>}
  </div>;
}

export function hermesPermissionAllowed(payload: unknown, optionId: string): boolean {
  if (!isExactOptionPermission(payload)) return false;
  const p = payload as { options?: Array<{ optionId: string; kind: string }> };
  return (p.options ?? []).some(o => o.optionId === optionId && (o.kind === "allow_once" || o.kind === "allow_always"));
}
