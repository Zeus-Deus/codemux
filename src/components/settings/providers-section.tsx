import { useCallback, useEffect, useMemo, useState } from "react";
import { ExternalLink, Plus, RefreshCw } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { ProviderLogo } from "@/components/chat/provider-logo";
import { Button } from "@/components/ui/button";
import { Spinner } from "@/components/ui/spinner";
import { Switch } from "@/components/ui/switch";
import { Eyebrow } from "@/components/ui/eyebrow";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { CHATGPT_USAGE_URL } from "@/components/auth/chatgpt-connection";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { isLastDefaultProvider, setProviderEnabled, useDisabledProviders } from "@/lib/agent-chat/provider-enablement";
import { useChatDraftStore } from "@/stores/chat-draft-store";
import { runAcpMutation, useAcpMutationLock } from "@/stores/acp-mutation-lock";
import { useChatGptStore } from "@/stores/chatgpt-store";
import { useCustomAcp, acpCatalogKey } from "@/stores/custom-acp-store";
import { useHermes } from "@/stores/hermes-store";
import {
  refreshProviderCapabilitiesForIntent,
  selectCapabilities,
  useProviderCapabilities,
} from "@/stores/provider-capabilities-store";
import { useProbedProviderHealth, useProviderHealth } from "@/stores/provider-health-store";
import type { AgentChatProviderKind, ProviderHealthReport } from "@/tauri/types";
import { acpSaveAgent, type AcpAgent } from "@/tauri/custom-acp";
import { useCustomAcpEvents } from "@/hooks/use-custom-acp-events";
import { SectionHeader } from "./settings-primitives";
import { CommandRow, DetailHeader, DetailRow, DetailSection, StatusLine, type ProviderStatus } from "./provider-parts";
import { HermesProviderDetail } from "./hermes-setting";
import { CustomAcpDetail } from "./custom-acp-settings";

/** The CLIs Codemux drives directly, in list order. */
const BUILT_IN: readonly { id: AgentChatProviderKind; name: string }[] = [
  { id: "claude", name: "Claude" },
  { id: "codex", name: "Codex" },
  { id: "opencode", name: "OpenCode" },
  { id: "cursor", name: "Cursor" },
  { id: "grok", name: "Grok" },
];
const PROBED: readonly AgentChatProviderKind[] = BUILT_IN.map((p) => p.id);

/** Turn a provider on or off. Untouched new-chat drafts sitting on a
 *  provider that was just turned off move to the next default, so an open
 *  empty composer cannot start a chat on it. */
function toggleProvider(id: AgentChatProviderKind, on: boolean) {
  if (!setProviderEnabled(id, on)) return;
  if (!on) useChatDraftStore.getState().applyNewChatDefaults(new Set([id]));
}

const LAST_PROVIDER_HINT = "New chats need at least one of Claude, Codex, OpenCode, Cursor or Grok.";

/** Selection: a built-in provider, Hermes, a saved custom agent, or the
 *  editor for a new one. */
type Selected =
  | { kind: "builtin"; id: AgentChatProviderKind }
  | { kind: "hermes" }
  | { kind: "acp"; id: string }
  | { kind: "acp-new" };

const sameSelection = (a: Selected, b: Selected) =>
  a.kind === b.kind && ("id" in a ? a.id === (b as typeof a).id : true);

function healthStatus(report: ProviderHealthReport | null | undefined, pending: boolean): ProviderStatus {
  if (!report) return pending ? { tone: "pending", label: "Checking…" } : { tone: "warn", label: "Couldn't check" };
  if (!report.installed) return { tone: "off", label: "Not installed" };
  if (report.status === "ready") return { tone: "ok", label: "Ready" };
  if (report.login_command) return { tone: "warn", label: "Signed out" };
  if (report.status === "warning") return { tone: "warn", label: "Couldn't verify" };
  return { tone: "error", label: "Not working" };
}

/**
 * Settings → Providers: every agent Codemux can run, one row each, with the
 * selected provider's accounts and setup on the right.
 */
export function ProvidersSection() {
  const remote = isRemoteClient();
  const disabled = useDisabledProviders();
  const probed = useMemo(() => PROBED.filter((p) => !disabled.has(p)), [disabled]);
  const health = useProbedProviderHealth(probed, true);
  const refreshHealth = useProviderHealth((s) => s.refresh);
  const profiles = useHermes((s) => s.profiles);
  const agents = useCustomAcp((s) => s.agents);
  const catalogs = useCustomAcp((s) => s.catalogs);
  const acpWarning = useCustomAcpEvents();
  const [selected, setSelected] = useState<Selected>({ kind: "builtin", id: "claude" });
  // A custom agent whose editor is open: its list switch stays inert so a
  // flag-only save cannot replace the definition being edited. Writes in
  // flight are locked per agent in `useAcpMutationLock`, independent of
  // which panel is mounted.
  const [editingAgent, setEditingAgent] = useState<string | null>(null);
  const mutatingAgents = useAcpMutationLock((s) => s.pending);
  const selectedAgentId = selected.kind === "acp" ? selected.id : null;
  const onAgentEditing = useCallback(
    (editing: boolean) =>
      setEditingAgent((current) =>
        editing ? selectedAgentId : current === selectedAgentId ? null : current,
      ),
    [selectedAgentId],
  );

  useEffect(() => {
    if (!useHermes.getState().profiles) useHermes.getState().loadProfiles().catch(() => {});
    void useCustomAcp.getState().loadAgents().catch(() => {});
    void useChatGptStore.getState().bootstrap();
  }, []);

  // A deleted custom agent cannot stay selected.
  useEffect(() => {
    if (selected.kind === "acp" && agents && !agents.some((a) => a.id === selected.id)) {
      setSelected({ kind: "builtin", id: "claude" });
    }
  }, [agents, selected]);

  const statusFor = (id: AgentChatProviderKind): ProviderStatus => {
    if (disabled.has(id)) return { tone: "off", label: "Off" };
    const slot = health[id];
    return healthStatus(slot?.report, slot?.pending ?? true);
  };
  const hermesStatus: ProviderStatus = disabled.has("hermes")
    ? { tone: "off", label: "Off" }
    : profiles === null
      ? { tone: "pending", label: "Checking…" }
      : profiles.length === 0
        ? { tone: "warn", label: "No profiles found" }
        : { tone: "ok", label: `${profiles.length} ${profiles.length === 1 ? "profile" : "profiles"}` };
  const acpStatus = (agent: AcpAgent): ProviderStatus =>
    !agent.enabled
      ? { tone: "off", label: "Off" }
      : catalogs[acpCatalogKey(agent.id, agent.revision)]
        ? { tone: "ok", label: "Ready" }
        : { tone: "pending", label: "Not tested yet" };

  const [checking, setChecking] = useState(false);
  const checkAll = () => {
    setChecking(true);
    void Promise.all(probed.map((p) => refreshHealth(p, { force: true }))).finally(() => setChecking(false));
  };

  return (
    <div>
      <SectionHeader
        title="Providers"
        description="The agents Codemux can run, and the accounts they use."
        action={
          <Button variant="ghost" size="sm" className="text-muted-foreground" disabled={checking} onClick={checkAll}>
            {checking ? <Spinner className="size-3.5" aria-hidden /> : <RefreshCw />}
            Check all
          </Button>
        }
      />
      <div className="@container">
        <div className="grid min-h-[520px] grid-cols-1 overflow-hidden rounded-lg border border-border bg-background @2xl:grid-cols-[240px_minmax(0,1fr)]">
          <nav aria-label="Providers" className="flex flex-col gap-px border-b border-border p-1.5 @2xl:border-r @2xl:border-b-0">
            {BUILT_IN.map((p) => (
              <ProviderListRow
                key={p.id}
                name={p.name}
                logo={<ProviderLogo provider={p.id} className="size-4" />}
                status={statusFor(p.id)}
                selected={sameSelection(selected, { kind: "builtin", id: p.id })}
                onSelect={() => setSelected({ kind: "builtin", id: p.id })}
                enabled={!disabled.has(p.id)}
                onEnabledChange={(on) => toggleProvider(p.id, on)}
                lockedReason={isLastDefaultProvider(p.id, disabled) ? LAST_PROVIDER_HINT : undefined}
              />
            ))}
            <ProviderListRow
              name="Hermes"
              logo={<ProviderLogo provider="hermes" className="size-4" />}
              status={hermesStatus}
              selected={selected.kind === "hermes"}
              onSelect={() => setSelected({ kind: "hermes" })}
              enabled={!disabled.has("hermes")}
              onEnabledChange={(on) => toggleProvider("hermes", on)}
            />
            {!remote && (
              <>
                <Eyebrow className="px-2.5 pt-3 pb-1">Custom agents</Eyebrow>
                {agents?.map((agent) => (
                  <ProviderListRow
                    key={agent.id}
                    name={agent.name}
                    logo={<ProviderLogo provider="acp" className="size-4" />}
                    status={acpStatus(agent)}
                    selected={sameSelection(selected, { kind: "acp", id: agent.id })}
                    onSelect={() => setSelected({ kind: "acp", id: agent.id })}
                    enabled={agent.enabled}
                    switchDisabled={editingAgent === agent.id || !!mutatingAgents[agent.id]}
                    onEnabledChange={(on) => {
                      setCustomAgentEnabled(agent, on).catch((e) => toast.failure(`Couldn't update ${agent.name}`, e));
                    }}
                  />
                ))}
                <button
                  type="button"
                  onClick={() => setSelected({ kind: "acp-new" })}
                  className={cn(
                    "flex h-9 items-center gap-2 rounded-md px-2.5 text-body-sm text-muted-foreground transition-colors duration-100 hover:bg-surface-2 hover:text-foreground",
                    selected.kind === "acp-new" && "bg-surface-3 text-foreground",
                  )}
                >
                  <Plus className="size-3.5" />
                  <span>Add custom agent</span>
                </button>
                {acpWarning && <p role="status" className="px-2.5 pt-1 text-label text-muted-foreground">{acpWarning}</p>}
              </>
            )}
          </nav>
          <div className="min-w-0">
            {selected.kind === "builtin" && (
              <BuiltInDetail
                key={selected.id}
                id={selected.id}
                name={BUILT_IN.find((p) => p.id === selected.id)!.name}
                report={health[selected.id]?.report ?? null}
                status={statusFor(selected.id)}
                enabled={!disabled.has(selected.id)}
                lockedReason={isLastDefaultProvider(selected.id, disabled) ? LAST_PROVIDER_HINT : undefined}
              />
            )}
            {selected.kind === "hermes" && (
              <HermesProviderDetail
                status={hermesStatus}
                enabled={!disabled.has("hermes")}
                onEnabledChange={(on) => toggleProvider("hermes", on)}
              />
            )}
            {(selected.kind === "acp" || selected.kind === "acp-new") && (
              <CustomAcpDetail
                key={selected.kind === "acp" ? selected.id : "new"}
                agentId={selected.kind === "acp" ? selected.id : null}
                onSaved={(id) => setSelected({ kind: "acp", id })}
                onCancelNew={() => setSelected({ kind: "builtin", id: "claude" })}
                onEditingChange={onAgentEditing}
              />
            )}
          </div>
        </div>
      </div>
    </div>
  );
}

/** Flip a custom agent's own enabled flag, retaining every saved value. */
async function setCustomAgentEnabled(agent: AcpAgent, enabled: boolean) {
  await runAcpMutation(agent.id, async () => {
    await acpSaveAgent({
      id: agent.id,
      name: agent.name,
      executable: agent.executable,
      args: agent.args,
      environment: Object.fromEntries(Object.keys(agent.environment).map((k) => [k, null])),
      enabled,
      auth_method: agent.auth_method,
    });
    await useCustomAcp.getState().loadAgents(true);
  });
}

function ProviderListRow({
  name,
  logo,
  status,
  selected,
  onSelect,
  enabled,
  onEnabledChange,
  switchDisabled,
  lockedReason,
}: {
  name: string;
  logo: React.ReactNode;
  status: ProviderStatus;
  selected: boolean;
  onSelect: () => void;
  enabled: boolean;
  onEnabledChange: (enabled: boolean) => void;
  switchDisabled?: boolean;
  /** Why the provider cannot be turned off right now. */
  lockedReason?: string;
}) {
  return (
    <div
      className={cn(
        "flex items-center gap-2 rounded-md pr-2.5 transition-colors duration-100",
        selected ? "bg-surface-3" : "hover:bg-surface-2",
      )}
    >
      <button
        type="button"
        aria-current={selected ? "true" : undefined}
        onClick={onSelect}
        className="flex min-w-0 flex-1 items-center gap-2.5 py-2 pl-2.5 text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/60 rounded-md"
      >
        <span aria-hidden className={cn("flex size-5 shrink-0 items-center justify-center", !enabled && "opacity-50")}>{logo}</span>
        <span className="min-w-0 flex-1">
          <span className={cn("block truncate text-body font-medium", enabled ? "text-foreground" : "text-muted-foreground")}>{name}</span>
          <StatusLine status={status} className="flex" />
        </span>
      </button>
      <Switch
        size="sm"
        checked={enabled}
        aria-label={`Use ${name}`}
        title={lockedReason}
        disabled={switchDisabled || (enabled && !!lockedReason)}
        onCheckedChange={onEnabledChange}
      />
    </div>
  );
}

function BuiltInDetail({
  id,
  name,
  report,
  status,
  enabled,
  lockedReason,
}: {
  id: AgentChatProviderKind;
  name: string;
  report: ProviderHealthReport | null;
  status: ProviderStatus;
  enabled: boolean;
  lockedReason?: string;
}) {
  const refresh = useProviderHealth((s) => s.refresh);
  const caps = useProviderCapabilities((s) => selectCapabilities(s, id));
  const [checking, setChecking] = useState(false);
  const ready = enabled && report?.installed;
  // Opening a provider's page is the intent that lets its catalog load.
  useEffect(() => {
    if (ready) void refreshProviderCapabilitiesForIntent(id);
  }, [id, ready]);
  const check = () => {
    setChecking(true);
    void refresh(id, { force: true }).finally(() => setChecking(false));
  };
  const models = caps?.models ?? [];
  return (
    <div>
      <DetailHeader
        logo={<ProviderLogo provider={id} className="size-5" />}
        name={name}
        status={status}
        version={enabled ? report?.version : null}
        enabled={enabled}
        onEnabledChange={(on) => toggleProvider(id, on)}
        switchDisabled={enabled && !!lockedReason}
        switchTitle={lockedReason}
        actions={enabled && (
          <Button variant="ghost" size="sm" className="text-muted-foreground" disabled={checking} onClick={check}>
            {checking ? <Spinner className="size-3.5" aria-hidden /> : <RefreshCw />}
            Check
          </Button>
        )}
      />
      {lockedReason && enabled && (
        <p className="-mt-2 px-5 pb-3 text-label text-muted-foreground">{lockedReason}</p>
      )}
      {!enabled ? (
        <DetailSection title="Turned off">
          <p className="text-body-sm text-muted-foreground">
            {name} is hidden from the model picker and new chats. Chats that already use it keep working.
          </p>
        </DetailSection>
      ) : (
        <>
          {report && !report.installed && (
            <DetailSection title="Install">
              <p className="text-body-sm text-muted-foreground">{report.message ?? `${name} isn't installed on this computer.`} Install it, then Check again.</p>
            </DetailSection>
          )}
          {report?.installed && report.login_command && (
            <DetailSection title="Sign in" description={`Run this in a terminal, then Check again.`}>
              <CommandRow command={report.login_command} />
            </DetailSection>
          )}
          {report?.installed && report.status !== "ready" && !report.login_command && report.message && (
            <DetailSection title="Problem">
              <p className="text-body-sm text-muted-foreground">{report.message}</p>
            </DetailSection>
          )}
          {id === "codex" && !isRemoteClient() && <ChatGptAccounts />}
          {ready && (
            <DetailSection title="Models">
              {models.length ? (
                <div className="flex flex-wrap gap-1.5">
                  {models.slice(0, 12).map((m) => (
                    <span key={m.id} className="rounded-md border border-hairline bg-surface-1 px-2 py-0.5 text-label text-foreground">{m.label}</span>
                  ))}
                  {models.length > 12 && <span className="px-1 py-0.5 text-label text-muted-foreground">+{models.length - 12} more</span>}
                </div>
              ) : (
                <p className="text-body-sm text-muted-foreground">Loading models…</p>
              )}
            </DetailSection>
          )}
        </>
      )}
    </div>
  );
}

/** Codex's ChatGPT plan sign-ins: every saved account, which one is in use,
 *  and switching or adding another. */
function ChatGptAccounts() {
  const status = useChatGptStore((s) => s.status);
  const busy = useChatGptStore((s) => s.busy);
  const error = useChatGptStore((s) => s.error);
  const start = useChatGptStore((s) => s.start);
  const cancel = useChatGptStore((s) => s.cancel);
  const disconnect = useChatGptStore((s) => s.disconnect);
  const pending = status?.phase === "pending";
  const connected = status?.phase === "connected";
  const profiles = status?.profiles ?? [];
  // The connection is the authority: a connected account the saved list
  // does not (yet) include still shows, with its email.
  const saved =
    connected && !profiles.some((p) => p.id === status.activeProfileId)
      ? [{ id: status.activeProfileId ?? "active", label: "ChatGPT plan", email: status.email, connected: true }, ...profiles]
      : profiles;
  const active = (id: string) =>
    connected && (status.activeProfileId ?? "active") === id;
  return (
    <DetailSection
      title="ChatGPT accounts"
      description="Use a ChatGPT plan for Codex instead of the CLI's own sign-in."
      action={
        !pending && (
          <Button variant="ghost" size="xs" disabled={busy} onClick={() => void start(null)}>
            <Plus />
            {saved.length ? "Add account" : "Connect ChatGPT"}
          </Button>
        )
      }
    >
      <div className="space-y-2">
        {pending && (
          <DetailRow>
            <Spinner className="size-3.5" aria-hidden />
            <span className="flex-1 text-body-sm">Finish signing in in your browser.</span>
            <Button variant="ghost" size="xs" disabled={busy} onClick={() => void cancel()}>Cancel</Button>
          </DetailRow>
        )}
        {saved.map((profile) => (
          <DetailRow key={profile.id}>
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-2 text-body font-medium">
                <span className="truncate">{profile.email ?? "ChatGPT account"}</span>
                {active(profile.id) && <span className="rounded-full border border-border px-2 text-label font-normal text-muted-foreground">In use</span>}
              </div>
              <div className="truncate text-label text-muted-foreground">{profile.label}</div>
            </div>
            {active(profile.id) ? (
              <>
                <Button variant="ghost" size="xs" onClick={() => void openUrl(CHATGPT_USAGE_URL)}>Usage <ExternalLink /></Button>
                <Button variant="ghost" size="xs" disabled={busy} onClick={() => void disconnect()}>Disconnect</Button>
              </>
            ) : (
              <Button variant="ghost" size="xs" disabled={busy || pending} onClick={() => void start(profile.id)}>Switch to</Button>
            )}
          </DetailRow>
        ))}
        {!saved.length && !pending && (
          <p className="text-body-sm text-muted-foreground">No ChatGPT account connected. Codex uses its own CLI sign-in.</p>
        )}
        {(error || status?.error) && <p role="alert" className="text-body-sm text-destructive">{error ?? status?.error}</p>}
      </div>
    </DetailSection>
  );
}
