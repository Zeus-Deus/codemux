import { describe, expect, it } from "vitest";

import {
  ALL_PROVIDER_KINDS,
  CUSTOM_HOST_KINDS,
  changeRequestListUrl,
  providerForWorkspace,
  providerHostLabel,
  providerRef,
  providerRefLabel,
  resolveProvider,
  unsupportedRepoMessage,
  type ProviderKind,
} from "./source-control";

describe("resolveProvider", () => {
  // The back-compat contract the whole copy sweep rests on: a snapshot
  // written before detection existed renders exactly what it rendered
  // before. If this drifts, every GitHub surface silently changes words.
  it("treats an absent provider as GitHub", () => {
    for (const absent of [null, undefined, "", "   "]) {
      const p = resolveProvider(absent);
      expect(p.kind).toBe("github");
      expect(p.shortNoun).toBe("PR");
      expect(p.sigil).toBe("#");
      expect(p.loginCommand).toBe("gh auth login");
    }
  });

  it("gives a present-but-unrecognised kind the neutral row, not GitHub", () => {
    // A newer backend could name a product this build has never heard
    // of. Borrowing GitHub's nouns for it would tell the user to run
    // `gh` against a host gh does not serve; the neutral row says
    // nothing it cannot back up.
    expect(resolveProvider("gitea").kind).toBe("unknown");
    expect(resolveProvider("!!!").kind).toBe("unknown");
    expect(resolveProvider("gitea").supported).toBe(false);
    expect(resolveProvider("gitea").cli).toBeNull();
  });

  it("still treats an absent kind as GitHub, for snapshots written before detection", () => {
    expect(resolveProvider(null).kind).toBe("github");
    expect(resolveProvider(undefined).kind).toBe("github");
    expect(resolveProvider("  ").kind).toBe("github");
  });

  it("is case- and whitespace-insensitive", () => {
    expect(resolveProvider("GitLab").kind).toBe("gitlab");
    expect(resolveProvider("  gitlab  ").kind).toBe("gitlab");
    expect(resolveProvider("AZURE_DEVOPS").kind).toBe("azure_devops");
  });

  it("gives GitLab merge-request vocabulary and the glab CLI", () => {
    const p = resolveProvider("gitlab");
    expect(p.name).toBe("GitLab");
    expect(p.shortNoun).toBe("MR");
    expect(p.noun).toBe("merge request");
    expect(p.nounTitle).toBe("Merge request");
    expect(p.nounTitleCase).toBe("Merge Request");
    expect(p.nounPlural).toBe("merge requests");
    expect(p.sigil).toBe("!");
    expect(p.cli).toBe("glab");
    expect(p.cliLabel).toBe("GitLab CLI (glab)");
    expect(p.loginCommand).toBe("glab auth login");
    expect(p.installUrl).toBe("gitlab.com/gitlab-org/cli");
    expect(p.supported).toBe(true);
  });

  it("gives an unimplemented product neutral wording and no CLI", () => {
    // Borrowing GitHub's nouns for Bitbucket would be a lie, and there
    // is no command to suggest, so every CLI field must be null rather
    // than a plausible-looking guess.
    for (const kind of ["bitbucket", "azure_devops", "unknown"] as const) {
      const p = resolveProvider(kind);
      expect(p.supported).toBe(false);
      expect(p.noun).toBe("change request");
      expect(p.cli).toBeNull();
      expect(p.cliLabel).toBeNull();
      expect(p.loginCommand).toBeNull();
      expect(p.installUrl).toBeNull();
    }
    expect(resolveProvider("bitbucket").name).toBe("Bitbucket");
    expect(resolveProvider("azure_devops").name).toBe("Azure DevOps");
  });

  it("returns a stable singleton per kind", () => {
    // Components pass the result straight into effect deps and memo'd
    // rows; a fresh object each call would defeat both.
    expect(resolveProvider("gitlab")).toBe(resolveProvider("gitlab"));
    expect(resolveProvider(null)).toBe(resolveProvider("github"));
  });

  it("exposes an icon component for every kind", () => {
    for (const kind of [...ALL_PROVIDER_KINDS, "unknown" as ProviderKind]) {
      expect(resolveProvider(kind).Icon).toBeTruthy();
    }
  });

  it("only offers implemented products as custom-host classifications", () => {
    // Letting someone tag a server as Bitbucket would produce a
    // detection result Codemux then refuses to act on.
    for (const kind of CUSTOM_HOST_KINDS) {
      expect(resolveProvider(kind).supported).toBe(true);
    }
  });
});

describe("providerForWorkspace", () => {
  it("reads provider_kind off a workspace-shaped object", () => {
    expect(providerForWorkspace({ provider_kind: "gitlab" }).kind).toBe(
      "gitlab",
    );
  });

  it("survives a missing field, a null workspace, and undefined", () => {
    expect(providerForWorkspace({}).kind).toBe("github");
    expect(providerForWorkspace(null).kind).toBe("github");
    expect(providerForWorkspace(undefined).kind).toBe("github");
  });
});

describe("providerRef", () => {
  it("uses each product's sigil", () => {
    expect(providerRef(resolveProvider("github"), 172)).toBe("#172");
    expect(providerRef(resolveProvider("gitlab"), 12)).toBe("!12");
  });

  it("renders nothing for a missing number", () => {
    // Chips render this inline; "#null" would be worse than an empty
    // span, and callers already gate on the number elsewhere.
    expect(providerRef(resolveProvider("github"), null)).toBe("");
    expect(providerRef(resolveProvider("github"), undefined)).toBe("");
  });

  it("treats zero as a real number rather than absent", () => {
    expect(providerRef(resolveProvider("github"), 0)).toBe("#0");
  });
});

describe("providerRefLabel", () => {
  it("pairs the abbreviation with the reference", () => {
    expect(providerRefLabel(resolveProvider("github"), 5)).toBe("PR #5");
    expect(providerRefLabel(resolveProvider("gitlab"), 5)).toBe("MR !5");
  });

  it("degrades to the bare noun when unnumbered", () => {
    expect(providerRefLabel(resolveProvider("gitlab"), null)).toBe(
      "Merge request",
    );
  });
});

describe("changeRequestListUrl", () => {
  it("rewrites a GitHub detail URL to the list page", () => {
    expect(
      changeRequestListUrl(
        resolveProvider("github"),
        "https://github.com/acme/app/pull/7",
      ),
    ).toBe("https://github.com/acme/app/pulls");
  });

  it("rewrites a GitLab detail URL to its own list path", () => {
    // The bug this exists to prevent: GitLab's path is
    // `/-/merge_requests/N`, which the GitHub regex leaves untouched,
    // so "View all" used to open a single merge request.
    expect(
      changeRequestListUrl(
        resolveProvider("gitlab"),
        "https://gitlab.com/acme/app/-/merge_requests/12",
      ),
    ).toBe("https://gitlab.com/acme/app/-/merge_requests");
  });

  it("handles a nested GitLab group path", () => {
    expect(
      changeRequestListUrl(
        resolveProvider("gitlab"),
        "https://gitlab.com/acme/team/app/-/merge_requests/3",
      ),
    ).toBe("https://gitlab.com/acme/team/app/-/merge_requests");
  });

  it("returns a non-matching URL unchanged rather than mangling it", () => {
    const odd = "https://example.test/something/else";
    expect(changeRequestListUrl(resolveProvider("github"), odd)).toBe(odd);
    expect(changeRequestListUrl(resolveProvider("gitlab"), odd)).toBe(odd);
  });

  it("does not apply one product's rewrite to another's URL", () => {
    // Cross-contamination check: a GitLab URL passed with the GitHub
    // presentation must not be half-rewritten.
    const gitlabUrl = "https://gitlab.com/acme/app/-/merge_requests/12";
    expect(changeRequestListUrl(resolveProvider("github"), gitlabUrl)).toBe(
      gitlabUrl,
    );
  });

  it("is null-safe", () => {
    expect(changeRequestListUrl(resolveProvider("github"), null)).toBeNull();
    expect(
      changeRequestListUrl(resolveProvider("github"), undefined),
    ).toBeNull();
  });
});

describe("providerHostLabel", () => {
  it("names a self-hosted instance alongside the product", () => {
    expect(
      providerHostLabel(resolveProvider("gitlab"), "gitlab.acme.com"),
    ).toBe("GitLab · gitlab.acme.com");
  });

  it("omits the product's own domain as redundant", () => {
    expect(providerHostLabel(resolveProvider("gitlab"), "gitlab.com")).toBe(
      "GitLab",
    );
    expect(providerHostLabel(resolveProvider("github"), "github.com")).toBe(
      "GitHub",
    );
  });

  it("falls back to the bare product name without a host", () => {
    expect(providerHostLabel(resolveProvider("github"), null)).toBe("GitHub");
    expect(providerHostLabel(resolveProvider("github"), "  ")).toBe("GitHub");
  });
});

describe("unsupportedRepoMessage", () => {
  it("says the checkout is not that product's when the product is served", () => {
    // A supported product reaching this state means the checkout simply
    // is not one — not that anything is missing from the machine.
    expect(unsupportedRepoMessage(resolveProvider("github"))).toBe(
      "Not a GitHub repository",
    );
    expect(unsupportedRepoMessage(resolveProvider("gitlab"))).toBe(
      "Not a GitLab repository",
    );
  });

  it("owns the gap for a product Codemux recognises but cannot serve", () => {
    expect(unsupportedRepoMessage(resolveProvider("bitbucket"))).toBe(
      "Codemux has no Bitbucket integration yet.",
    );
  });

  it("stays generic when there is no product to name", () => {
    expect(unsupportedRepoMessage(resolveProvider("unknown"))).toBe(
      "No supported source control host for this repository",
    );
  });
});
