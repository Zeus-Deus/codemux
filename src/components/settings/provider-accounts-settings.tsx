import { useEffect, useState } from "react";
import { MoreHorizontal, Plus } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { useConfirm } from "@/components/ui/confirm-dialog";
import { isRemoteClient } from "@/components/remote/is-remote-client";
import { cn } from "@/lib/utils";
import { toast } from "@/lib/toast";
import { signInToProvider } from "@/lib/agent-chat/provider-sign-in";
import {
  accountName,
  accountsFor,
  DEFAULT_ACCOUNT_ID,
  useProviderAccounts,
  type AccountProvider,
} from "@/stores/provider-accounts-store";
import { useUIStore } from "@/stores/ui-store";
import {
  activateWorkspace,
  getOrCreateHomeWorkspace,
  providerAccountOpenSignIn,
  type ProviderAccount,
} from "@/tauri/commands";
import { CommandRow, DetailRow, DetailSection } from "./provider-parts";

const PROVIDER_NAME: Record<AccountProvider, string> = { claude: "Claude", codex: "Codex" };

/** Open a terminal that signs `account` in, and bring it into view. */
async function signIn(account: ProviderAccount) {
  if (account.id === DEFAULT_ACCOUNT_ID) {
    await signInToProvider(account.provider, account.login_command, null);
    useUIStore.getState().setShowSettings(false);
    return;
  }
  try {
    const workspace = await getOrCreateHomeWorkspace();
    await providerAccountOpenSignIn(workspace, account.id);
    await activateWorkspace(workspace);
    useUIStore.getState().setShowSettings(false);
  } catch (error) {
    toast.failure("Couldn't open a terminal to sign in", error);
  }
}

function initials(account: ProviderAccount): string {
  const source = account.label ?? account.identity.email ?? "D";
  return source.trim().charAt(0).toUpperCase() || "?";
}

/**
 * Settings → Providers → Claude / Codex: every account the provider can
 * sign in to. The CLI's own sign-in is always there as Default; each extra
 * account gets its own CLI home, so it keeps its own login, plan limits,
 * and history while sharing settings, skills, and instructions.
 */
export function ProviderAccountsSettings({ provider }: { provider: AccountProvider }) {
  const all = useProviderAccounts((s) => s.accounts);
  const defaults = useProviderAccounts((s) => s.defaults);
  const accounts = accountsFor(all, provider);
  const appDefault = defaults[provider] ?? DEFAULT_ACCOUNT_ID;
  const [adding, setAdding] = useState(false);
  const [renaming, setRenaming] = useState<string | null>(null);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const { confirm, dialog } = useConfirm();
  const remote = isRemoteClient();

  useEffect(() => {
    const store = useProviderAccounts.getState();
    void store
      .load(true)
      .then(() => store.refresh())
      .catch((error: unknown) => setLoadError(error instanceof Error ? error.message : String(error)));
  }, []);

  const run = async (label: string, action: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await action();
      return true;
    } catch (error) {
      toast.failure(label, error);
      return false;
    } finally {
      setBusy(false);
    }
  };

  const submitAdd = async () => {
    const ok = await run("Couldn't add the account", () => useProviderAccounts.getState().add(provider, name));
    if (ok) {
      setAdding(false);
      setName("");
    }
  };
  const submitRename = async (account: ProviderAccount) => {
    const ok = await run("Couldn't rename the account", () =>
      useProviderAccounts.getState().rename(account.id, name),
    );
    if (ok) setRenaming(null);
  };
  const remove = async (account: ProviderAccount) => {
    const yes = await confirm({
      title: `Remove ${accountName(account)}?`,
      description:
        "This signs the account out of Codemux and deletes its folder, including the provider's own history for chats run on it. Transcripts in Codemux and usage already counted stay.",
      confirmLabel: "Remove account",
      destructive: true,
    });
    if (yes) await run("Couldn't remove the account", () => useProviderAccounts.getState().remove(account.id));
  };

  return (
    <DetailSection
      title="Accounts"
      description={`Sign in to more than one ${PROVIDER_NAME[provider]} account. Each chat runs on one; switch from the composer when one hits its limit.`}
      action={
        !adding &&
        !remote && (
          <Button variant="ghost" size="xs" disabled={busy || all === null} onClick={() => { setAdding(true); setName(""); }}>
            <Plus />
            Add account
          </Button>
        )
      }
    >
      <div className="space-y-2">
        {all === null && !loadError && <p className="text-body-sm text-muted-foreground">Loading accounts…</p>}
        {all === null && loadError && (
          <p role="alert" className="text-body-sm text-destructive">Couldn't load accounts: {loadError}</p>
        )}
        {accounts.map((account) => {
          const isDefault = account.id === appDefault;
          const signedOut = account.identity.signed_in === false;
          const extra = account.id !== DEFAULT_ACCOUNT_ID;
          if (renaming === account.id) {
            return (
              <DetailRow key={account.id}>
                <form
                  className="flex flex-1 items-center gap-2"
                  onSubmit={(e) => { e.preventDefault(); void submitRename(account); }}
                >
                  <Input autoFocus value={name} maxLength={40} aria-label="Account name" onChange={(e) => setName(e.target.value)} className="h-7 flex-1" />
                  <Button type="submit" size="xs" disabled={busy || !name.trim()}>Save</Button>
                  <Button type="button" variant="ghost" size="xs" onClick={() => setRenaming(null)}>Cancel</Button>
                </form>
              </DetailRow>
            );
          }
          return (
            <div key={account.id} className="space-y-1.5">
              <DetailRow>
                <span
                  aria-hidden
                  className={cn(
                    "flex size-7 shrink-0 items-center justify-center rounded-full border border-hairline bg-surface-2 text-label font-medium",
                    signedOut ? "text-muted-foreground" : "text-foreground",
                  )}
                >
                  {initials(account)}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex items-center gap-2 text-body font-medium">
                    <span className="truncate">{accountName(account)}</span>
                    {isDefault && (
                      <span className="rounded-full border border-border px-2 text-label font-normal text-muted-foreground">Default for new chats</span>
                    )}
                  </div>
                  <div className={cn("truncate text-label", signedOut ? "text-status-attention" : "text-muted-foreground")}>
                    {signedOut
                      ? "Not signed in"
                      : [account.identity.email, account.identity.plan, account.identity.organization]
                          .filter(Boolean)
                          .join(" · ") || (extra ? "Signed in" : `The ${PROVIDER_NAME[provider]} CLI's own sign-in`)}
                  </div>
                </div>
                {!remote && signedOut && (
                  <Button variant="outline" size="xs" disabled={busy} onClick={() => void signIn(account)}>Sign in</Button>
                )}
                {!isDefault && !remote && (
                  <Button
                    variant="ghost"
                    size="xs"
                    disabled={busy}
                    onClick={() => void run("Couldn't change the default", () => useProviderAccounts.getState().setDefault(provider, account.id))}
                  >
                    Make default
                  </Button>
                )}
                {extra && !remote && (
                  <DropdownMenu>
                    <DropdownMenuTrigger asChild>
                      <Button variant="ghost" size="icon-xs" aria-label={`More for ${accountName(account)}`} disabled={busy}>
                        <MoreHorizontal />
                      </Button>
                    </DropdownMenuTrigger>
                    <DropdownMenuContent align="end">
                      <DropdownMenuItem onSelect={() => { setRenaming(account.id); setName(account.label ?? ""); }}>Rename</DropdownMenuItem>
                      <DropdownMenuItem onSelect={() => void signIn(account)}>{signedOut ? "Sign in" : "Sign in again"}</DropdownMenuItem>
                      <DropdownMenuSeparator />
                      <DropdownMenuItem variant="destructive" onSelect={() => void remove(account)}>Remove…</DropdownMenuItem>
                    </DropdownMenuContent>
                  </DropdownMenu>
                )}
              </DetailRow>
              {extra && signedOut && <CommandRow command={account.login_command} />}
            </div>
          );
        })}
        {adding && (
          <DetailRow>
            <form className="flex flex-1 items-center gap-2" onSubmit={(e) => { e.preventDefault(); void submitAdd(); }}>
              <Input
                autoFocus
                value={name}
                maxLength={40}
                placeholder="Account name, e.g. Work"
                aria-label="Account name"
                onChange={(e) => setName(e.target.value)}
                className="h-7 flex-1"
              />
              <Button type="submit" size="xs" disabled={busy || !name.trim()}>Add</Button>
              <Button type="button" variant="ghost" size="xs" onClick={() => setAdding(false)}>Cancel</Button>
            </form>
          </DetailRow>
        )}
        {accounts.length > 1 && (
          <p className="pt-1 text-label text-muted-foreground">
            Extra accounts share your {PROVIDER_NAME[provider]} settings, MCP servers, skills, and instructions. Each keeps its own sign-in, limits, and history.
          </p>
        )}
      </div>
      {dialog}
    </DetailSection>
  );
}
