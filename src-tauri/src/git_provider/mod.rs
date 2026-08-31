//! Provider-agnostic seam for git *hosting* features (pull requests,
//! issues, checks, reviews).
//!
//! Codemux's hosting integration grew up GitHub-only: every call shells
//! out to `gh` and the gate for "may we show PR UI here?" was a string
//! match on `github.com` in `git remote -v`. This module replaces both
//! halves of that with a seam:
//!
//! - [`detect`] answers *which* product a checkout is hosted on, from
//!   its remotes alone (no network, no CLI auth).
//! - [`SourceControlProvider`] is the operation surface the UI actually
//!   uses, so a second product can be added without touching callers.
//! - [`registry`] maps a detected repository to the implementation that
//!   serves it, falling back to a null object rather than an `Option`
//!   so no caller has to branch on "is there a provider".
//!
//! Two adapters are implemented. The GitHub one is a thin delegation to
//! the existing `crate::github` functions, so routing a call site
//! through the trait cannot change what GitHub users observe. The GitLab
//! one shells out to `glab` and owns its own mapping onto the same
//! structs — see [`gitlab`].

pub mod cache;
pub mod detect;
pub(crate) mod exec;
pub mod github;
pub mod gitlab;
pub mod provider;
pub mod registry;
pub mod unsupported;

pub use detect::{
    detect_provider, invalidate_detection_cache, try_detect_provider, DetectedProvider,
    ProviderKind,
};
pub use gitlab::GitLabProvider;
pub use provider::{Capabilities, Operation, OperationCapabilities, SourceControlProvider};
pub use registry::{
    github_provider, provider_for_detection, provider_for_detection_or_default, provider_for_path,
    provider_for_path_or_default, provider_kind_field, repo_has_supported_provider,
};
pub use unsupported::UnsupportedProvider;
