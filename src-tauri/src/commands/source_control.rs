//! Diagnostics for the Settings → Source Control pane.
//!
//! Answers one question per hosting product: *is the tooling Codemux
//! needs actually here and logged in?* The pane is the place a user
//! lands when PR/MR UI is missing, so the probe has to distinguish "no
//! CLI" from "CLI but signed out" — the two have different fixes.
//!
//! Three properties matter:
//!
//! - **Per-provider isolation.** One product's probe wedging or failing
//!   must not blank the pane. Every entry is built independently and a
//!   failure degrades to "not installed" rather than an `Err`, so the
//!   command itself is infallible.
//! - **Bounded.** Both probes shell out. `--version` gets a short
//!   deadline and is killed past it; the auth probes reuse the adapters'
//!   own implementations, which already carry the keyring-aware env
//!   handling the desktop needs.
//! - **No secrets.** The only strings that escape are a version line and
//!   an account name. Nothing here reads a token, and the version output
//!   is truncated to its first line so a chatty CLI cannot smuggle
//!   environment detail into the UI.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::execution::{host_command, sanitize_gui_env_std};
use crate::git_provider::exec::run_timed;
use crate::git_provider::{
    self, Capabilities, DetectedProvider, GitLabProvider, ProviderKind, SourceControlProvider,
};
use crate::github::GhStatus;

/// How long a `--version` call may take before it is killed. Generous
/// enough for a cold binary on a slow disk, short enough that opening
/// Settings never feels stalled.
const VERSION_TIMEOUT: Duration = Duration::from_secs(4);

/// Upper bound on the version string we surface. A version line is tens
/// of characters; anything longer is not a version and is not worth
/// rendering.
const MAX_VERSION_LEN: usize = 120;

/// One product's readiness, as the settings pane renders it.
///
/// Presentation (product name, CLI name, login command, install URL)
/// deliberately lives in the frontend map at `src/lib/source-control.ts`
/// rather than here: this struct carries probe *results* only, so there
/// is one source of truth for copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderDiagnostic {
    /// Wire form of [`ProviderKind`] — `"github"`, `"gitlab"`, …
    pub kind: String,
    /// Does Codemux ship an adapter for this product? False rows render
    /// as dimmed "not yet supported" entries.
    pub supported: bool,
    pub cli_installed: bool,
    /// First line of `<cli> --version`, trimmed. `None` when the CLI is
    /// missing or did not answer in time.
    pub cli_version: Option<String>,
    pub authenticated: bool,
    /// Account the CLI reports being logged in as. `None` when signed
    /// out, or when the CLI authenticated without naming an account.
    pub account: Option<String>,
    /// One sanitized sentence explaining a non-ready state, for the
    /// expandable detail. `None` when everything is fine.
    pub detail: Option<String>,
    /// What the adapter actually serves. Declared by the adapter rather
    /// than restated in the UI, so a product that gains (or is missing)
    /// a feature says so in one place. All-false on an unsupported row.
    pub capabilities: Capabilities,
}

impl ProviderDiagnostic {
    /// The row for a product with no adapter. Probing would be
    /// meaningless — there is no CLI Codemux would call.
    fn unsupported(kind: ProviderKind) -> Self {
        Self {
            kind: kind.as_str().to_string(),
            supported: false,
            cli_installed: false,
            cli_version: None,
            authenticated: false,
            account: None,
            detail: Some(format!(
                "Codemux has no {} integration yet.",
                kind.display_name()
            )),
            capabilities: Capabilities::default(),
        }
    }
}

/// Whether the product serving one checkout is usable right now.
///
/// The question every UI gate actually asks, answered once for the
/// provider that checkout resolves to — as opposed to
/// [`discover_source_control`], which answers it for every product at
/// once and is scoped to no instance in particular. The distinction
/// matters for self-hosted deployments: two GitLab instances are two
/// separate logins, and only a host-scoped probe can tell them apart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAuthStatus {
    /// Wire form of the *detected* [`ProviderKind`] — `"unknown"` when
    /// nothing classified, which is a real answer, not an error.
    pub kind: String,
    /// Does Codemux ship an adapter for this checkout's product?
    pub supported: bool,
    /// Is that adapter's CLI on PATH?
    pub installed: bool,
    pub authenticated: bool,
    /// Account the CLI reports. `None` when signed out, or when it
    /// authenticated without naming one.
    pub username: Option<String>,
}

impl ProviderAuthStatus {
    /// What a wedged probe degrades to: no product, nothing usable. The
    /// UI treats it exactly like an unclassified checkout.
    fn unavailable() -> Self {
        Self {
            kind: ProviderKind::Unknown.as_str().to_string(),
            supported: false,
            installed: false,
            authenticated: false,
            username: None,
        }
    }
}

/// Probe every product Codemux knows how to name.
///
/// Infallible by construction: the return type has no `Err` arm, and
/// each entry is independent.
#[tauri::command]
pub async fn discover_source_control() -> Vec<ProviderDiagnostic> {
    // Each supported product gets its own blocking task: the two probes
    // are independent subprocesses, and running them in sequence made
    // opening Settings cost the sum of two CLI round trips.
    let (github, gitlab) = tokio::join!(
        tokio::task::spawn_blocking(probe_github),
        tokio::task::spawn_blocking(probe_gitlab),
    );

    vec![
        // A failed blocking task still answers with a well-formed row so
        // the pane renders rather than showing an error page.
        github.unwrap_or_else(|_| ProviderDiagnostic::unsupported(ProviderKind::GitHub)),
        gitlab.unwrap_or_else(|_| ProviderDiagnostic::unsupported(ProviderKind::GitLab)),
        ProviderDiagnostic::unsupported(ProviderKind::Bitbucket),
        ProviderDiagnostic::unsupported(ProviderKind::AzureDevOps),
    ]
}

fn probe_github() -> ProviderDiagnostic {
    probe(
        ProviderKind::GitHub,
        "gh",
        git_provider::github_provider().as_ref(),
    )
}

fn probe_gitlab() -> ProviderDiagnostic {
    probe(
        ProviderKind::GitLab,
        "glab",
        // Unbound to any checkout: the settings pane asks about the
        // user's overall glab login, not one repository's instance, so
        // the host-scoped `--hostname` filter is intentionally absent
        // here.
        &GitLabProvider::from_detection(&DetectedProvider::unknown()),
    )
}

/// Sequential form, for tests that want the whole list synchronously.
#[cfg(test)]
fn discover_blocking() -> Vec<ProviderDiagnostic> {
    vec![
        probe_github(),
        probe_gitlab(),
        ProviderDiagnostic::unsupported(ProviderKind::Bitbucket),
        ProviderDiagnostic::unsupported(ProviderKind::AzureDevOps),
    ]
}

/// Build one supported product's row from its adapter.
///
/// Installed-ness comes from the auth probe rather than a separate
/// `which` call: the adapter already answers `NotInstalled` when its CLI
/// is missing, and asking twice both cost a subprocess and left the two
/// answers free to disagree.
fn probe(
    kind: ProviderKind,
    cli: &str,
    provider: &dyn SourceControlProvider,
) -> ProviderDiagnostic {
    let status = provider.auth_status();
    if matches!(status, GhStatus::NotInstalled) {
        return ProviderDiagnostic {
            kind: kind.as_str().to_string(),
            supported: provider.is_implemented(),
            cli_installed: false,
            cli_version: None,
            authenticated: false,
            account: None,
            detail: Some(format!(
                "`{cli}` was not found on PATH, so Codemux cannot talk to {}.",
                kind.display_name()
            )),
            capabilities: provider.capabilities(),
        };
    }

    let (authenticated, account, detail) = match status {
        GhStatus::Authenticated { username } => {
            let account = Some(username).filter(|u| !u.trim().is_empty());
            (true, account, None)
        }
        GhStatus::NotAuthenticated => (
            false,
            None,
            Some(format!(
                "`{cli}` is installed but not signed in to {}.",
                kind.display_name()
            )),
        ),
        GhStatus::NotInstalled => unreachable!("handled above"),
    };

    ProviderDiagnostic {
        kind: kind.as_str().to_string(),
        supported: provider.is_implemented(),
        cli_installed: true,
        // Only worth asking once we know the binary is there.
        cli_version: cli_version(cli),
        authenticated,
        account,
        detail,
        capabilities: provider.capabilities(),
    }
}

/// How long the per-workspace probe may take before the UI is told
/// "nothing usable". Covers detection plus one CLI auth call, both of
/// which carry their own deadlines; this is the backstop for a wedge
/// neither of them catches.
const AUTH_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// Host-scoped readiness for one checkout — the gate behind the review
/// panel, the composer's attach menu and the new-workspace preflight.
///
/// Infallible: a wedged probe answers "not usable" rather than rejecting,
/// because every caller would have to render that as "not usable" anyway.
#[tauri::command]
pub async fn check_provider_auth(path: String) -> ProviderAuthStatus {
    let repo_path = PathBuf::from(path);
    match tokio::time::timeout(
        AUTH_PROBE_TIMEOUT,
        tokio::task::spawn_blocking(move || provider_auth_blocking(&repo_path)),
    )
    .await
    {
        Ok(Ok(status)) => status,
        _ => ProviderAuthStatus::unavailable(),
    }
}

fn provider_auth_blocking(repo_path: &std::path::Path) -> ProviderAuthStatus {
    let detected = git_provider::detect_provider(repo_path);
    // Strict resolution, matching `check_github_repo`: only a positively
    // identified, implemented product gets a real adapter, so the two
    // gates cannot disagree about whether a checkout is servable.
    let provider = git_provider::provider_for_detection(&detected);
    let kind = detected.kind.as_str().to_string();

    if !provider.is_implemented() {
        return ProviderAuthStatus {
            kind,
            supported: false,
            installed: false,
            authenticated: false,
            username: None,
        };
    }

    match provider.auth_status() {
        GhStatus::NotInstalled => ProviderAuthStatus {
            kind,
            supported: true,
            installed: false,
            authenticated: false,
            username: None,
        },
        GhStatus::NotAuthenticated => ProviderAuthStatus {
            kind,
            supported: true,
            installed: true,
            authenticated: false,
            username: None,
        },
        GhStatus::Authenticated { username } => ProviderAuthStatus {
            kind,
            supported: true,
            installed: true,
            authenticated: true,
            username: Some(username).filter(|u| !u.trim().is_empty()),
        },
    }
}

/// `<cli> --version`, reduced to its first line.
///
/// Returns `None` on any failure — a missing version is cosmetic, and a
/// broken probe must not turn into an error the pane has to render.
fn cli_version(cli: &str) -> Option<String> {
    let mut cmd = host_command(cli);
    cmd.arg("--version");
    // No DBus needed: `--version` never touches the keyring.
    sanitize_gui_env_std(&mut cmd);

    let output = run_timed(cmd, VERSION_TIMEOUT).ok()?;
    if !output.success {
        return None;
    }
    sanitize_version(&output.stdout)
}

/// First non-empty line, trimmed and length-capped. Control characters
/// are dropped so a CLI cannot inject terminal escapes into the UI.
fn sanitize_version(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).find(|l| !l.is_empty())?;
    let cleaned: String = line
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_VERSION_LEN)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_rows_are_inert_but_well_formed() {
        let row = ProviderDiagnostic::unsupported(ProviderKind::Bitbucket);
        assert_eq!(row.kind, "bitbucket");
        assert!(!row.supported);
        assert!(!row.cli_installed);
        assert!(!row.authenticated);
        assert!(row.account.is_none());
        assert!(row.detail.is_some());
    }

    #[test]
    fn discovery_always_covers_every_named_product() {
        // Runs the real probes; the assertion is only about shape, so it
        // holds whether or not `gh`/`glab` exist on the test machine.
        let rows = discover_blocking();
        let kinds: Vec<&str> = rows.iter().map(|r| r.kind.as_str()).collect();
        assert_eq!(kinds, ["github", "gitlab", "bitbucket", "azure_devops"]);
        assert!(rows[0].supported);
        assert!(rows[1].supported);
        assert!(!rows[2].supported);
        assert!(!rows[3].supported);
    }

    #[test]
    fn a_missing_cli_never_claims_authentication() {
        for row in discover_blocking() {
            if !row.cli_installed {
                assert!(!row.authenticated, "{}", row.kind);
                assert!(row.account.is_none(), "{}", row.kind);
                assert!(row.detail.is_some(), "{}", row.kind);
            }
        }
    }

    #[test]
    fn version_is_reduced_to_one_clean_line() {
        assert_eq!(
            sanitize_version("gh version 2.40.1 (2024-01-01)\nhttps://example.test\n").as_deref(),
            Some("gh version 2.40.1 (2024-01-01)")
        );
        // Leading blank lines are skipped rather than yielding None.
        assert_eq!(
            sanitize_version("\n\n  glab 1.36.0  \n").as_deref(),
            Some("glab 1.36.0")
        );
        assert_eq!(sanitize_version(""), None);
        assert_eq!(sanitize_version("   \n  \n"), None);
    }

    #[test]
    fn version_strips_control_characters_and_caps_length() {
        assert_eq!(
            sanitize_version("gh \x1b[31mversion\x1b[0m 2.0").as_deref(),
            Some("gh [31mversion[0m 2.0")
        );
        let long = format!("gh version {}", "9".repeat(400));
        let out = sanitize_version(&long).unwrap();
        assert!(out.len() <= MAX_VERSION_LEN, "{} chars", out.len());
    }

    #[test]
    fn a_version_probe_for_a_nonexistent_binary_is_none_not_a_hang() {
        assert_eq!(cli_version("codemux-definitely-not-a-real-binary-xyz"), None);
    }

    #[test]
    fn supported_rows_carry_the_adapter_s_declared_capabilities() {
        let rows = discover_blocking();
        // GitHub is the only product Codemux serves deployments for; the
        // row says so rather than the settings pane restating it.
        assert!(rows[0].capabilities.has_deployments);
        assert!(!rows[1].capabilities.has_deployments);
        assert!(rows[1].capabilities.has_pull_requests);
        assert_eq!(rows[2].capabilities, Capabilities::default());
    }

    // ── Per-workspace probe ──

    fn git(dir: &std::path::Path, args: &[&str]) {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
        assert!(output.status.success(), "git {args:?}");
    }

    #[test]
    fn a_checkout_on_an_unclassifiable_host_is_reported_as_unsupported() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);
        git(
            repo,
            &["remote", "add", "origin", "git@git.acme.internal:acme/app.git"],
        );

        let status = provider_auth_blocking(repo);
        assert_eq!(status.kind, "unknown");
        assert!(!status.supported);
        assert!(!status.installed);
        assert!(!status.authenticated);
        assert!(status.username.is_none());

        git_provider::invalidate_detection_cache(Some(repo));
    }

    /// The probe names the *detected* product even when its CLI is
    /// missing, so the UI can say "install glab" rather than falling back
    /// to GitHub copy on a GitLab checkout.
    #[test]
    fn the_detected_product_is_named_independently_of_its_cli() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);
        git(repo, &["remote", "add", "origin", "git@gitlab.com:g/p.git"]);

        let status = provider_auth_blocking(repo);
        assert_eq!(status.kind, "gitlab");
        assert!(status.supported);
        // Whether glab exists on the test machine is not this test's
        // business; the invariants that hold either way are.
        if !status.installed {
            assert!(!status.authenticated);
        }
        if !status.authenticated {
            assert!(status.username.is_none());
        }

        git_provider::invalidate_detection_cache(Some(repo));
    }

    #[test]
    fn a_wedged_probe_degrades_to_nothing_usable_rather_than_an_error() {
        let status = ProviderAuthStatus::unavailable();
        assert_eq!(status.kind, "unknown");
        assert!(!status.supported);
        assert!(!status.installed);
        assert!(!status.authenticated);
    }
}
