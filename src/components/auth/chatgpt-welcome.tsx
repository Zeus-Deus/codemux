import { useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { CHATGPT_USAGE_URL } from "./chatgpt-connection";

export function ChatGptWelcome({ enterWorkbench }: { enterWorkbench: boolean }) {
  const status = useChatGptStore((s) => s.status);
  const busy = useChatGptStore((s) => s.busy);
  const error = useChatGptStore((s) => s.error);
  const acknowledge = useChatGptStore((s) => s.acknowledgeWelcome);
  const enterLocal = useChatGptStore((s) => s.enterLocal);
  const [saving, setSaving] = useState(false);
  const gotItRef = useRef<HTMLButtonElement>(null);
  if (isRemoteClient()) return null;
  const dismiss = async () => {
    if (busy || saving) return;
    setSaving(true);
    try { if (await acknowledge() && enterWorkbench) await enterLocal(); }
    finally { setSaving(false); }
  };
  return <Dialog open={status?.phase === "connected" && status.welcomePending} onOpenChange={(open) => { if (!open) void dismiss(); }}>
    {/* Centered with layout rather than the default translate, which leaves
        text soft on a half pixel (see DIALOG_CRISP_POSITION). Focus starts
        on the acknowledgement, not the first control (the usage link). */}
    <DialogContent className="inset-0 m-auto h-fit translate-x-0 translate-y-0 p-6 sm:max-w-md" showCloseButton={false}
      onOpenAutoFocus={(event) => { event.preventDefault(); gotItRef.current?.focus(); }}
      onEscapeKeyDown={(event) => { event.preventDefault(); void dismiss(); }} onPointerDownOutside={(event) => event.preventDefault()}>
      <ProviderLogo provider="codex" className="size-8" />
      <DialogHeader>
        <DialogTitle>You're using your ChatGPT plan</DialogTitle>
        <DialogDescription className="text-body leading-relaxed">
          Codex in Codemux now runs on your ChatGPT plan and its usage limits. No API key needed, and it doesn't sign you in to Codemux sync.
        </DialogDescription>
      </DialogHeader>
      <button className="text-body-sm text-muted-foreground underline underline-offset-4 text-left" onClick={() => void openUrl(CHATGPT_USAGE_URL)}>
        Manage usage in ChatGPT settings
      </button>
      {error && <p role="alert" className="text-body-sm text-destructive">{error}</p>}
      <Button ref={gotItRef} size="lg" disabled={busy || saving} onClick={() => void dismiss()}>Got it</Button>
    </DialogContent>
  </Dialog>;
}
