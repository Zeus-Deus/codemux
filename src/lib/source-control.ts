/**
 * Per-provider presentation: the words, sigils, icons and CLI names the
 * UI needs to talk about a checkout's hosting product.
 *
 * The backend classifies a checkout and puts the answer on the workspace
 * snapshot as `provider_kind` (see `src-tauri/src/git_provider/detect.rs`).
 * This module is the single place that turns that string into copy, so a
 * third product only needs a row added here rather than a sweep through
 * every component that says "PR".
 *
 * Two rules keep the sweep safe:
 *
 * - **Absent means GitHub.** Snapshots written before detection existed,
 *   and hosts detection cannot classify, have no `provider_kind`. Those
 *   used to render GitHub wording and must keep rendering it, so
 *   `resolveProvider(undefined)` is the GitHub row. Every existing GitHub
 *   surface therefore resolves to exactly the string it had before.
 * - **Unknown is neutral, not wrong.** A positively-detected product
 *   Codemux has no adapter for gets generic wording ("change request")
 *   instead of borrowed GitHub nouns.
 */

import {
  GitBranch,
  Github,
  Gitlab,
  type LucideIcon,
} from "lucide-react";

/** Wire values of `ProviderKind`, mirroring the Rust enum's serde renames. */
export type ProviderKind =
  | "github"
  | "gitlab"
  | "bitbucket"
  | "azure_devops"
  | "unknown";

export interface ProviderPresentation {
  kind: ProviderKind;
  /** Product name as the vendor writes it — "GitHub", "Azure DevOps". */
  name: string;
  /** Abbreviation used in chips and dense rows: "PR" / "MR". */
  shortNoun: string;
  /** Lowercase prose noun: "pull request" / "merge request". */
  noun: string;
  /** Sentence-initial / label form: "Pull request". */
  nounTitle: string;
  /** Title-cased, for button labels: "Pull Request". Kept distinct from
   *  `nounTitle` so existing GitHub buttons ("Create Pull Request") stay
   *  byte-identical rather than drifting to sentence case. */
  nounTitleCase: string;
  /** Lowercase plural: "pull requests". */
  nounPlural: string;
  /** Reference sigil: `#` on GitHub, `!` on GitLab. */
  sigil: string;
  /** Bare CLI binary — "gh" / "glab". `null` when Codemux has no adapter. */
  cli: string | null;
  /** How the CLI is named in prose: "GitHub CLI (gh)". */
  cliLabel: string | null;
  /** Command that authenticates the CLI: "gh auth login". */
  loginCommand: string | null;
  /** Where to get the CLI, without a scheme so it reads as prose. */
  installUrl: string | null;
  /** Hosts that *are* the product, so naming them alongside it would be
   *  redundant ("GitHub · github.com"). Empty for products with no
   *  hosted offering Codemux knows about. */
  canonicalHosts: readonly string[];
  /** Brand-ish glyph for chips and settings rows. */
  Icon: LucideIcon;
  /** Does Codemux have a working integration for this product? */
  supported: boolean;
}

const GITHUB: ProviderPresentation = {
  kind: "github",
  name: "GitHub",
  shortNoun: "PR",
  noun: "pull request",
  nounTitle: "Pull request",
  nounTitleCase: "Pull Request",
  nounPlural: "pull requests",
  sigil: "#",
  cli: "gh",
  cliLabel: "GitHub CLI (gh)",
  loginCommand: "gh auth login",
  installUrl: "cli.github.com",
  canonicalHosts: ["github.com"],
  Icon: Github,
  supported: true,
};

const GITLAB: ProviderPresentation = {
  kind: "gitlab",
  name: "GitLab",
  shortNoun: "MR",
  noun: "merge request",
  nounTitle: "Merge request",
  nounTitleCase: "Merge Request",
  nounPlural: "merge requests",
  sigil: "!",
  cli: "glab",
  cliLabel: "GitLab CLI (glab)",
  loginCommand: "glab auth login",
  installUrl: "gitlab.com/gitlab-org/cli",
  canonicalHosts: ["gitlab.com"],
  Icon: Gitlab,
  supported: true,
};

/** Shared shape for a detected-but-unimplemented product. */
function unimplemented(
  kind: ProviderKind,
  name: string,
  canonicalHosts: readonly string[] = [],
): ProviderPresentation {
  return {
    kind,
    name,
    // Neutral vocabulary: borrowing GitHub's nouns for a product that
    // does not use them would be a lie, and Codemux cannot act on these
    // anyway, so the copy only ever appears in "not supported" states.
    shortNoun: "CR",
    noun: "change request",
    nounTitle: "Change request",
    nounTitleCase: "Change Request",
    nounPlural: "change requests",
    sigil: "#",
    cli: null,
    cliLabel: null,
    loginCommand: null,
    installUrl: null,
    canonicalHosts,
    Icon: GitBranch,
    supported: false,
  };
}

const BITBUCKET = unimplemented("bitbucket", "Bitbucket", ["bitbucket.org"]);
const AZURE_DEVOPS = unimplemented("azure_devops", "Azure DevOps", [
  "dev.azure.com",
]);

/** A host detection could not classify. Named generically because there
 *  is no product to name. */
const UNKNOWN: ProviderPresentation = {
  ...unimplemented("unknown", "Unknown provider"),
  kind: "unknown",
};

const BY_KIND: Record<ProviderKind, ProviderPresentation> = {
  github: GITHUB,
  gitlab: GITLAB,
  bitbucket: BITBUCKET,
  azure_devops: AZURE_DEVOPS,
  unknown: UNKNOWN,
};

/** Every product Codemux knows how to name, in settings-display order.
 *  Supported ones first so the diagnostics list leads with what works. */
export const ALL_PROVIDER_KINDS: ProviderKind[] = [
  "github",
  "gitlab",
  "bitbucket",
  "azure_devops",
];

/** Products a user may classify a self-hosted instance as. */
export const CUSTOM_HOST_KINDS: ProviderKind[] = ["github", "gitlab"];

/**
 * Presentation for a snapshot's `provider_kind`.
 *
 * Absent (`null`/`undefined`/empty) resolves to GitHub — see the
 * back-compat rule in the module docs. A value that is *present* but
 * unrecognised is the opposite situation: detection classified something
 * this build has no row for, most likely a product added by a newer
 * backend. Calling that GitHub would put GitHub's nouns and its `gh`
 * instructions on a checkout GitHub does not serve, so it gets the
 * neutral row instead — the same one a literal `"unknown"` gets.
 */
export function resolveProvider(
  kind: string | null | undefined,
): ProviderPresentation {
  if (kind == null) return GITHUB;
  const normalized = kind.trim().toLowerCase();
  if (normalized === "") return GITHUB;
  return BY_KIND[normalized as ProviderKind] ?? UNKNOWN;
}

/** Presentation for a workspace-shaped object. Accepts anything with the
 *  field so callers do not have to import `WorkspaceSnapshot`. */
export function providerForWorkspace(
  workspace: { provider_kind?: string | null } | null | undefined,
): ProviderPresentation {
  return resolveProvider(workspace?.provider_kind);
}

/** `#172` on GitHub, `!12` on GitLab. */
export function providerRef(
  provider: ProviderPresentation,
  number: number | null | undefined,
): string {
  return number == null ? "" : `${provider.sigil}${number}`;
}

/** "PR #172" / "MR !12", falling back to the bare noun when unnumbered. */
export function providerRefLabel(
  provider: ProviderPresentation,
  number: number | null | undefined,
): string {
  return number == null
    ? provider.nounTitle
    : `${provider.shortNoun} ${providerRef(provider, number)}`;
}

/**
 * The product's list page, derived from a single change request's URL.
 *
 * Used by the "View all" affordance, which only has a row's URL to go
 * on. Each product's detail path is rewritten to its index path;
 * anything that does not match is returned unchanged rather than
 * mangled, so an unexpected URL still opens *somewhere* real.
 */
export function changeRequestListUrl(
  provider: ProviderPresentation,
  url: string | null | undefined,
): string | null {
  if (!url) return null;
  switch (provider.kind) {
    case "gitlab":
      return url.replace(/\/-\/merge_requests\/\d+$/, "/-/merge_requests");
    case "github":
      return url.replace(/\/pull\/\d+$/, "/pulls");
    default:
      return url;
  }
}

/** "GitLab · gitlab.example.com" — the subtle detected-provider line.
 *  Host omitted when it adds nothing (the product's own domain). */
export function providerHostLabel(
  provider: ProviderPresentation,
  host: string | null | undefined,
): string {
  if (!host) return provider.name;
  const bare = host.trim().toLowerCase();
  if (bare === "" || provider.canonicalHosts.includes(bare)) {
    return provider.name;
  }
  return `${provider.name} · ${bare}`;
}
