//! Maps a checkout to the implementation that serves it.
//!
//! Two entry points, because the two families of caller want different
//! things from an unclassifiable host:
//!
//! - [`provider_for_path`] is **strict**, for gates and pollers: only a
//!   positively identified, implemented product gets a real provider.
//!   This is the direct replacement for `github::is_github_repo`.
//! - [`provider_for_path_or_default`] is **advisory**, for the
//!   GitHub-named command surface: a host Codemux cannot classify still
//!   reaches the GitHub adapter. Self-hosted GitHub Enterprise on a
//!   bespoke domain (`git.acme.com`) has always worked through those
//!   commands — `gh` resolves it from its own hosts config — and
//!   detection must not be allowed to take that away. Only a repository
//!   identified as a *different* known product gets the null object,
//!   which is the one case where the old passthrough could never have
//!   produced anything but a confusing CLI error.

use std::path::Path;
use std::sync::{Arc, LazyLock};

use super::detect::{detect_provider, DetectedProvider, ProviderKind};
use super::github::GitHubProvider;
use super::gitlab::GitLabProvider;
use super::provider::SourceControlProvider;
use super::unsupported::UnsupportedProvider;

static GITHUB: LazyLock<Arc<GitHubProvider>> = LazyLock::new(|| Arc::new(GitHubProvider));

/// The GitHub adapter, for the handful of call sites that are GitHub by
/// definition rather than by detection (the `gh`-availability and
/// `gh`-auth probes behind the GitHub-named Tauri commands).
pub fn github_provider() -> Arc<dyn SourceControlProvider> {
    GITHUB.clone()
}

/// Resolve an already-computed detection to its implementation.
///
/// GitHub is a shared singleton because the adapter is stateless; the
/// GitLab one is built per detection because it needs the instance the
/// checkout points at — to scope its auth probe to that host, and to
/// name it in error copy.
pub fn provider_for_detection(detected: &DetectedProvider) -> Arc<dyn SourceControlProvider> {
    match detected.kind {
        ProviderKind::GitHub => GITHUB.clone(),
        ProviderKind::GitLab => Arc::new(GitLabProvider::from_detection(detected)),
        _ => Arc::new(UnsupportedProvider::from_detection(detected)),
    }
}

/// Advisory counterpart of [`provider_for_detection`].
pub fn provider_for_detection_or_default(
    detected: &DetectedProvider,
) -> Arc<dyn SourceControlProvider> {
    match detected.kind {
        ProviderKind::Unknown => GITHUB.clone(),
        _ => provider_for_detection(detected),
    }
}

/// Strict resolution — see the module docs.
pub fn provider_for_path(repo_path: &Path) -> Arc<dyn SourceControlProvider> {
    provider_for_detection(&detect_provider(repo_path))
}

/// Advisory resolution — see the module docs.
pub fn provider_for_path_or_default(repo_path: &Path) -> Arc<dyn SourceControlProvider> {
    provider_for_detection_or_default(&detect_provider(repo_path))
}

/// The gate the workspace pollers and the repo preflight apply: does an
/// implemented integration exist for this checkout?
///
/// Replaces `github::is_github_repo`. For a `github.com` remote the
/// answer is identical; the difference is that the question is now asked
/// of the classified hostname rather than of a substring anywhere in the
/// remote URL.
pub fn repo_has_supported_provider(repo_path: &Path) -> bool {
    provider_for_path(repo_path).is_implemented()
}

/// Value for the workspace snapshot's `provider_kind`.
///
/// `Unknown` maps to `None`: a checkout whose host says nothing has no
/// product to name, and "unknown" on the wire would only invite the UI
/// to render it.
pub fn provider_kind_field(detected: &DetectedProvider) -> Option<String> {
    match detected.kind {
        ProviderKind::Unknown => None,
        kind => Some(kind.as_str().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detected(kind: ProviderKind) -> DetectedProvider {
        DetectedProvider {
            kind,
            host: Some("example.test".to_string()),
            base_url: Some("https://example.test".to_string()),
            remote_name: Some("origin".to_string()),
        }
    }

    #[test]
    fn implemented_products_resolve_to_a_working_provider() {
        for kind in [ProviderKind::GitHub, ProviderKind::GitLab] {
            let provider = provider_for_detection(&detected(kind));
            assert_eq!(provider.kind(), kind);
            assert!(provider.is_implemented(), "{kind:?}");
        }
    }

    #[test]
    fn every_other_kind_resolves_to_the_null_object() {
        for kind in [
            ProviderKind::Bitbucket,
            ProviderKind::AzureDevOps,
            ProviderKind::Unknown,
        ] {
            let provider = provider_for_detection(&detected(kind));
            assert_eq!(provider.kind(), kind);
            assert!(!provider.is_implemented(), "{kind:?}");
        }
    }

    #[test]
    fn advisory_resolution_only_rescues_the_unclassified() {
        // An unrecognised host keeps the pre-detection passthrough, so a
        // self-hosted deployment the CLI can resolve on its own is not
        // locked out by a hostname that reveals nothing.
        let provider = provider_for_detection_or_default(&detected(ProviderKind::Unknown));
        assert_eq!(provider.kind(), ProviderKind::GitHub);
        // A positively identified other product is served by its own
        // adapter, or by the null object when there isn't one.
        let provider = provider_for_detection_or_default(&detected(ProviderKind::GitLab));
        assert_eq!(provider.kind(), ProviderKind::GitLab);
        let provider = provider_for_detection_or_default(&detected(ProviderKind::Bitbucket));
        assert!(!provider.is_implemented());
    }

    #[test]
    fn snapshot_field_names_known_products_only() {
        assert_eq!(
            provider_kind_field(&detected(ProviderKind::GitHub)).as_deref(),
            Some("github")
        );
        assert_eq!(
            provider_kind_field(&detected(ProviderKind::AzureDevOps)).as_deref(),
            Some("azure_devops")
        );
        assert_eq!(provider_kind_field(&detected(ProviderKind::Unknown)), None);
        assert_eq!(provider_kind_field(&DetectedProvider::unknown()), None);
    }
}
