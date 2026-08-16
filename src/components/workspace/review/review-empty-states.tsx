import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { toast } from "@/lib/toast";
import { btnCard, btnEmberSolid, plural } from "./review-ui";
import type { ProviderPresentation } from "@/lib/source-control";

/**
 * Five different nothings.
 *
 * The pane already gates on auth and repo before it renders anything, so
 * these are refinements of a gate that exists — but each one is a
 * different sentence and a different next step, and none of them is an
 * empty panel. Every action here does real work; a state whose next step
 * we can't perform gets a sentence and the browser, never a dead button
 * (binding rule 5).
 */

interface EmptyStateProps {
  title: string;
  body: ReactNode;
  actions: ReactNode;
  testId: string;
}

function EmptyState({ title, body, actions, testId }: EmptyStateProps) {
  return (
    <div className="flex flex-col gap-2.5 px-3.5 py-4" data-testid={testId}>
      <span className="text-[12px] font-semibold text-foreground">{title}</span>
      <p className="text-[11px] leading-relaxed text-foreground/75">{body}</p>
      <div className="flex flex-wrap items-center gap-1.5">{actions}</div>
    </div>
  );
}

const Mono = ({ children }: { children: ReactNode }) => (
  <span className="font-mono text-[10.5px]">{children}</span>
);

/** No PR for a branch that is already pushed. */
export function NoPullRequestState({
  branch,
  baseBranch,
  commitsAhead,
  provider,
  onCreate,
  onViewCommits,
}: {
  branch: string | null;
  baseBranch: string;
  commitsAhead: number;
  provider: ProviderPresentation;
  onCreate: () => void;
  onViewCommits: () => void;
}) {
  return (
    <EmptyState
      testId="empty-no-pr"
      title={`No ${provider.noun} yet`}
      body={
        <>
          <Mono>{branch ?? "This branch"}</Mono> is pushed
          {commitsAhead > 0 && (
            <>
              {" "}
              and {plural(commitsAhead, "commit")} ahead of <Mono>{baseBranch}</Mono>
            </>
          )}
          .
        </>
      }
      actions={
        <>
          <button type="button" className={btnEmberSolid} onClick={onCreate}>
            Open a {provider.noun}
          </button>
          <button type="button" className={btnCard} onClick={onViewCommits}>
            View commits
          </button>
        </>
      }
    />
  );
}

/** The branch exists only on this machine. */
export function BranchLocalOnlyState({
  changedFiles,
  onCommitAndPush,
  onOpenChanges,
  pushOnly,
}: {
  changedFiles: number;
  onCommitAndPush: () => void;
  onOpenChanges: () => void;
  /** Nothing to commit — the branch just needs pushing, so the primary
   *  says so instead of sending you to a Changes pane with no work. */
  pushOnly: boolean;
}) {
  return (
    <EmptyState
      testId="empty-local-only"
      title="Nothing pushed yet"
      body={
        <>
          This branch exists only on this machine.
          {changedFiles > 0 && <> {plural(changedFiles, "file")} have uncommitted changes.</>}
        </>
      }
      actions={
        <>
          <button type="button" className={btnEmberSolid} onClick={onCommitAndPush}>
            {pushOnly ? "Push branch" : "Commit and push"}
          </button>
          <button type="button" className={btnCard} onClick={onOpenChanges}>
            Changes
          </button>
        </>
      }
    />
  );
}

/** The host CLI is installed but signed out. */
export function SignedOutState({ provider }: { provider: ProviderPresentation }) {
  const command = provider.loginCommand;
  const copyCommand = () => {
    if (!command) return;
    navigator.clipboard
      .writeText(command)
      .then(() => toast.success(`Copied — run \`${command}\` in any terminal`))
      .catch(() => toast.error("Couldn't copy the command"));
  };
  return (
    <EmptyState
      testId="empty-signed-out"
      title={`Sign in to ${provider.name}`}
      body="Reviewing needs a signed-in host. Everything else in this workspace keeps working."
      actions={
        command ? (
          <>
            {/* Signing in happens in a terminal, so the button hands you
                the command rather than pretending to open a dialog. */}
            <button type="button" className={btnEmberSolid} onClick={copyCommand}>
              Sign in
            </button>
            <Mono>{command}</Mono>
          </>
        ) : null
      }
    />
  );
}

/** The CLI is missing entirely. */
export function CliMissingState({ provider }: { provider: ProviderPresentation }) {
  return (
    <EmptyState
      testId="empty-cli-missing"
      title={`${provider.cliLabel ?? "The host CLI"} isn't installed`}
      body={
        <>
          Codemux talks to {provider.name} through <Mono>{provider.cli}</Mono>. Everything
          else in this workspace keeps working.
        </>
      }
      actions={
        provider.installUrl ? (
          <button
            type="button"
            className={btnEmberSolid}
            onClick={() => {
              // `installUrl` is stored without a scheme so it reads as
              // prose next to the CLI name.
              const url = /^https?:\/\//.test(provider.installUrl!)
                ? provider.installUrl!
                : `https://${provider.installUrl}`;
              openUrl(url).catch((err) => toast.error(String(err)));
            }}
          >
            Install {provider.cli}
          </button>
        ) : null
      }
    />
  );
}

/** Authenticated, but this repository can't be seen. */
export function RepoUnreachableState({
  repoSlug,
  provider,
  onRetry,
}: {
  repoSlug: string | null;
  provider: ProviderPresentation;
  onRetry: () => void;
}) {
  return (
    <EmptyState
      testId="empty-unreachable"
      title="Can't reach this repository"
      body={
        <>
          Your account can't see {repoSlug ? <Mono>{repoSlug}</Mono> : "this repository"}. It
          may be private, or the token may lack <Mono>repo</Mono> scope.
        </>
      }
      actions={
        <>
          <button type="button" className={btnEmberSolid} onClick={onRetry}>
            Retry
          </button>
          <Mono>{provider.cli} auth status</Mono>
        </>
      }
    />
  );
}

/**
 * A host Codemux has no adapter for.
 *
 * Two shapes, because two different things are true. When detection
 * named a product ("Bitbucket"), say so — the user knows what they're
 * on and wants to know when it lands. When it named nothing, or named a
 * product we *do* support but the checkout isn't served by it, don't
 * borrow a vendor's name for a repository it has nothing to do with.
 */
export function UnsupportedHostState({
  provider,
  url,
}: {
  provider: ProviderPresentation;
  url: string | null;
}) {
  const namedProduct = !provider.supported && provider.kind !== "unknown";
  return (
    <EmptyState
      testId="empty-unsupported"
      title={
        namedProduct
          ? `${provider.name}, read-only`
          : "No supported source control host for this repository"
      }
      body={
        namedProduct
          ? "Codemux can see this repository but can't review or merge it here yet."
          : "Codemux reviews and merges on GitHub and GitLab. Everything else in this workspace keeps working."
      }
      actions={
        // Unsupported is a sentence and an offer of the browser — never
        // a disabled control (binding rule 5).
        url ? (
          <button
            type="button"
            className={btnCard}
            onClick={() => {
              openUrl(url).catch((err) => toast.error(String(err)));
            }}
          >
            Open in browser
          </button>
        ) : null
      }
    />
  );
}
