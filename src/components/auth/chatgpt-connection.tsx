import { Check, ExternalLink, Loader2 } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { Button } from "@/components/ui/button";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { isRemoteClient } from "@/components/remote/is-remote-client";

export const CHATGPT_USAGE_URL = "https://chatgpt.com/settings/usage";
export function ChatGptConnection({ onboarding = false }: { onboarding?: boolean }) {
  const status = useChatGptStore((s) => s.status);
  const busy = useChatGptStore((s) => s.busy);
  const loading = useChatGptStore((s) => s.loading);
  const error = useChatGptStore((s) => s.error);
  const subscriptionError = useChatGptStore((s) => s.subscriptionError);
  const start = useChatGptStore((s) => s.start);
  const cancel = useChatGptStore((s) => s.cancel);
  const refresh = useChatGptStore((s) => s.refresh);
  const disconnect = useChatGptStore((s) => s.disconnect);
  const enterLocal = useChatGptStore((s) => s.enterLocal);
  if (isRemoteClient()) return null;
  const pending = status?.phase === "pending";
  const connected = status?.phase === "connected";
  return (
    <section aria-label="ChatGPT connection" className="w-full space-y-3">
      {!onboarding && <div className="space-y-1">
        <h3 className="text-body-lg font-semibold text-foreground">Use your ChatGPT plan</h3>
        <p className="text-body-sm leading-relaxed text-muted-foreground">
          Connect your plan to Codex. This is separate from your CodeMux account and cloud sync.
        </p>
      </div>}
      {pending ? <div className="rounded-lg border border-border bg-muted/30 p-4 space-y-3">
        <div className="flex items-center gap-2 text-body font-medium">
          <Loader2 className="size-4 animate-spin" /> Finish in your browser
        </div>
        <p className="text-body-sm leading-relaxed text-muted-foreground">
          Approve CodeMux and enable ChatGPT plan usage, then return here. This request expires after five minutes.
        </p>
        <Button variant="outline" size="sm" onClick={() => void cancel()} disabled={busy}>Cancel sign-in</Button>
      </div> : connected ? <div className="rounded-lg border border-border bg-muted/30 p-4 space-y-3">
        <div className="flex items-center gap-2 text-body font-medium"><Check className="size-4" /> ChatGPT connected</div>
        {status.email && <p className="text-body-sm text-muted-foreground break-all">{status.email}</p>}
        <p className="text-body-sm leading-relaxed text-muted-foreground">Eligible Codex requests use your ChatGPT plan.</p>
        {onboarding ? <Button className="w-full" size="lg" disabled={busy} onClick={() => void enterLocal()}>Continue to CodeMux</Button> :
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => void openUrl(CHATGPT_USAGE_URL)}>Manage usage <ExternalLink className="size-3" /></Button>
            <Button variant="ghost" size="sm" disabled={busy} onClick={() => void disconnect()}>Disconnect</Button>
          </div>}
      </div> : <>
        <Button className="w-full gap-2.5" size="lg" disabled={busy || loading} onClick={() => void start(null)}>
          {busy || loading ? <Loader2 className="size-4 animate-spin" /> : <span aria-hidden="true"><ProviderLogo provider="codex" className="size-4 brightness-0 invert dark:invert-0" /></span>}
          Continue with ChatGPT
        </Button>
        {onboarding && <p className="text-label leading-relaxed text-center text-muted-foreground">
          Use your ChatGPT plan with Codex. No API key needed.
        </p>}
      </>}
      {!pending && status?.profiles.length ? <div className="space-y-1">
        <p className="text-label text-muted-foreground">Saved connections</p>
        {status.profiles.map((profile) => <Button key={profile.id} variant="ghost" size="sm" className="w-full justify-start truncate" disabled={busy}
          onClick={() => void start(profile.id)}>{profile.email ?? "ChatGPT account"} · {profile.label}</Button>)}
        {connected && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void start(null)}>Use a different ChatGPT account</Button>}
      </div> : null}
      {status?.installed === false && <p className="text-label leading-relaxed text-muted-foreground">
        Codex CLI is required to start coding. <button className="underline underline-offset-4" onClick={() => void openUrl("https://developers.openai.com/codex/cli/")}>Installation guide</button>
      </p>}
      {(error || status?.error) && <p role="alert" className="text-body-sm leading-relaxed text-destructive">{error ?? status?.error}</p>}
      {subscriptionError && <p role="alert" className="text-label text-muted-foreground">{subscriptionError}</p>}
      {(error || status?.error || subscriptionError) && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void refresh()}>Refresh connection</Button>}
    </section>
  );
}
