import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { NativeSelect } from "@/components/ui/native-select";
import type {
  AgentConnectorAccess,
  AgentConnectorClient,
  AgentConnectorIssuedToken,
  AgentConnectorPending,
} from "@/tauri/types";
import { CopyButton } from "./remote-access-section";
import type { LocalSetupRow } from "./outside-assistants-setup";
import { SettingsCard } from "./settings-primitives";

/** Presentational pieces of Settings → Outside assistants. State and every
 *  native call stay in the section; these render what they are given.
 *  Client names and labels are untrusted text, so they render as text only. */

export const ACCESS_LABELS: Record<AgentConnectorAccess, string> = {
  read_only: "Read-only", supervised: "Supervised", full_access: "Full access",
};

export function displayDate(value: string): string {
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Unknown" : date.toLocaleString();
}

/** A copyable value on a line of its own. */
export function CopySetup({ value, label }: { value: string; label: string }) {
  return (
    <div className="flex items-center gap-2">
      <code className="min-w-0 flex-1 select-text break-all rounded-md bg-muted/40 px-2 py-1.5 font-mono text-label">{value}</code>
      <CopyButton text={value} label={label} />
    </div>
  );
}

/** One always-open row per local client: name, command, copy. */
export function LocalSetupList({ rows }: { rows: LocalSetupRow[] }) {
  return (
    <SettingsCard className="p-0">
      <ul className="divide-y divide-border/40">
        {rows.map((row) => (
          // Narrow screens put the name above the command so the command keeps the width.
          <li key={row.id} className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-3 gap-y-0.5 px-4 py-2.5 sm:grid-cols-[6rem_minmax(0,1fr)_auto]">
            <span className="col-span-2 text-body-sm font-medium text-foreground sm:col-span-1">{row.label}</span>
            <code className="select-text font-mono text-label wrap-anywhere text-foreground/90">{row.command}</code>
            {/* Keyed by the command so a changed command never shows the old copy's check mark. */}
            <CopyButton key={row.command} text={row.command} label={row.copyLabel} />
          </li>
        ))}
      </ul>
    </SettingsCard>
  );
}

export function PendingConnection({ request, locked, canApprove, onApprove, onDeny }: {
  request: AgentConnectorPending; locked: boolean; canApprove: boolean;
  onApprove: (access: AgentConnectorAccess) => void; onDeny: () => void;
}) {
  const [selectedAccess, setAccess] = useState<AgentConnectorAccess>("read_only");
  const undecided = request.phase === "awaiting_approval";
  const access = request.phase === "approved_awaiting_client" ? request.access : selectedAccess;
  return (
    <li className="space-y-2 px-4 py-3" role="group" aria-label={`Connection request from ${request.clientName}`}>
      <div>
        <p className="break-all text-body font-medium text-foreground">
          {request.clientName}
          {request.reconnecting && <span className="ms-2 text-label font-normal text-muted-foreground">Reconnecting · approved before</span>}
        </p>
        <p className="break-all text-label text-muted-foreground">Registered callback origin: {request.callbackOrigin}</p>
        <p className="text-label text-muted-foreground">Client name and callback origin are self-asserted, not proof of identity.</p>
        <p className="text-caption text-muted-foreground">Requested {displayDate(request.requestedAt)}</p>
      </div>
      {undecided && <p className="text-label font-medium text-foreground">Approve only a sign-in you just started. Any program on this computer can ask.</p>}
      <p className="text-label text-muted-foreground">Applies to all Codemux workspaces and visible conversations, not just this project.</p>
      <p className="text-label text-muted-foreground">Conversation reads include visible user and top-level assistant messages, not reasoning or tool payloads.</p>
      {request.phase !== "denied" && <p className={access === "full_access" ? "text-label text-destructive" : "text-label text-muted-foreground"}>
        {access === "read_only" ? "Can read workspace state and visible conversations; cannot launch workers, send messages or change state." : access === "supervised"
          ? "Can start workers and send messages in supervised modes; some modes allow file edits. You handle approval requests in Codemux; the assistant cannot approve its own requests."
          : "Warning: Full access can run commands and change files with your account’s permissions, not a project-confined sandbox. Approve only an assistant you trust."}
      </p>}
      {request.phase === "approved_awaiting_client" && <div className="space-y-1">
        <p className="text-label font-medium">Approved · {ACCESS_LABELS[request.access]} · waiting for the assistant</p>
        <p className="text-label text-muted-foreground">Press Continue in the authorization browser, then let the assistant finish connecting. Access is not granted until its token exchange completes.</p>
      </div>}
      {request.phase === "denied" && <p className="text-label text-muted-foreground">Denied · no access was granted. The request will clear when the browser continues or it expires.</p>}
      {undecided && <div className="flex flex-wrap items-center gap-2">
        <NativeSelect size="sm" className="w-auto" aria-label={`Access for ${request.clientName}`} value={access} disabled={locked}
          onChange={(event) => setAccess(event.target.value as AgentConnectorAccess)}>
          {Object.entries(ACCESS_LABELS).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
        </NativeSelect>
        <Button variant="outline" size="sm" disabled={locked || !canApprove} onClick={() => onApprove(access)}>Approve</Button>
        <Button variant="ghost" size="sm" disabled={locked} onClick={onDeny}>Deny</Button>
      </div>}
    </li>
  );
}

export function GrantedConnection({ client, locked, onRevoke }: {
  client: AgentConnectorClient; locked: boolean; onRevoke: () => void;
}) {
  const [confirm, setConfirm] = useState(false);
  const token = client.kind === "token";
  const lapse = client.renews
    ? `Renews automatically · lapses if unused by ${displayDate(client.expiresAt)}`
    : `Expires ${displayDate(client.expiresAt)}`;
  return (
    <li role="group" aria-label={`${token ? "Token" : "Signed-in assistant"} ${client.clientName}`} className="space-y-2 px-4 py-3">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <p className="break-all text-body font-medium text-foreground">{client.clientName}</p>
          <p className="break-all text-label text-muted-foreground">
            {token ? "Token" : client.callbackOrigin ?? "Signed in"} · {ACCESS_LABELS[client.access]}
          </p>
          <p className="text-caption text-muted-foreground">{token ? "Created" : "Approved"} {displayDate(client.createdAt)} · {lapse}</p>
        </div>
        {!confirm && <Button variant="ghost" size="sm" disabled={locked} onClick={() => setConfirm(true)}>Revoke</Button>}
      </div>
      {confirm && <div className="flex flex-wrap items-center gap-2">
        <p className="flex-1 text-label">
          {token ? "Revoke this token? Assistants using it stop working right away." : "Revoke this assistant’s access? It will need approval to reconnect."}
        </p>
        <Button variant="ghost" size="sm" disabled={locked} onClick={() => setConfirm(false)}>Cancel</Button>
        <Button variant="destructive" size="sm" disabled={locked} onClick={onRevoke}>Revoke access</Button>
      </div>}
    </li>
  );
}

/** Two-step "Revoke all", inline like a single revoke. */
export function RevokeAll({ locked, onRevokeAll }: { locked: boolean; onRevokeAll: () => void }) {
  const [confirm, setConfirm] = useState(false);
  if (!confirm) return <Button variant="ghost" size="xs" className="text-muted-foreground" disabled={locked} onClick={() => setConfirm(true)}>Revoke all</Button>;
  return (
    <div className="flex items-center gap-2" role="group" aria-label="Confirm revoking every network assistant">
      <span className="text-label">Every network assistant stops working.</span>
      <Button variant="ghost" size="xs" disabled={locked} onClick={() => setConfirm(false)}>Cancel</Button>
      <Button variant="destructive" size="xs" disabled={locked} onClick={() => { setConfirm(false); onRevokeAll(); }}>Revoke every assistant</Button>
    </div>
  );
}

export function TokenForm({ label, access, locked, onLabelChange, onAccessChange, onCreate }: {
  label: string; access: AgentConnectorAccess; locked: boolean;
  onLabelChange: (label: string) => void; onAccessChange: (access: AgentConnectorAccess) => void; onCreate: () => void;
}) {
  return (
    <form className="space-y-1.5" onSubmit={(event) => { event.preventDefault(); onCreate(); }}>
      <div className="flex flex-wrap items-center gap-2">
        <Input aria-label="Token label" className="w-48" placeholder="hermes-bot" maxLength={64} value={label} disabled={locked}
          onChange={(event) => onLabelChange(event.target.value)} />
        <NativeSelect size="sm" className="w-auto" aria-label="Access for new token" value={access} disabled={locked}
          onChange={(event) => onAccessChange(event.target.value as AgentConnectorAccess)}>
          {Object.entries(ACCESS_LABELS).map(([value, text]) => <option key={value} value={value}>{text}</option>)}
        </NativeSelect>
        <Button type="submit" variant="outline" disabled={locked || label.trim() === ""}>Create token</Button>
      </div>
      <p className="text-label text-muted-foreground">For a bot that can’t sign in. A token works like a password: anyone who has it gets this access.</p>
    </form>
  );
}

/** Shown once, right after a token is created. The token lives only in the
 *  section's state; this card never puts it in a label, title or toast. */
export function IssuedTokenCard({ issued, onDone }: { issued: AgentConnectorIssuedToken; onDone: () => void }) {
  return (
    <SettingsCard role="status" className="space-y-2 p-3">
      <p className="text-body-sm font-medium text-foreground">Copy the token for “{issued.label}” now</p>
      <CopySetup value={issued.token} label="Copy token" />
      <p className="text-label leading-relaxed text-muted-foreground">
        Codemux won’t show it again. Send it as <code className="font-mono">Authorization: Bearer &lt;token&gt;</code> to{" "}
        <span className="break-all">{issued.mcpUrl}</span>. It expires {displayDate(issued.expiresAt)}, and stops working
        sooner if you change the port or public origin.
        {issued.replaced && ` The earlier token for “${issued.label}” no longer works.`}
      </p>
      <Button variant="outline" size="sm" onClick={onDone}>Done</Button>
    </SettingsCard>
  );
}
