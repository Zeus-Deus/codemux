//! Which hosting product a checkout's remotes point at.
//!
//! Detection is deliberately offline: it reads `git remote -v` (plus the
//! current branch's upstream) and classifies the *hostname*. No CLI, no
//! auth, no network — the same properties the old `github::is_github_repo`
//! gate had, so it can sit on the hot path of the workspace pollers.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::execution::sanitize_gui_env_std;

/// Which hosting product serves a repository.
///
/// Serialized as flat snake_case strings so the value can ride on the
/// workspace snapshot and be compared in TypeScript without a mapping
/// table. The variant names are Rust-cased; the wire names are not
/// derived from them (`GitHub` would render as `git_hub`), so every
/// variant carries an explicit rename.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum ProviderKind {
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "gitlab")]
    GitLab,
    #[serde(rename = "bitbucket")]
    Bitbucket,
    #[serde(rename = "azure_devops")]
    AzureDevOps,
    /// No remote, a local-only checkout, or a host nothing recognises.
    /// Never an error: an unclassified repository is a normal state.
    #[default]
    #[serde(rename = "unknown")]
    Unknown,
}

impl ProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProviderKind::GitHub => "github",
            ProviderKind::GitLab => "gitlab",
            ProviderKind::Bitbucket => "bitbucket",
            ProviderKind::AzureDevOps => "azure_devops",
            ProviderKind::Unknown => "unknown",
        }
    }

    /// Human-facing product name, for error copy and (later) settings UI.
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderKind::GitHub => "GitHub",
            ProviderKind::GitLab => "GitLab",
            ProviderKind::Bitbucket => "Bitbucket",
            ProviderKind::AzureDevOps => "Azure DevOps",
            ProviderKind::Unknown => "an unrecognised host",
        }
    }

    /// Lenient parse for values that arrive from synced settings, where a
    /// blob written by a newer build (or by hand) must never break the
    /// read. Separators and case are ignored; anything unrecognised is
    /// `None` so the caller can drop the entry instead of guessing.
    pub fn parse_lenient(value: &str) -> Option<ProviderKind> {
        let normalized: String = value
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect();
        match normalized.as_str() {
            "github" | "githubenterprise" | "ghe" => Some(ProviderKind::GitHub),
            "gitlab" => Some(ProviderKind::GitLab),
            "bitbucket" | "bitbucketserver" | "bitbucketcloud" => Some(ProviderKind::Bitbucket),
            "azuredevops" | "azure" | "vsts" | "tfs" => Some(ProviderKind::AzureDevOps),
            "unknown" | "none" => Some(ProviderKind::Unknown),
            _ => None,
        }
    }
}

/// The outcome of classifying a checkout.
///
/// `host` is a bare lowercased hostname (no userinfo, no port) — it is
/// what classification runs against and what error copy may safely echo.
/// `base_url` is the web origin the host implies, and *does* carry a
/// port when the remote named one over http/https. An SSH port is
/// deliberately dropped: `ssh://git@host:2222/…` says nothing about
/// where the web UI listens, and a `https://host:2222` base would be a
/// broken link.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedProvider {
    pub kind: ProviderKind,
    pub host: Option<String>,
    pub base_url: Option<String>,
    /// Which remote the classification came from — see
    /// [`select_remote`] for the preference order.
    pub remote_name: Option<String>,
}

impl DetectedProvider {
    /// The "nothing to go on" result: no remotes, not a repo, or git
    /// itself unavailable. Callers treat it exactly like a classified
    /// `Unknown`; only the cache distinguishes them (see
    /// [`detect_provider`]).
    pub fn unknown() -> Self {
        Self {
            kind: ProviderKind::Unknown,
            host: None,
            base_url: None,
            remote_name: None,
        }
    }
}

impl Default for DetectedProvider {
    fn default() -> Self {
        Self::unknown()
    }
}

// ── Remote parsing ──────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Remote {
    pub name: String,
    pub url: String,
}

/// Parse the stdout of `git remote -v` into one entry per remote, in
/// first-appearance order. `git remote -v` prints a `(fetch)` and a
/// `(push)` row per remote and they can differ (push-to-fork setups);
/// the fetch URL is the one that identifies where the code lives, so it
/// wins when both are present.
pub(crate) fn parse_remote_lines(text: &str) -> Vec<Remote> {
    let mut out: Vec<Remote> = Vec::new();
    let mut seen_fetch: Vec<String> = Vec::new();

    for line in text.lines() {
        let mut fields = line.split_whitespace();
        let (Some(name), Some(url)) = (fields.next(), fields.next()) else {
            continue;
        };
        let is_fetch = fields.next().is_some_and(|kind| kind == "(fetch)");

        match out.iter_mut().find(|r| r.name == name) {
            Some(existing) => {
                if is_fetch && !seen_fetch.iter().any(|n| n == name) {
                    existing.url = url.to_string();
                    seen_fetch.push(name.to_string());
                }
            }
            None => {
                out.push(Remote {
                    name: name.to_string(),
                    url: url.to_string(),
                });
                if is_fetch {
                    seen_fetch.push(name.to_string());
                }
            }
        }
    }

    out
}

/// A remote URL reduced to what detection needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteEndpoint {
    /// Lowercased hostname with userinfo and port removed.
    pub host: String,
    /// Web origin implied by the remote, port included when the remote
    /// used http/https and named one.
    pub base_url: String,
}

/// Split a remote URL into `(host, web origin)`.
///
/// Handles the three forms git accepts for a network remote:
/// `scheme://[user[:pass]@]host[:port]/path`, scp-style
/// `[user@]host:path` (which has no port — a `host:1234/path` reads as a
/// *path* starting with `1234`), and bracketed IPv6 authorities.
/// Filesystem remotes (`/srv/repo.git`, `../sibling`, `file://…`) return
/// `None`: there is no host to classify.
pub(crate) fn parse_remote_endpoint(url: &str) -> Option<RemoteEndpoint> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }

    let (scheme, authority) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            (scheme.to_ascii_lowercase(), authority.to_string())
        }
        None => {
            // scp-style only when the colon comes before any slash;
            // otherwise this is a local path like `/a/b:c`.
            let (before, _after) = url.split_once(':')?;
            if before.contains('/') || before.is_empty() {
                return None;
            }
            ("ssh".to_string(), before.to_string())
        }
    };

    if scheme == "file" {
        return None;
    }

    // Strip `user[:pass]@`. The rightmost `@` wins so a password
    // containing one can't truncate the host.
    let authority = authority
        .rsplit_once('@')
        .map_or(authority.as_str(), |(_, host)| host);
    if authority.is_empty() {
        return None;
    }

    let (host, port) = split_host_port(authority)?;
    if host.is_empty() {
        return None;
    }

    // Only an http(s) remote names a *web* port. See `DetectedProvider`.
    let web_scheme = if scheme == "http" { "http" } else { "https" };
    let keep_port = matches!(scheme.as_str(), "http" | "https");
    let base_url = match port.filter(|_| keep_port) {
        Some(port) => format!("{web_scheme}://{host}:{port}"),
        None => format!("{web_scheme}://{host}"),
    };

    Some(RemoteEndpoint {
        host: host.to_ascii_lowercase(),
        base_url,
    })
}

/// Split `host[:port]`, tolerating a bracketed IPv6 literal.
fn split_host_port(authority: &str) -> Option<(String, Option<String>)> {
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']')?;
        let port = after.strip_prefix(':').map(str::to_string);
        return Some((format!("[{host}]"), port));
    }
    match authority.split_once(':') {
        Some((host, port)) if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) => {
            Some((host.to_string(), Some(port.to_string())))
        }
        // A colon with a non-numeric tail is not a port; keep the whole
        // authority as the host rather than silently truncating it.
        Some(_) => Some((authority.to_string(), None)),
        None => Some((authority.to_string(), None)),
    }
}

// ── Classification ──────────────────────────────────────────────

/// Classify a bare hostname.
///
/// Order is fixed and load-bearing: an explicit custom-host mapping from
/// synced settings always wins, because it exists precisely to name a
/// self-hosted instance whose domain says nothing about the product
/// (`git.acme.internal`). Only then do the built-in heuristics run —
/// exact well-known hosts first, then the looser substring rules that
/// cover the usual self-hosted naming (`gitlab.acme.com`).
pub(crate) fn classify_host(
    host: &str,
    custom_hosts: &HashMap<String, ProviderKind>,
) -> ProviderKind {
    let host = host.to_ascii_lowercase();

    if let Some(kind) = custom_hosts.get(&host) {
        return *kind;
    }

    match host.as_str() {
        "github.com" | "www.github.com" | "ssh.github.com" => return ProviderKind::GitHub,
        "gitlab.com" | "www.gitlab.com" | "altssh.gitlab.com" => return ProviderKind::GitLab,
        "bitbucket.org" | "www.bitbucket.org" | "altssh.bitbucket.org" => {
            return ProviderKind::Bitbucket
        }
        "dev.azure.com" | "ssh.dev.azure.com" => return ProviderKind::AzureDevOps,
        _ => {}
    }

    if host.ends_with(".dev.azure.com") || host.ends_with(".visualstudio.com") {
        return ProviderKind::AzureDevOps;
    }
    if host.contains("github") {
        return ProviderKind::GitHub;
    }
    if host.contains("gitlab") {
        return ProviderKind::GitLab;
    }
    if host.contains("bitbucket") {
        return ProviderKind::Bitbucket;
    }

    ProviderKind::Unknown
}

/// Resolve `git rev-parse --abbrev-ref @{upstream}` (`origin/main`) to
/// the remote name it names. Matching against the actual remote list
/// rather than splitting on the first `/` is what makes this correct for
/// remotes whose own name contains a slash, and for branch names that
/// contain one.
pub(crate) fn upstream_remote_name<'a>(upstream: &str, remotes: &'a [Remote]) -> Option<&'a str> {
    remotes
        .iter()
        .filter(|remote| upstream.starts_with(&format!("{}/", remote.name)))
        .max_by_key(|remote| remote.name.len())
        .map(|remote| remote.name.as_str())
}

/// THE multi-remote policy, stated once so nothing has to re-derive it.
///
/// A classifiable remote always beats an unclassifiable one, because the
/// gate this replaced (`is_github_repo`) matched *any* remote: a checkout
/// whose `origin` is an unrecognised mirror and whose second remote is on
/// github.com used to light up the PR UI, and must keep doing so. Within
/// the classifiable remotes, and again within the unclassifiable ones,
/// the preference is:
///
/// 1. the remote the current branch's upstream tracks — the repository
///    this checkout actually pushes to and reads PRs from, which for a
///    fork workflow is not `origin`;
/// 2. `origin`, the conventional primary;
/// 3. the first remote listed.
///
/// With nothing classifiable at all the tail is `origin`, then the first
/// remote, so `host`/`base_url` are still populated for the UI.
pub(crate) fn select_remote<'a>(
    remotes: &'a [Remote],
    upstream_remote: Option<&str>,
    custom_hosts: &HashMap<String, ProviderKind>,
) -> Option<&'a Remote> {
    let classifiable = |remote: &Remote| {
        parse_remote_endpoint(&remote.url)
            .map(|endpoint| classify_host(&endpoint.host, custom_hosts))
            .is_some_and(|kind| kind != ProviderKind::Unknown)
    };

    let upstream = upstream_remote.and_then(|name| remotes.iter().find(|r| r.name == name));
    let origin = remotes.iter().find(|r| r.name == "origin");

    if let Some(remote) = upstream.filter(|r| classifiable(r)) {
        return Some(remote);
    }
    if let Some(remote) = origin.filter(|r| classifiable(r)) {
        return Some(remote);
    }
    if let Some(remote) = remotes.iter().find(|r| classifiable(r)) {
        return Some(remote);
    }
    origin.or_else(|| remotes.first())
}

/// Pure core of detection, so the policy above is testable without a
/// repository on disk.
pub(crate) fn detect_from_remotes(
    remotes: &[Remote],
    upstream_remote: Option<&str>,
    custom_hosts: &HashMap<String, ProviderKind>,
) -> DetectedProvider {
    let Some(remote) = select_remote(remotes, upstream_remote, custom_hosts) else {
        return DetectedProvider::unknown();
    };
    let Some(endpoint) = parse_remote_endpoint(&remote.url) else {
        // A filesystem remote is a real remote with no host to classify.
        return DetectedProvider {
            kind: ProviderKind::Unknown,
            host: None,
            base_url: None,
            remote_name: Some(remote.name.clone()),
        };
    };
    DetectedProvider {
        kind: classify_host(&endpoint.host, custom_hosts),
        host: Some(endpoint.host),
        base_url: Some(endpoint.base_url),
        remote_name: Some(remote.name.clone()),
    }
}

// ── Settings ────────────────────────────────────────────────────

/// Read `source_control.custom_hosts` out of the synced-settings cache.
///
/// Written by Settings → Source Control's self-hosted mapping. Entries
/// are normalised (hostname lowercased, any port the user typed dropped,
/// value parsed leniently) and unparseable ones are skipped: a settings
/// blob is synced across devices and must never be able to break
/// detection.
pub(crate) fn custom_hosts_from_settings() -> HashMap<String, ProviderKind> {
    let Some(settings) = crate::settings_sync::load_cache() else {
        return HashMap::new();
    };
    normalize_custom_hosts(&settings.source_control.custom_hosts)
}

pub(crate) fn normalize_custom_hosts(
    raw: &HashMap<String, String>,
) -> HashMap<String, ProviderKind> {
    raw.iter()
        .filter_map(|(host, kind)| {
            let host = host.trim().trim_end_matches('/').to_ascii_lowercase();
            let host = host.rsplit_once(':').map_or(host.as_str(), |(h, port)| {
                if port.chars().all(|c| c.is_ascii_digit()) && !port.is_empty() {
                    h
                } else {
                    host.as_str()
                }
            });
            if host.is_empty() {
                return None;
            }
            Some((host.to_string(), ProviderKind::parse_lenient(kind)?))
        })
        .collect()
}

// ── Cache ───────────────────────────────────────────────────────

/// 60s. Remotes change about as often as a user edits `.git/config`,
/// and the pollers hit detection every 5s per active workspace, so a
/// minute of reuse removes almost all of the subprocess cost while
/// keeping a newly added remote within one poll cycle of being noticed.
const DETECTION_TTL: Duration = Duration::from_secs(60);

struct CacheEntry {
    value: DetectedProvider,
    fetched_at: Instant,
}

static DETECTION_CACHE: LazyLock<Mutex<HashMap<String, CacheEntry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Drop cached detection for one path, or all of them when `None`.
pub fn invalidate_detection_cache(repo_path: Option<&Path>) {
    if let Ok(mut cache) = DETECTION_CACHE.lock() {
        match repo_path {
            Some(path) => {
                cache.remove(&path.display().to_string());
            }
            None => cache.clear(),
        }
    }
}

/// `git` in a checkout, reduced to trimmed stdout. `None` when git could
/// not run, exited non-zero, or said nothing — the three cases every
/// caller treats the same way ("no answer"). Shared with the provider
/// adapters, which ask git the same kind of question.
pub(crate) fn run_git(repo_path: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = crate::execution::host_command("git");
    cmd.args(args).current_dir(repo_path);
    sanitize_gui_env_std(&mut cmd);
    cmd.output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|output| !output.is_empty())
}

/// Classify the checkout at `repo_path`.
///
/// Never fails: a directory that is not a repository, has no remotes, or
/// sits on a host nothing recognises all resolve to `Unknown`. Only a
/// positive classification is cached — an `Unknown` is re-probed every
/// call, so a freshly cloned workspace, a `git remote add origin …`, or a
/// newly saved custom-host mapping is picked up on the next render rather
/// than a minute later.
pub fn detect_provider(repo_path: &Path) -> DetectedProvider {
    let key = repo_path.display().to_string();

    if let Ok(cache) = DETECTION_CACHE.lock() {
        if let Some(entry) = cache.get(&key) {
            if entry.fetched_at.elapsed() < DETECTION_TTL {
                return entry.value.clone();
            }
        }
    }

    let Some(remote_text) = run_git(repo_path, &["remote", "-v"]) else {
        return DetectedProvider::unknown();
    };
    let remotes = parse_remote_lines(&remote_text);

    // Only worth asking when there is more than one remote to choose
    // between — for the single-remote case the answer cannot change the
    // outcome and this is a subprocess.
    let upstream = if remotes.len() > 1 {
        run_git(repo_path, &["rev-parse", "--abbrev-ref", "@{upstream}"])
    } else {
        None
    };
    let upstream_remote =
        upstream.as_deref().and_then(|u| upstream_remote_name(u, &remotes));

    let detected = detect_from_remotes(&remotes, upstream_remote, &custom_hosts_from_settings());

    // Only a positive classification is remembered. `Unknown` is the
    // answer for a checkout whose remote was just added, or one waiting
    // on a custom-host mapping, and the UI deliberately never caches a
    // negative repo check so `git remote add origin …` takes effect on
    // the next render rather than a minute later.
    if detected.kind != ProviderKind::Unknown {
        if let Ok(mut cache) = DETECTION_CACHE.lock() {
            cache.retain(|_, entry| entry.fetched_at.elapsed() < DETECTION_TTL);
            cache.insert(
                key,
                CacheEntry {
                    value: detected.clone(),
                    fetched_at: Instant::now(),
                },
            );
        }
    }
    detected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str, url: &str) -> Remote {
        Remote {
            name: name.to_string(),
            url: url.to_string(),
        }
    }

    fn no_custom() -> HashMap<String, ProviderKind> {
        HashMap::new()
    }

    #[test]
    fn kind_serializes_as_flat_snake_case() {
        let cases = [
            (ProviderKind::GitHub, "\"github\""),
            (ProviderKind::GitLab, "\"gitlab\""),
            (ProviderKind::Bitbucket, "\"bitbucket\""),
            (ProviderKind::AzureDevOps, "\"azure_devops\""),
            (ProviderKind::Unknown, "\"unknown\""),
        ];
        for (kind, wire) in cases {
            assert_eq!(serde_json::to_string(&kind).unwrap(), wire);
            assert_eq!(kind.as_str(), wire.trim_matches('"'));
            let back: ProviderKind = serde_json::from_str(wire).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn parse_lenient_accepts_settings_spellings() {
        assert_eq!(
            ProviderKind::parse_lenient("Azure-DevOps"),
            Some(ProviderKind::AzureDevOps)
        );
        assert_eq!(
            ProviderKind::parse_lenient("  GITLAB "),
            Some(ProviderKind::GitLab)
        );
        assert_eq!(ProviderKind::parse_lenient("gitea"), None);
        assert_eq!(ProviderKind::parse_lenient(""), None);
    }

    // ── remote URL parsing ──

    #[test]
    fn parses_ssh_https_and_scp_forms_to_the_same_host() {
        for url in [
            "git@github.com:acme/app.git",
            "ssh://git@github.com/acme/app.git",
            "https://github.com/acme/app.git",
            "https://github.com/acme/app",
        ] {
            let endpoint = parse_remote_endpoint(url).unwrap_or_else(|| panic!("{url}"));
            assert_eq!(endpoint.host, "github.com", "{url}");
        }
    }

    #[test]
    fn https_port_is_kept_in_base_url_but_never_in_the_host() {
        let endpoint = parse_remote_endpoint("https://gitlab.example.com:8443/g/p.git").unwrap();
        assert_eq!(endpoint.host, "gitlab.example.com");
        assert_eq!(endpoint.base_url, "https://gitlab.example.com:8443");
        assert_eq!(
            classify_host(&endpoint.host, &no_custom()),
            ProviderKind::GitLab,
            "the port must not defeat classification"
        );
    }

    #[test]
    fn http_remote_keeps_its_scheme_and_port() {
        let endpoint = parse_remote_endpoint("http://gitlab.example.com:8080/g/p.git").unwrap();
        assert_eq!(endpoint.base_url, "http://gitlab.example.com:8080");
    }

    #[test]
    fn ssh_port_is_dropped_from_the_web_origin() {
        // 2222 is where sshd listens, not where the web UI does — a
        // `https://host:2222` base would be a dead link.
        let endpoint = parse_remote_endpoint("ssh://git@gitlab.example.com:2222/g/p.git").unwrap();
        assert_eq!(endpoint.host, "gitlab.example.com");
        assert_eq!(endpoint.base_url, "https://gitlab.example.com");
    }

    #[test]
    fn scp_form_colon_introduces_a_path_not_a_port() {
        let endpoint = parse_remote_endpoint("git@gitlab.example.com:1234/group/proj.git").unwrap();
        assert_eq!(endpoint.host, "gitlab.example.com");
        assert_eq!(endpoint.base_url, "https://gitlab.example.com");
    }

    #[test]
    fn userinfo_is_stripped_from_the_host() {
        let endpoint =
            parse_remote_endpoint("https://user:ghp_secret@github.com/acme/app.git").unwrap();
        assert_eq!(endpoint.host, "github.com");
        assert_eq!(endpoint.base_url, "https://github.com");
    }

    #[test]
    fn host_is_lowercased() {
        let endpoint = parse_remote_endpoint("https://GitHub.COM/Acme/App.git").unwrap();
        assert_eq!(endpoint.host, "github.com");
    }

    #[test]
    fn bracketed_ipv6_authority_survives_port_splitting() {
        let endpoint = parse_remote_endpoint("https://[2001:db8::1]:8443/g/p.git").unwrap();
        assert_eq!(endpoint.host, "[2001:db8::1]");
        assert_eq!(endpoint.base_url, "https://[2001:db8::1]:8443");
    }

    #[test]
    fn filesystem_remotes_have_no_host() {
        for url in [
            "/srv/git/repo.git",
            "../sibling",
            "./repo",
            "file:///srv/git/repo.git",
            "",
        ] {
            assert_eq!(parse_remote_endpoint(url), None, "{url}");
        }
    }

    // ── `git remote -v` parsing ──

    #[test]
    fn remote_list_dedupes_rows_and_prefers_the_fetch_url() {
        let text = "\
origin\tgit@github.com:acme/app.git (fetch)
origin\tgit@github.com:fork/app.git (push)
upstream\thttps://gitlab.com/g/p.git (fetch)
upstream\thttps://gitlab.com/g/p.git (push)
";
        let remotes = parse_remote_lines(text);
        assert_eq!(remotes.len(), 2);
        assert_eq!(remotes[0].name, "origin");
        assert_eq!(remotes[0].url, "git@github.com:acme/app.git");
        assert_eq!(remotes[1].name, "upstream");
    }

    #[test]
    fn remote_list_ignores_malformed_rows() {
        assert!(parse_remote_lines("").is_empty());
        assert!(parse_remote_lines("origin\n\n   \n").is_empty());
    }

    // ── classification ──

    #[test]
    fn well_known_hosts_classify_exactly() {
        let cases = [
            ("github.com", ProviderKind::GitHub),
            ("gitlab.com", ProviderKind::GitLab),
            ("bitbucket.org", ProviderKind::Bitbucket),
            ("dev.azure.com", ProviderKind::AzureDevOps),
            ("ssh.dev.azure.com", ProviderKind::AzureDevOps),
            ("acme.visualstudio.com", ProviderKind::AzureDevOps),
            ("gitlab.acme.com", ProviderKind::GitLab),
            ("github.acme.com", ProviderKind::GitHub),
            ("bitbucket.acme.com", ProviderKind::Bitbucket),
        ];
        for (host, expected) in cases {
            assert_eq!(classify_host(host, &no_custom()), expected, "{host}");
        }
    }

    /// The gate this module replaced matched the literal string
    /// `github.com` in the remote URL, so a self-hosted GitHub Enterprise
    /// domain answered "not GitHub" and the PR/issue UI stayed dark on a
    /// checkout `gh` could actually serve. Classifying on the hostname
    /// reverses that deliberately: GHE is GitHub, and the auth question
    /// is asked separately.
    #[test]
    fn self_hosted_enterprise_domains_classify_as_their_product() {
        assert_eq!(
            classify_host("github.acme.internal", &no_custom()),
            ProviderKind::GitHub
        );
        assert_eq!(
            classify_host("gitlab.acme.internal", &no_custom()),
            ProviderKind::GitLab
        );
    }

    /// The old gate scanned the whole remote URL, so a *path* containing
    /// the host name could false-positive. Classification reads the
    /// authority only.
    #[test]
    fn a_host_name_appearing_in_the_path_cannot_classify() {
        let remotes = vec![remote("origin", "https://mirror.acme.com/github.com/acme/app.git")];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.host.as_deref(), Some("mirror.acme.com"));
        assert_eq!(detected.kind, ProviderKind::Unknown);
    }

    /// Likewise a remote *name* is not a URL.
    #[test]
    fn a_remote_named_after_a_product_does_not_classify_it() {
        let remotes = vec![remote("github-old", "git@gitlab.com:user/repo.git")];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.kind, ProviderKind::GitLab);
    }

    #[test]
    fn unrecognised_hosts_are_unknown_never_an_error() {
        for host in [
            "git.acme.internal",
            "codeberg.org",
            "example.com",
            "localhost",
        ] {
            assert_eq!(classify_host(host, &no_custom()), ProviderKind::Unknown, "{host}");
        }
    }

    #[test]
    fn custom_host_mapping_overrides_the_heuristics() {
        let mut custom = HashMap::new();
        // A self-hosted instance whose domain reveals nothing.
        custom.insert("git.acme.internal".to_string(), ProviderKind::GitLab);
        // And a deliberate override of a name the heuristics would guess.
        custom.insert("gitlab-mirror.acme.com".to_string(), ProviderKind::GitHub);

        assert_eq!(
            classify_host("git.acme.internal", &custom),
            ProviderKind::GitLab
        );
        assert_eq!(
            classify_host("GIT.ACME.INTERNAL", &custom),
            ProviderKind::GitLab,
            "lookup is case-insensitive"
        );
        assert_eq!(
            classify_host("gitlab-mirror.acme.com", &custom),
            ProviderKind::GitHub,
            "an explicit mapping beats the substring heuristic"
        );
    }

    #[test]
    fn settings_custom_hosts_are_normalised_and_bad_entries_dropped() {
        let raw = HashMap::from([
            ("GitLab.Example.com:8443".to_string(), "gitlab".to_string()),
            ("git.acme.internal".to_string(), "Azure DevOps".to_string()),
            ("bad.example.com".to_string(), "gitea".to_string()),
            ("  ".to_string(), "github".to_string()),
        ]);
        let normalized = normalize_custom_hosts(&raw);
        assert_eq!(
            normalized.get("gitlab.example.com"),
            Some(&ProviderKind::GitLab),
            "the port is dropped so it matches a parsed hostname"
        );
        assert_eq!(
            normalized.get("git.acme.internal"),
            Some(&ProviderKind::AzureDevOps)
        );
        assert!(!normalized.contains_key("bad.example.com"));
        assert_eq!(normalized.len(), 2);
    }

    // ── multi-remote preference ──

    #[test]
    fn upstream_tracked_remote_wins_over_origin() {
        let remotes = vec![
            remote("origin", "git@github.com:acme/app.git"),
            remote("fork", "git@gitlab.com:me/app.git"),
        ];
        let upstream = upstream_remote_name("fork/feature", &remotes);
        assert_eq!(upstream, Some("fork"));
        let detected = detect_from_remotes(&remotes, upstream, &no_custom());
        assert_eq!(detected.kind, ProviderKind::GitLab);
        assert_eq!(detected.remote_name.as_deref(), Some("fork"));
    }

    #[test]
    fn upstream_name_resolution_prefers_the_longest_matching_remote() {
        let remotes = vec![
            remote("fork", "git@github.com:me/app.git"),
            remote("fork/mirror", "git@gitlab.com:me/app.git"),
        ];
        assert_eq!(
            upstream_remote_name("fork/mirror/main", &remotes),
            Some("fork/mirror"),
        );
        assert_eq!(upstream_remote_name("fork/main", &remotes), Some("fork"));
        assert_eq!(upstream_remote_name("other/main", &remotes), None);
    }

    #[test]
    fn origin_wins_when_no_upstream_is_tracked() {
        let remotes = vec![
            remote("mirror", "git@gitlab.com:me/app.git"),
            remote("origin", "git@github.com:acme/app.git"),
        ];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.kind, ProviderKind::GitHub);
        assert_eq!(detected.remote_name.as_deref(), Some("origin"));
    }

    #[test]
    fn a_stale_upstream_remote_name_falls_through_to_origin() {
        let remotes = vec![remote("origin", "git@github.com:acme/app.git")];
        let detected = detect_from_remotes(&remotes, Some("deleted-remote"), &no_custom());
        assert_eq!(detected.remote_name.as_deref(), Some("origin"));
        assert_eq!(detected.kind, ProviderKind::GitHub);
    }

    /// The gate this replaced matched *any* remote, so a repository whose
    /// `origin` is a mirror on an unrecognised box and whose second
    /// remote is on github.com lit up the PR UI. Preferring `origin`
    /// unconditionally would have silently taken that away.
    #[test]
    fn a_classifiable_secondary_beats_an_unclassifiable_origin() {
        let remotes = vec![
            remote("origin", "git@git.acme.internal:acme/app.git"),
            remote("hub", "git@github.com:acme/app.git"),
        ];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.remote_name.as_deref(), Some("hub"));
        assert_eq!(detected.kind, ProviderKind::GitHub);

        // With no `origin` at all, the same rule picks the same remote.
        let remotes = vec![
            remote("backup", "git@git.acme.internal:acme/app.git"),
            remote("hub", "git@github.com:acme/app.git"),
        ];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.remote_name.as_deref(), Some("hub"));
        assert_eq!(detected.kind, ProviderKind::GitHub);
    }

    /// …but only over an *unclassifiable* origin. When both classify,
    /// `origin` is still the conventional primary.
    #[test]
    fn origin_wins_over_a_secondary_when_both_classify() {
        let remotes = vec![
            remote("origin", "git@gitlab.com:acme/app.git"),
            remote("hub", "git@github.com:acme/app.git"),
        ];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.remote_name.as_deref(), Some("origin"));
        assert_eq!(detected.kind, ProviderKind::GitLab);
    }

    #[test]
    fn an_unclassifiable_upstream_does_not_veto_a_classifiable_origin() {
        let remotes = vec![
            remote("origin", "git@github.com:acme/app.git"),
            remote("fork", "git@git.acme.internal:me/app.git"),
        ];
        let detected = detect_from_remotes(&remotes, Some("fork"), &no_custom());
        assert_eq!(detected.remote_name.as_deref(), Some("origin"));
        assert_eq!(detected.kind, ProviderKind::GitHub);
    }

    #[test]
    fn falls_back_to_the_first_remote_when_nothing_classifies() {
        let remotes = vec![
            remote("backup", "git@git.acme.internal:acme/app.git"),
            remote("other", "git@code.acme.internal:acme/app.git"),
        ];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.kind, ProviderKind::Unknown);
        assert_eq!(detected.remote_name.as_deref(), Some("backup"));
        assert_eq!(detected.host.as_deref(), Some("git.acme.internal"));
    }

    #[test]
    fn no_remotes_detects_as_unknown_with_nothing_populated() {
        let detected = detect_from_remotes(&[], None, &no_custom());
        assert_eq!(detected, DetectedProvider::unknown());
    }

    #[test]
    fn filesystem_remote_reports_the_remote_but_no_host() {
        let remotes = vec![remote("origin", "/srv/git/app.git")];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        assert_eq!(detected.kind, ProviderKind::Unknown);
        assert_eq!(detected.host, None);
        assert_eq!(detected.base_url, None);
        assert_eq!(detected.remote_name.as_deref(), Some("origin"));
    }

    #[test]
    fn custom_host_mapping_drives_end_to_end_detection() {
        let remotes = vec![remote("origin", "https://git.acme.internal:8443/g/p.git")];
        let custom = HashMap::from([(
            "git.acme.internal".to_string(),
            ProviderKind::GitLab,
        )]);
        let detected = detect_from_remotes(&remotes, None, &custom);
        assert_eq!(detected.kind, ProviderKind::GitLab);
        assert_eq!(detected.base_url.as_deref(), Some("https://git.acme.internal:8443"));
    }

    // ── credential stripping ──

    #[test]
    fn detection_never_leaks_credentials_into_its_output() {
        let remotes = vec![remote(
            "origin",
            "https://oauth2:glpat-supersecret@gitlab.example.com:8443/g/p.git",
        )];
        let detected = detect_from_remotes(&remotes, None, &no_custom());
        let rendered = format!("{detected:?}");
        assert!(!rendered.contains("glpat-supersecret"), "{rendered}");
        assert!(!rendered.contains("oauth2"), "{rendered}");
        assert_eq!(detected.kind, ProviderKind::GitLab);
        assert_eq!(
            detected.base_url.as_deref(),
            Some("https://gitlab.example.com:8443")
        );
    }

    // ── on-disk behaviour ──

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git");
        assert!(
            status.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
    }

    #[test]
    fn detects_a_real_checkout_from_its_remote() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);
        git(
            repo,
            &["remote", "add", "origin", "git@github.com:acme/app.git"],
        );

        let detected = detect_provider(repo);
        assert_eq!(detected.kind, ProviderKind::GitHub);
        assert_eq!(detected.host.as_deref(), Some("github.com"));
        assert_eq!(detected.base_url.as_deref(), Some("https://github.com"));
        assert_eq!(detected.remote_name.as_deref(), Some("origin"));

        invalidate_detection_cache(Some(repo));
    }

    /// The cache must remember an answer, not a lack of one. A directory
    /// that is not a repository yet (or a git that failed to run) has to
    /// be re-asked on the next call — otherwise a freshly cloned or
    /// freshly initialised workspace would sit un-detected for a full TTL.
    #[test]
    fn failures_are_never_cached_but_successes_are() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path().join("not-a-repo-yet");
        std::fs::create_dir_all(&repo).unwrap();

        assert_eq!(detect_provider(&repo), DetectedProvider::unknown());

        git(&repo, &["init"]);
        git(
            &repo,
            &["remote", "add", "origin", "git@gitlab.com:g/p.git"],
        );
        assert_eq!(
            detect_provider(&repo).kind,
            ProviderKind::GitLab,
            "the failed probe must not have been cached"
        );

        // A *successful* probe is cached, so changing the remote is not
        // observed until the TTL lapses or the entry is dropped.
        git(&repo, &["remote", "set-url", "origin", "git@github.com:a/b.git"]);
        assert_eq!(detect_provider(&repo).kind, ProviderKind::GitLab);
        invalidate_detection_cache(Some(&repo));
        assert_eq!(detect_provider(&repo).kind, ProviderKind::GitHub);

        invalidate_detection_cache(Some(&repo));
    }

    /// An `Unknown` is "nothing classified *yet*" as often as it is a
    /// final answer — a clone whose remote is about to be added, or a
    /// self-hosted host waiting on a custom-host mapping. Caching it
    /// would leave the PR UI dark for a full TTL after the fix.
    #[test]
    fn an_unknown_classification_is_re_probed_rather_than_remembered() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);
        git(
            repo,
            &["remote", "add", "origin", "git@git.acme.internal:acme/app.git"],
        );
        assert_eq!(detect_provider(repo).kind, ProviderKind::Unknown);

        git(
            repo,
            &["remote", "set-url", "origin", "git@github.com:acme/app.git"],
        );
        assert_eq!(
            detect_provider(repo).kind,
            ProviderKind::GitHub,
            "the Unknown must not have been cached"
        );

        invalidate_detection_cache(Some(repo));
    }

    #[test]
    fn a_repo_with_no_remotes_detects_as_unknown() {
        let dir = tempfile::TempDir::new().unwrap();
        let repo = dir.path();
        git(repo, &["init"]);

        let detected = detect_provider(repo);
        assert_eq!(detected, DetectedProvider::unknown());

        invalidate_detection_cache(Some(repo));
    }
}
