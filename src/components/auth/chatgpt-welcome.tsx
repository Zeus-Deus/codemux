import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle, DIALOG_CRISP_POSITION } from "@/components/ui/dialog";
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
  if (isRemoteClient()) return null;
  const dismiss = async () => {
    if (busy || saving) return;
    setSaving(true);
    try { if (await acknowledge() && enterWorkbench) await enterLocal(); }
    finally { setSaving(false); }
  };
  return <Dialog open={status?.phase === "connected" && status.welcomePending} onOpenChange={(open) => { if (!open) void dismiss(); }}>
    <DialogContent className={`${DIALOG_CRISP_POSITION} p-6 sm:max-w-md`} showCloseButton={false}
      onEscapeKeyDown={(event) => { event.preventDefault(); void dismiss(); }} onPointerDownOutside={(event) => event.preventDefault()}>
      <ProviderLogo provider="codex" className="size-8" />
      <DialogHeader>
        <DialogTitle>You're using your ChatGPT plan</DialogTitle>
        <DialogDescription className="text-body leading-relaxed">
          Eligible Codex requests in CodeMux use your ChatGPT plan and its shared usage limits. This does not sign you into CodeMux cloud sync.
        </DialogDescription>
      </DialogHeader>
      <button className="text-body-sm text-muted-foreground underline underline-offset-4 text-left" onClick={() => void openUrl(CHATGPT_USAGE_URL)}>
        Manage usage in ChatGPT settings
      </button>
      {error && <p role="alert" className="text-body-sm text-destructive">{error}</p>}
      <Button size="lg" disabled={busy || saving} onClick={() => void dismiss()}>Got it</Button>
    </DialogContent>
  </Dialog>;
}
