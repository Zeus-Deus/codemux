import { useRef, type ComponentProps, type ReactNode } from "react";
import { DialogContent } from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import type { AddonAccess, AddonManifest } from "@/lib/addons/types";
import {
  PERMISSION_LABELS,
  count,
  hasAccess,
  manifestAccess,
  type AddonProblem,
} from "./addon-presentation";

// These controlled dialogs have no Radix Trigger. Restore their actual opener,
// and keep Escape from also reaching the window-level Settings close shortcut.
export function AddonDialogContent(
  props: ComponentProps<typeof DialogContent>,
) {
  const opener = useRef<HTMLElement | null>(null);
  return (
    <DialogContent
      {...props}
      onOpenAutoFocus={() => {
        opener.current =
          document.activeElement instanceof HTMLElement
            ? document.activeElement
            : null;
      }}
      onCloseAutoFocus={(event) => {
        event.preventDefault();
        if (opener.current?.isConnected) opener.current.focus();
      }}
      onEscapeKeyDown={(event) => event.stopPropagation()}
    />
  );
}

/** An operation's failure, placed where the user is looking. */
export function ProblemAlert({
  problem,
  className,
}: {
  problem: AddonProblem | string | null;
  className?: string;
}) {
  if (!problem) return null;
  const { title, message } =
    typeof problem === "string" ? { title: null, message: problem } : problem;
  if (!message) return null;
  return (
    <p
      role="alert"
      className={cn(
        "rounded-lg border border-destructive/30 bg-destructive/5 p-3 text-body",
        className,
      )}
    >
      {title && <span className="font-medium">{title}. </span>}
      {message}
    </p>
  );
}

export function AccessList({ access }: { access: AddonAccess }) {
  return (
    <ul className="list-inside list-disc space-y-1">
      {access.permissions.map((p) => (
        <li key={p}>{PERMISSION_LABELS[p] ?? p}</li>
      ))}
      {access.http.map((grant) => (
        <li key={grant.origin}>
          {grant.methods.join(", ")} {grant.origin}
        </li>
      ))}
      {access.credentials.map((credential) => (
        <li key={`${credential.id}/${credential.origin}`}>
          Host-managed credential “{credential.label}” for {credential.origin}
        </li>
      ))}
    </ul>
  );
}

/** A titled access list; `tone="new"` marks access the user has not granted yet. */
export function AccessSection({
  title,
  access,
  tone = "plain",
  children,
}: {
  title: string;
  access: AddonAccess;
  tone?: "plain" | "new" | "removed";
  children?: ReactNode;
}) {
  return (
    <section
      aria-label={title}
      className={cn(
        "space-y-2 text-body",
        tone === "new" && "rounded-lg border border-warning/35 bg-warning/5 p-3",
        tone === "removed" && "text-muted-foreground",
      )}
    >
      <h4 className="font-medium">{title}</h4>
      {hasAccess(access) ? (
        <AccessList access={access} />
      ) : (
        <p className="text-muted-foreground">
          Private settings, storage, and declared UI only.
        </p>
      )}
      {children}
    </section>
  );
}

export function Contributions({ manifest }: { manifest: AddonManifest }) {
  const c = manifest.contributes;
  return (
    <p className="text-body text-muted-foreground">
      {[
        count(c.commands.length, "command"),
        count(c.panels.length, "panel"),
        count(c.composerActions.length, "composer action"),
        count(c.composerViews.length, "composer view"),
      ].join(" · ")}
    </p>
  );
}

/** Consequences of the network access a manifest declares. */
export function NetworkNotes({ manifest }: { manifest: AddonManifest }) {
  return (
    <>
      {manifest.http.some((grant) =>
        grant.methods.some((method) => method !== "GET"),
      ) && (
        <p className="text-body text-muted-foreground">
          This add-on can write to the listed external services using the
          declared methods.
        </p>
      )}
      {manifest.http.length > 0 && (
        <p className="text-body text-muted-foreground">
          Data sent to an external service cannot be recalled by removing the
          add-on.
        </p>
      )}
    </>
  );
}

/** Everything a manifest can access, as installed or as a new installation requests it. */
export function Capabilities({
  manifest,
  title = "Requested access",
}: {
  manifest: AddonManifest;
  title?: string;
}) {
  return (
    <div className="space-y-3">
      <AccessSection title={title} access={manifestAccess(manifest)} />
      <Contributions manifest={manifest} />
      <NetworkNotes manifest={manifest} />
    </div>
  );
}
