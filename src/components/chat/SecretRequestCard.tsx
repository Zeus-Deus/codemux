import { useId, useRef, useState, type FormEvent } from "react";
import { KeyRound, ShieldCheck } from "lucide-react";

import { isRemoteClient } from "@/components/remote/is-remote-client";
import { useEaseInOnce } from "@/hooks/use-ease-in-once";
import { cn } from "@/lib/utils";
import { agentSecretRequestAnswer } from "@/tauri/commands";
import type { SecretRequest } from "@/tauri/types";
import { Button } from "../ui/button";
import { Input } from "../ui/input";
import { CHAT_COLUMN_INNER, CHAT_COLUMN_OUTER } from "./chat-column";
import { COMPOSER_OVERLAY_CARD } from "./composer-overlay";

/** What the card promises: the agent's commands can read the file, so it
 *  must not claim the agent never can. */
export const SECRET_PRIVACY_NOTE =
  "Kept out of chat, in a private file this agent's commands can read for 24 hours";

/** Shown instead of the field where typing the secret would send it across
 *  the network unencrypted. */
export const UNENCRYPTED_NOTE =
  "This connection isn't encrypted, so the secret can't be entered here. Use the Codemux desktop app, or connect over HTTPS or the relay.";

/**
 * Whether a remote page reaches Codemux over plain HTTP across the network.
 * A LAN client talks to its own origin, so the page's address decides. Only
 * HTTPS (the relay, or a TLS proxy) or this machine prove the value stays
 * private: an address range, a VPN's included, says nothing about whether
 * the link is encrypted. The desktop app never sends it over a network.
 */
export function unencryptedConnection(
  remote: boolean,
  location: Pick<Location, "protocol" | "hostname">,
): boolean {
  if (!remote || location.protocol === "https:") return false;
  const host = location.hostname
    .toLowerCase()
    .replace(/^\[|\]$/g, "")
    .replace(/\.$/, "");
  const v4 = /^(\d+)\.(\d+)\.\d+\.\d+$/.exec(host);
  const loopback =
    host === "localhost" || host === "::1" || (v4 !== null && v4[1] === "127");
  return !loopback;
}

// Masking a text field keeps password managers from offering to save a
// secret that is not a login. Without the property, fall back to a password
// field so the value is still hidden.
const MASKS_TEXT =
  typeof CSS !== "undefined" && CSS.supports?.("-webkit-text-security", "disc");

/** The chat's first open secret card, docked above the composer like a
 *  question. Answering removes it; the transcript's tool row keeps the
 *  record. */
export function SecretRequestPanel({ requests }: { requests: SecretRequest[] }) {
  const request = requests[0];
  if (!request) return null;
  return (
    <SecretRequestCard
      // A new request starts with an empty field.
      key={request.id}
      request={request}
      more={requests.length - 1}
    />
  );
}

function SecretRequestCard({
  request,
  more,
}: {
  request: SecretRequest;
  more: number;
}) {
  const inputId = useId();
  const errorId = useId();
  const privacyId = useId();
  const ease = useEaseInOnce(request.id);
  const [secret, setSecret] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Enter and a click can both submit before a render.
  const inFlight = useRef(false);
  const unencrypted = unencryptedConnection(isRemoteClient(), window.location);

  const send = async (
    answer: { type: "save"; secret: string } | { type: "decline" },
  ) => {
    if (inFlight.current) return;
    if (answer.type === "save" && (unencrypted || !answer.secret.trim())) return;
    inFlight.current = true;
    setSubmitting(true);
    setError(null);
    try {
      await agentSecretRequestAnswer(request.id, answer);
      setSecret("");
    } catch (cause) {
      // Commands reject with Codemux's own message, which never holds the
      // value; anything else gets generic copy.
      setError(
        typeof cause === "string" && cause.trim()
          ? cause
          : "Could not save it. Try again.",
      );
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  };

  const onSubmit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void send({ type: "save", secret });
  };

  return (
    <section
      aria-label="Secret requested"
      className={cn(
        CHAT_COLUMN_OUTER,
        "pb-2",
        ease && "animate-in fade-in-0 slide-in-from-bottom-2 duration-150 ease-out",
      )}
    >
      {/* Inset like the composer's strip and chin, so the card lines up
          with them instead of overhanging. */}
      <div className={cn(CHAT_COLUMN_INNER, COMPOSER_OVERLAY_CARD, "px-5")}>
        <form
          data-testid="secret-request-card"
          autoComplete="off"
          onSubmit={onSubmit}
          className="flex min-w-0 flex-col gap-3 rounded-lg border border-border bg-muted/40 px-4 py-3 shadow-sm"
        >
          <div className="flex min-w-0 flex-col gap-1">
            <div className="flex min-w-0 items-center gap-2">
              <KeyRound className="size-3.5 shrink-0 text-muted-foreground" aria-hidden />
              <label htmlFor={inputId} className="min-w-0 truncate text-body font-medium text-foreground">
                {request.label}
              </label>
              {more > 0 && (
                <span className="ml-auto shrink-0 text-caption text-muted-foreground/70">
                  {more} more after this
                </span>
              )}
            </div>
            {request.reason && (
              <p className="text-body-sm text-muted-foreground">{request.reason}</p>
            )}
          </div>
          <div className="flex min-w-0 items-center gap-2">
            <Input
              id={inputId}
              // A password field would offer to save the secret as a login.
              type={MASKS_TEXT ? "text" : "password"}
              className={cn("min-w-0 flex-1 font-mono", MASKS_TEXT && "[-webkit-text-security:disc]")}
              autoComplete="off"
              autoCorrect="off"
              autoCapitalize="off"
              spellCheck={false}
              data-1p-ignore
              data-lpignore="true"
              data-bwignore
              placeholder={request.placeholder ?? "Paste the secret"}
              value={secret}
              disabled={submitting || unencrypted}
              aria-invalid={error !== null || undefined}
              aria-describedby={error !== null ? `${privacyId} ${errorId}` : privacyId}
              onChange={(event) => setSecret(event.currentTarget.value)}
            />
            <Button type="submit" disabled={submitting || unencrypted || !secret.trim()}>
              Save securely
            </Button>
          </div>
          {error !== null && (
            <p id={errorId} role="alert" className="text-label text-destructive">
              {error}
            </p>
          )}
          <div className="flex min-w-0 items-center justify-between gap-2">
            <p
              id={privacyId}
              className="flex min-w-0 items-center gap-1.5 text-caption text-muted-foreground"
            >
              <ShieldCheck className="size-3.5 shrink-0" aria-hidden />
              {unencrypted ? UNENCRYPTED_NOTE : SECRET_PRIVACY_NOTE}
            </p>
            <Button
              type="button"
              size="xs"
              variant="ghost"
              disabled={submitting}
              onClick={() => void send({ type: "decline" })}
              className="shrink-0 text-muted-foreground"
            >
              Decline
            </Button>
          </div>
        </form>
      </div>
    </section>
  );
}
