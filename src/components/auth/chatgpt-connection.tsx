import { Check, ExternalLink } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { useDelayedPending } from "@/hooks/use-delayed-pending";
import { cn } from "@/lib/utils";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import codexMark from "@/assets/preset-icons/codex.svg";

export const CHATGPT_USAGE_URL = "https://chatgpt.com/settings/usage";

/** Pending or connected: the flow has taken over and the connect button is
 *  replaced by a status card. */
export function useChatGptConnectionActive(): boolean {
  return useChatGptStore((s) => s.status?.phase === "pending" || s.status?.phase === "connected");
}

/** The OpenAI mark drawn as a mask over the button's icon colour, so it
 *  matches the label on filled and outline buttons in any theme. */
function ChatGptMark() {
  const mask = `url("${codexMark}") center / contain no-repeat`;
  return <span aria-hidden="true" className="size-4 shrink-0 bg-(--control-icon-color)" style={{ mask, WebkitMask: mask }} />;
}

export function ChatGptConnectButton({ variant = "default", className }: {
  variant?: "default" | "outline";
  className?: string;
}) {
  const busy = useChatGptStore((s) => s.busy);
  const loading = useChatGptStore((s) => s.loading);
  const start = useChatGptStore((s) => s.start);
  const starting = useDelayedPending(busy || loading);
  if (isRemoteClient()) return null;
  return (
    <Button variant={variant} className={cn("w-full gap-2.5", className)} size="lg" disabled={busy || loading} onClick={() => void start(null)}>
      {starting ? <Spinner immediate className="size-4" aria-hidden /> : <ChatGptMark />}
      Continue with ChatGPT
    </Button>
  );
}

/** "Finish in your browser" while pending, the account once connected,
 *  nothing otherwise. `onboarding` swaps the manage actions for a way in. */
export function ChatGptConnectionCard({ onboarding = false }: { onboarding?: boolean }) {
  const status = useChatGptStore((s) => s.status);
  const busy = useChatGptStore((s) => s.busy);
  const cancel = useChatGptStore((s) => s.cancel);
  const disconnect = useChatGptStore((s) => s.disconnect);
  const enterLocal = useChatGptStore((s) => s.enterLocal);
  if (isRemoteClient()) return null;
  if (status?.phase === "pending") return <div className="w-full rounded-lg border border-border bg-muted/30 p-4 space-y-3">
    <div className="flex items-center gap-2 text-body font-medium">
      <Spinner className="size-4" aria-hidden /> Finish in your browser
    </div>
    <p className="text-body-sm leading-relaxed text-muted-foreground">
      Approve Codemux and enable ChatGPT plan usage, then return here. This request expires after five minutes.
    </p>
    <Button variant="outline" size="sm" onClick={() => void cancel()} disabled={busy}>Cancel sign-in</Button>
  </div>;
  if (status?.phase === "connected") return <div className="w-full rounded-lg border border-border bg-muted/30 p-4 space-y-3">
    <div className="flex items-center gap-2 text-body font-medium"><Check className="size-4" /> ChatGPT connected</div>
    {status.email && <p className="text-body-sm text-muted-foreground break-all">{status.email}</p>}
    <p className="text-body-sm leading-relaxed text-muted-foreground">Eligible Codex requests use your ChatGPT plan.</p>
    {onboarding ? <Button className="w-full" size="lg" disabled={busy} onClick={() => void enterLocal()}>Continue to Codemux</Button> :
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="outline" size="sm" onClick={() => void openUrl(CHATGPT_USAGE_URL)}>Manage usage <ExternalLink className="size-3" /></Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={() => void disconnect()}>Disconnect</Button>
      </div>}
  </div>;
  return null;
}

/** Whether the ChatGPT connection has an error to show. */
function useChatGptHasError(): boolean {
  return useChatGptStore((s) => Boolean(s.error || s.status?.error || s.subscriptionError));
}

/** Connection errors, from starting sign-in through the browser flow
 *  failing or expiring, with a way to read the connection again. Nothing
 *  while there is no error. */
export function ChatGptConnectionErrors() {
  const status = useChatGptStore((s) => s.status);
  const busy = useChatGptStore((s) => s.busy);
  const error = useChatGptStore((s) => s.error);
  const subscriptionError = useChatGptStore((s) => s.subscriptionError);
  const refresh = useChatGptStore((s) => s.refresh);
  const hasError = useChatGptHasError();
  if (isRemoteClient() || !hasError) return null;
  return (
    <>
      {(error || status?.error) && <p role="alert" className="text-body-sm leading-relaxed text-destructive">{error ?? status?.error}</p>}
      {subscriptionError && <p role="alert" className="text-label text-muted-foreground">{subscriptionError}</p>}
      <Button variant="ghost" size="sm" disabled={busy} onClick={() => void refresh()}>Refresh connection</Button>
    </>
  );
}

/** Saved connections, the Codex CLI requirement and connection errors. */
export function ChatGptConnectionNotes() {
  const status = useChatGptStore((s) => s.status);
  const busy = useChatGptStore((s) => s.busy);
  const start = useChatGptStore((s) => s.start);
  const hasError = useChatGptHasError();
  if (isRemoteClient()) return null;
  const pending = status?.phase === "pending";
  const connected = status?.phase === "connected";
  const hasProfiles = !pending && Boolean(status?.profiles.length);
  if (!hasProfiles && status?.installed !== false && !hasError) return null;
  return (
    <div className="w-full space-y-3">
      {hasProfiles && status ? <div className="space-y-1">
        <p className="text-label text-muted-foreground">Saved connections</p>
        {status.profiles.map((profile) => <Button key={profile.id} variant="ghost" size="sm" className="w-full justify-start truncate" disabled={busy}
          onClick={() => void start(profile.id)}>{profile.email ?? "ChatGPT account"} · {profile.label}</Button>)}
        {connected && <Button variant="ghost" size="sm" disabled={busy} onClick={() => void start(null)}>Use a different ChatGPT account</Button>}
      </div> : null}
      {status?.installed === false && <p className="text-label leading-relaxed text-muted-foreground">
        Codex CLI is required to start coding. <button className="underline underline-offset-4" onClick={() => void openUrl("https://developers.openai.com/codex/cli/")}>Installation guide</button>
      </p>}
      <ChatGptConnectionErrors />
    </div>
  );
}

export function ChatGptConnection({ onboarding = false }: { onboarding?: boolean }) {
  const active = useChatGptConnectionActive();
  if (isRemoteClient()) return null;
  return (
    <section aria-label="ChatGPT connection" className="w-full space-y-3">
      {!onboarding && <div className="space-y-1">
        <h3 className="text-body-lg font-semibold text-foreground">Use your ChatGPT plan</h3>
        <p className="text-body-sm leading-relaxed text-muted-foreground">
          Connect your plan to Codex. This is separate from your Codemux account and cloud sync.
        </p>
      </div>}
      {active ? <ChatGptConnectionCard onboarding={onboarding} /> : <>
        <ChatGptConnectButton />
        {onboarding && <p className="text-label leading-relaxed text-center text-muted-foreground">
          Use your ChatGPT plan with Codex. No API key needed.
        </p>}
      </>}
      <ChatGptConnectionNotes />
    </section>
  );
}
