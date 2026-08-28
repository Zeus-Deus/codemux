//! Install and profile discovery for Hermes — filesystem only.
//!
//! Everything here answers from disk. No `hermes` process is spawned,
//! and none should be added: the profile picker renders while the user
//! is mid-click, and a Hermes launch costs seconds. The two facts the
//! picker needs — where the binary lives and which profiles exist — are
//! both plainly readable on the filesystem.
//!
//! # Profile layout
//!
//! Hermes keeps one root directory (`~/.hermes`, overridable with
//! `HERMES_HOME`) and nests named profiles under `<root>/profiles/`:
//!
//! ```text
//! ~/.hermes/                  <- the `default` profile IS the root
//!   config.yaml
//!   profile.yaml
//!   profiles/
//!     coder/                  <- a named profile
//!       config.yaml
//!       profile.yaml
//! ```
//!
//! The trap is that `default` has **no directory under `profiles/`** —
//! it is the root itself. A plain `read_dir` of `profiles/` enumerates
//! every profile except the one most users are actually on.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::HERMES_BINARY;

/// Environment variable that relocates the Hermes root.
pub const HERMES_HOME_ENV: &str = "HERMES_HOME";

/// Profile id of the root directory. Hermes has no `profiles/default`
/// directory — `-p default` and no `-p` at all both mean the root.
pub const DEFAULT_PROFILE_ID: &str = "default";

/// A profile as it appears on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesProfile {
    /// Value passed to `hermes -p <id>`.
    pub id: String,
    /// Directory backing the profile. The root itself for `default`.
    pub dir: PathBuf,
    /// `description` from `profile.yaml`, when the profile has one.
    /// Optional — plenty of profiles carry only `ui_meta`.
    pub description: Option<String>,
    /// Default model from `config.yaml`, already in the
    /// `provider:model` form the ACP catalogue uses. `None` when
    /// `config.yaml` is absent, unreadable or malformed.
    pub default_model: Option<String>,
    /// Model from `config.yaml`'s optional `fallback_model` block, same
    /// form. Usually `None` — the key ships commented out.
    pub fallback_model: Option<String>,
    /// True for the root profile.
    pub is_default: bool,
}

impl HermesProfile {
    /// Whether the profile looks logged in.
    ///
    /// Advisory, not authoritative: Hermes writes credentials to
    /// `auth.json` inside the profile directory, so a missing or empty
    /// file means setup was never completed. The authoritative answer
    /// only comes from the agent's `authMethods`, which costs a process
    /// — this is the cheap pre-check that lets the picker say "sign in"
    /// instead of rendering an empty list.
    pub fn looks_authenticated(&self) -> bool {
        std::fs::metadata(self.dir.join("auth.json"))
            .map(|meta| meta.len() > 0)
            .unwrap_or(false)
    }
}

/// Resolve the `hermes` executable.
///
/// PATH first — a Hermes installed by any means puts itself there. The
/// fallbacks cover an app launched from a GUI session whose PATH never
/// sourced the user's shell profile, which is the normal state of
/// affairs for desktop launchers.
pub fn locate_hermes_binary() -> Option<PathBuf> {
    if let Ok(path) = which::which(HERMES_BINARY) {
        return Some(path);
    }
    fallback_binary_candidates()
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// Well-known install locations, tried in order after PATH.
///
/// Split out from [`locate_hermes_binary`] so the ordering is testable
/// without a `hermes` on the test machine's PATH.
pub fn fallback_binary_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(home) = dirs::home_dir() {
        // Where the install script puts it; verified against a real install.
        candidates.push(home.join(".local/bin").join(HERMES_BINARY));
    }
    #[cfg(windows)]
    if let Some(local_app_data) = dirs::data_local_dir() {
        candidates.push(
            local_app_data
                .join("hermes")
                .join("bin")
                .join(format!("{HERMES_BINARY}.exe")),
        );
    }
    candidates
}

/// Resolve the Hermes root: `$HERMES_HOME`, else `~/.hermes`.
///
/// `None` only when the process has neither the override nor a home
/// directory, which in practice means a service account.
pub fn hermes_root() -> Option<PathBuf> {
    resolve_hermes_root(std::env::var_os(HERMES_HOME_ENV), dirs::home_dir())
}

/// The root-resolution rule, with both inputs injected.
///
/// Kept pure so the precedence is tested without mutating the process
/// environment — these tests run in parallel with everything else.
fn resolve_hermes_root(override_value: Option<OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    match override_value {
        // An empty `HERMES_HOME=` is how a shell unsets a variable it
        // cannot remove; treat it as absent rather than as the cwd.
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => home.map(|home| home.join(".hermes")),
    }
}

/// Enumerate the profiles under `root`, `default` first.
///
/// Never fails: an unreadable or absent `profiles/` directory yields
/// just the root profile, because `-p default` still works against a
/// bare root. Callers get a list they can render, not an error to
/// handle.
pub fn discover_profiles_in(root: &Path) -> Vec<HermesProfile> {
    // The root is the `default` profile. It is listed unconditionally,
    // including when the directory does not exist yet — a first launch
    // creates it, and hiding the only profile the user has is worse
    // than listing one that is about to appear.
    let mut profiles = vec![read_profile(DEFAULT_PROFILE_ID, root, true)];

    let mut named: Vec<HermesProfile> = std::fs::read_dir(root.join("profiles"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| {
            let id = entry.file_name().to_string_lossy().into_owned();
            // Defend against a hand-made `profiles/default`: it would
            // shadow the root in every id lookup.
            (!id.is_empty() && !id.starts_with('.') && id != DEFAULT_PROFILE_ID)
                .then(|| read_profile(&id, &entry.path(), false))
        })
        .collect();
    named.sort_by(|a, b| a.id.cmp(&b.id));

    profiles.extend(named);
    profiles
}

/// Enumerate the profiles of the ambient Hermes root.
pub fn discover_profiles() -> Vec<HermesProfile> {
    hermes_root()
        .map(|root| discover_profiles_in(&root))
        .unwrap_or_default()
}

/// Look one profile up by id.
pub fn find_profile(root: &Path, id: &str) -> Option<HermesProfile> {
    discover_profiles_in(root)
        .into_iter()
        .find(|profile| profile.id == id)
}

fn read_profile(id: &str, dir: &Path, is_default: bool) -> HermesProfile {
    let meta = read_yaml::<ProfileMeta>(&dir.join("profile.yaml")).unwrap_or_default();
    let config = read_yaml::<ProfileConfig>(&dir.join("config.yaml")).unwrap_or_default();
    HermesProfile {
        id: id.to_string(),
        dir: dir.to_path_buf(),
        description: meta
            .description
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty()),
        default_model: config.model.and_then(ModelSelection::qualified_id),
        fallback_model: config.fallback_model.and_then(ModelSelection::qualified_id),
        is_default,
    }
}

/// Parse a YAML file, degrading every failure to `None`.
///
/// Both files this module reads are optional *and* hand-editable, so a
/// syntax error in one profile must not take out the profile list. The
/// worst case is a profile that renders without its description.
fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_yaml::from_str(&text).ok()
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ProfileMeta {
    description: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ProfileConfig {
    model: Option<ModelSelection>,
    fallback_model: Option<ModelSelection>,
}

/// The `model:` / `fallback_model:` blocks in `config.yaml`.
///
/// `model` names its model in `default:` while `fallback_model` uses
/// `model:`; both keys are accepted here so one shape covers both
/// blocks.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ModelSelection {
    provider: Option<String>,
    default: Option<String>,
    model: Option<String>,
}

impl ModelSelection {
    /// Compose the `provider:model` id the ACP catalogue reports.
    ///
    /// Both halves are required: a bare model name matches no entry in
    /// `models.availableModels`, and a seed that can never reconcile
    /// with the live list is worse than no seed at all.
    fn qualified_id(self) -> Option<String> {
        let name = self
            .default
            .or(self.model)
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty())?;
        let provider = self
            .provider
            .map(|provider| provider.trim().to_string())
            .filter(|provider| !provider.is_empty())?;
        Some(format!("{provider}:{name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().expect("path has a parent")).expect("create dirs");
        std::fs::write(path, contents).expect("write fixture");
    }

    #[test]
    fn default_profile_is_the_root_and_has_no_directory_under_profiles() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(
            &root.join("profile.yaml"),
            "description: The everyday profile\n",
        );
        write(
            &root.join("config.yaml"),
            "model:\n  default: gpt-5.6-sol\n  provider: openai-codex\n",
        );
        write(
            &root.join("profiles/coder/profile.yaml"),
            "description: Dev\n",
        );
        std::fs::create_dir_all(root.join("profiles/omar")).expect("create dirs");

        let profiles = discover_profiles_in(root);
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        // `default` must lead and must be present even though nothing
        // under profiles/ names it — the whole point of this module.
        assert_eq!(ids, vec!["default", "coder", "omar"]);

        let default = &profiles[0];
        assert!(default.is_default);
        assert_eq!(default.dir, root);
        assert_eq!(default.description.as_deref(), Some("The everyday profile"));
        assert_eq!(
            default.default_model.as_deref(),
            Some("openai-codex:gpt-5.6-sol")
        );
        assert!(!profiles[1].is_default);
    }

    #[test]
    fn a_profile_without_profile_yaml_is_still_listed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        // Neither profile.yaml nor config.yaml anywhere: a profile that
        // was created but never configured must still be pickable.
        std::fs::create_dir_all(root.join("profiles/bare")).expect("create dirs");

        let profiles = discover_profiles_in(root);
        let bare = profiles
            .iter()
            .find(|p| p.id == "bare")
            .expect("bare profile listed");
        assert_eq!(bare.description, None);
        assert_eq!(bare.default_model, None);
        assert_eq!(profiles.len(), 2, "root plus the bare profile");
    }

    #[test]
    fn a_malformed_config_degrades_instead_of_erroring() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(&root.join("profile.yaml"), "description: Still readable\n");
        // Unbalanced quote — serde_yaml rejects the document outright.
        write(
            &root.join("config.yaml"),
            "model:\n  default: \"unclosed\n\t- : :\n",
        );
        write(
            &root.join("profiles/ok/config.yaml"),
            "model:\n  default: claude-opus-5\n  provider: anthropic\n",
        );

        let profiles = discover_profiles_in(root);
        // The broken file costs the root its model and nothing more:
        // the description still reads and the sibling is untouched.
        assert_eq!(profiles[0].default_model, None);
        assert_eq!(profiles[0].description.as_deref(), Some("Still readable"));
        assert_eq!(
            profiles[1].default_model.as_deref(),
            Some("anthropic:claude-opus-5")
        );
    }

    #[test]
    fn a_half_specified_model_block_yields_no_seed() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        // Provider without a model name, and a model name without a
        // provider: neither composes into an id the live catalogue
        // would match, so neither is offered.
        write(&root.join("config.yaml"), "model:\n  provider: anthropic\n");
        write(
            &root.join("profiles/nameonly/config.yaml"),
            "model:\n  default: gpt-5.6-sol\n",
        );

        let profiles = discover_profiles_in(root);
        assert_eq!(profiles[0].default_model, None);
        assert_eq!(profiles[1].default_model, None);
    }

    #[test]
    fn fallback_model_is_read_from_its_own_key_shape() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(
            &root.join("config.yaml"),
            "model:\n  default: gpt-5.6-sol\n  provider: openai-codex\n\
             fallback_model:\n  provider: openrouter\n  model: anthropic/claude-sonnet-4\n",
        );

        let profile = find_profile(root, DEFAULT_PROFILE_ID).expect("default profile");
        assert_eq!(
            profile.default_model.as_deref(),
            Some("openai-codex:gpt-5.6-sol")
        );
        assert_eq!(
            profile.fallback_model.as_deref(),
            Some("openrouter:anthropic/claude-sonnet-4")
        );
    }

    #[test]
    fn a_hand_made_profiles_default_directory_does_not_shadow_the_root() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(&root.join("profile.yaml"), "description: Real root\n");
        write(
            &root.join("profiles/default/profile.yaml"),
            "description: Impostor\n",
        );

        let profiles = discover_profiles_in(root);
        let defaults: Vec<&HermesProfile> = profiles
            .iter()
            .filter(|p| p.id == DEFAULT_PROFILE_ID)
            .collect();
        assert_eq!(defaults.len(), 1);
        assert_eq!(defaults[0].dir, root);
        assert_eq!(defaults[0].description.as_deref(), Some("Real root"));
    }

    #[test]
    fn a_missing_root_still_offers_the_default_profile() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("never-created");
        let profiles = discover_profiles_in(&root);
        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].id, DEFAULT_PROFILE_ID);
    }

    #[test]
    fn authentication_is_judged_by_a_non_empty_auth_json() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        write(
            &root.join("profiles/signedin/auth.json"),
            "{\"token\":\"x\"}",
        );
        write(&root.join("profiles/empty/auth.json"), "");

        let profiles = discover_profiles_in(root);
        let looks_authenticated = |id: &str| {
            profiles
                .iter()
                .find(|p| p.id == id)
                .expect("profile listed")
                .looks_authenticated()
        };
        assert!(looks_authenticated("signedin"));
        assert!(
            !looks_authenticated("empty"),
            "an empty auth.json is not a login"
        );
        assert!(
            !looks_authenticated(DEFAULT_PROFILE_ID),
            "no auth.json at all"
        );
    }

    #[test]
    fn hermes_home_wins_over_the_home_directory_but_only_when_set() {
        let home = PathBuf::from("/home/tester");
        assert_eq!(
            resolve_hermes_root(Some(OsString::from("/srv/hermes")), Some(home.clone())),
            Some(PathBuf::from("/srv/hermes"))
        );
        assert_eq!(
            resolve_hermes_root(None, Some(home.clone())),
            Some(home.join(".hermes"))
        );
        // An empty override is an unset override, not the cwd.
        assert_eq!(
            resolve_hermes_root(Some(OsString::new()), Some(home.clone())),
            Some(home.join(".hermes"))
        );
        assert_eq!(resolve_hermes_root(None, None), None);
    }

    #[test]
    fn the_install_script_location_is_the_first_fallback_candidate() {
        let candidates = fallback_binary_candidates();
        let home = dirs::home_dir().expect("test host has a home directory");
        assert_eq!(candidates.first(), Some(&home.join(".local/bin/hermes")));
    }
}
