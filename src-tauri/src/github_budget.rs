//! One process-wide budget for every GitHub read, including background jobs.
//!
//! Reserve quota and coalesce equal reads under a short-lived lock; perform
//! HTTP/CLI work outside it with bounded concurrency. A cold account has one
//! quota probe, and refreshes/writes fence late results without lifting pauses.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::git_provider::exec::{TimedFailure, TimedOutput};

#[path = "github_http.rs"]
mod http;

const PROBE_TTL: Duration = Duration::from_secs(60);
const MAX_CACHE_ENTRIES: usize = 256;
const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const MAX_HOSTS: usize = 128;
const MAX_PARALLEL_READS: usize = 4;
const MAX_BACKGROUND_READS: usize = 3;

#[derive(Clone, Copy, Debug)]
enum Bucket {
    Core,
    Graphql,
}

#[derive(Clone, Copy, Debug)]
struct Quota {
    limit: u64,
    remaining: u64,
    reset: u64,
    observed_remaining: u64,
}

#[derive(Clone)]
struct Snapshot {
    core: Quota,
    graphql: Quota,
    raw: String,
    fetched: Instant,
}

#[derive(Default)]
struct HostState {
    snapshot: Option<Snapshot>,
    pause_until: u64,
    refusals: u32,
    refusal_generation: u64,
    probe_error: Option<(Instant, String)>,
    probe_failures: u32,
    core_pause_until: u64,
    graphql_pause_until: u64,
    probe_running: bool,
    core_debt: ChargeDebt,
    graphql_debt: ChargeDebt,
    probe_boundary: Option<(ChargeDebt, ChargeDebt, u64, u64)>,
}

/// Fixed-size per-account ledger: cache eviction cannot forgive a charge.
#[derive(Clone, Copy, Default)]
struct ChargeDebt {
    confirmed: u64,
    uncertain: u64,
    uncertain_until: u64,
}

impl ChargeDebt {
    fn total(self) -> u64 {
        self.confirmed.saturating_add(self.uncertain)
    }

    fn add(&mut self, cost: u64, confirmed: bool, reset: u64) {
        if confirmed {
            self.confirmed = self.confirmed.saturating_add(cost);
        } else if cost > 0 {
            self.uncertain = self.uncertain.saturating_add(cost);
            // A timeout completing after its admitted epoch needs a new
            // epoch established before we can choose a safe reset boundary.
            let until = if reset > epoch() { reset } else { u64::MAX };
            self.uncertain_until = self.uncertain_until.max(until);
        }
    }

    fn reconcile(&mut self, boundary: Self, started: u64, reset: u64) {
        self.confirmed = self.confirmed.saturating_sub(boundary.confirmed);
        if self.uncertain_until == boundary.uncertain_until {
            if boundary.uncertain_until == u64::MAX {
                self.uncertain_until = reset;
            } else if started >= boundary.uncertain_until && reset > boundary.uncertain_until {
                self.uncertain = self.uncertain.saturating_sub(boundary.uncertain);
            }
        }
    }
}

struct Entry {
    expires: Instant,
    result: Result<String, String>,
    failures: u32,
    scope: u64,
    validator: Option<String>,
}

#[derive(Default)]
struct Coordinator {
    hosts: HashMap<(String, Option<u64>), HostState>,
    reads: HashMap<((String, Option<u64>), u64), Entry>,
    inflight: HashMap<ReadKey, InFlight>,
}

static COORDINATOR: OnceLock<Mutex<Coordinator>> = OnceLock::new();
static READ_CHANGED: OnceLock<Condvar> = OnceLock::new();

#[cfg(test)]
thread_local! {
    // Scheduling-only hooks for public fresh-read races; no transport is mocked.
    static FRESH_BOUNDARY_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
    static READ_WAIT_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> = const { std::cell::RefCell::new(None) };
}

struct Executed {
    result: Result<String, String>,
    headers: Option<reqwest::header::HeaderMap>,
    not_modified: bool,
}

impl Executed {
    fn plain(result: Result<String, String>) -> Self {
        Self {
            result,
            headers: None,
            not_modified: false,
        }
    }
}

#[cfg(test)]
fn run_read_with(
    host: &str,
    identity: Option<u64>,
    path: &Path,
    args: &[&str],
    deadline: Instant,
    execute: impl FnMut(&[&str], Option<&str>) -> Executed,
) -> Result<String, String> {
    run_read_mode(host, identity, path, args, deadline, false, None, execute)
}

fn run_read_mode(
    host: &str,
    identity: Option<u64>,
    path: &Path,
    args: &[&str],
    deadline: Instant,
    fresh: bool,
    declared_cost: Option<u64>,
    mut execute: impl FnMut(&[&str], Option<&str>) -> Executed,
) -> Result<String, String> {
    // Local Git resolution stays outside the shared state lock too.
    let key = read_key(path, args);
    let scope = scope_key(path, args);
    let changed = READ_CHANGED.get_or_init(Condvar::new);
    if fresh {
        // Fence both the cache and pre-boundary admission in one lock, using
        // the exact context/credential/key that the ensuing read will use.
        lock_until(deadline)?.invalidate_key(&((host.to_string(), identity), key));
        #[cfg(test)]
        FRESH_BOUNDARY_HOOK.with(|hook| {
            if let Some(hook) = hook.borrow_mut().take() {
                hook();
            }
        });
    }
    loop {
        let mut coordinator = lock_until(deadline)?;
        let admission = if let Some(cost) = declared_cost {
            coordinator.prepare_with_cost(
                host,
                identity,
                key,
                scope,
                args,
                foreground_read(args),
                cost,
            )
        } else {
            coordinator.prepare(host, identity, key, scope, args, foreground_read(args))
        };
        match admission {
            Admission::Complete(result) => return result,
            Admission::Probe => {
                drop(coordinator);
                let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute(&["api", "rate_limit", "--hostname", host], None)
                }))
                .unwrap_or_else(|_| Executed::plain(Err("GitHub quota worker panicked".into())));
                COORDINATOR
                    .get()
                    .unwrap()
                    .lock()
                    .map_err(|_| "GitHub budget lock poisoned")?
                    .probe_finished(host, identity, output);
                changed.notify_all();
            }
            Admission::Read(plan) => {
                drop(coordinator);
                let measured: Vec<&str> = plan.args.iter().map(String::as_str).collect();
                let output = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute(&measured, plan.validator.as_deref())
                }))
                .unwrap_or_else(|_| Executed::plain(Err("GitHub read worker panicked".into())));
                let result = COORDINATOR
                    .get()
                    .unwrap()
                    .lock()
                    .map_err(|_| "GitHub budget lock poisoned")?
                    .finish(plan, args, output);
                changed.notify_all();
                return result;
            }
            Admission::Wait => {
                #[cfg(test)]
                READ_WAIT_HOOK.with(|hook| {
                    if let Some(hook) = hook.borrow_mut().take() {
                        hook();
                    }
                });
                let (_guard, waited) = changed
                    .wait_timeout(coordinator, time_left(deadline)?)
                    .map_err(|_| "GitHub budget lock poisoned")?;
                if waited.timed_out() {
                    return Err("GitHub read timed out".into());
                }
            }
        }
    }
}

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn flag<'a>(args: &'a [&str], names: &[&str]) -> Option<&'a str> {
    args.iter().enumerate().find_map(|(i, arg)| {
        if names.contains(arg) {
            args.get(i + 1).copied()
        } else {
            names.iter().find_map(|name| {
                arg.strip_prefix(&format!("{name}=")).or_else(|| {
                    if name.len() == 2 {
                        arg.strip_prefix(name).filter(|s| !s.is_empty())
                    } else {
                        None
                    }
                })
            })
        }
    })
}

fn graphql_document<'a>(args: &'a [&str]) -> Option<&'a str> {
    args.iter().find_map(|a| {
        a.strip_prefix("query=")
            .or_else(|| a.strip_prefix("-fquery="))
            .or_else(|| a.strip_prefix("--raw-field=query="))
    })
}

fn clean_host(host: &str) -> Option<String> {
    let host = host.trim().to_ascii_lowercase();
    (!host.is_empty()
        && host.len() <= 253
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".-:".contains(c)))
    .then_some(host)
}

fn remote_repository(url: &str) -> Option<String> {
    let endpoint = crate::git_provider::detect::parse_remote_endpoint(url)?;
    let (host, path) = if url.contains("://") {
        let parsed = url::Url::parse(url).ok()?;
        let host = if matches!(parsed.scheme(), "http" | "https") {
            parsed.port().map_or_else(
                || endpoint.host.clone(),
                |port| format!("{}:{port}", endpoint.host),
            )
        } else {
            endpoint.host.clone()
        };
        (host, parsed.path().to_string())
    } else {
        (endpoint.host, url.split_once(':')?.1.to_string())
    };
    let path = path.trim_matches('/').trim_end_matches(".git");
    let parts: Vec<_> = path.split('/').collect();
    (parts.len() == 2 && parts.iter().all(|p| !p.is_empty())).then(|| format!("{host}/{path}"))
}

/// Resolve locally, then pin the operation and quota probe to the same host.
/// No remote URL (which can contain credentials) is kept in a cache key.
fn context(path: &Path, args: &[&str]) -> Result<(String, Vec<String>), String> {
    let env_repo = std::env::var("GH_REPO")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let explicit_repo = flag(args, &["--repo", "-R"]);
    let selected_repo = explicit_repo.or(env_repo.as_deref());
    let explicit_host = flag(args, &["--hostname"]);
    let detected = crate::git_provider::try_detect_provider(path).ok();
    let repository = if let Some(repo) = selected_repo {
        Some(if repo.contains("://") {
            remote_repository(repo).ok_or("Invalid GitHub repository selector")?
        } else {
            repo.to_string()
        })
    } else {
        detected
            .as_ref()
            .and_then(|d| d.remote_name.as_deref())
            .and_then(|remote| {
                crate::git_provider::detect::run_git(
                    path,
                    &["config", "--get", &format!("remote.{remote}.url")],
                )
            })
            .and_then(|url| remote_repository(&url))
    };
    let repo_host = selected_repo.and_then(|repo| {
        if repo.contains("://") {
            url::Url::parse(repo).ok().and_then(|u| {
                u.host_str().map(|host| {
                    u.port()
                        .map_or_else(|| host.to_string(), |port| format!("{host}:{port}"))
                })
            })
        } else if repo.split('/').count() == 3 {
            repo.split('/').next().map(str::to_string)
        } else {
            None
        }
    });
    let env_host = std::env::var("GH_HOST").ok();
    let host = explicit_host
        .map(str::to_string)
        .or(repo_host)
        .or_else(|| env_host.filter(|h| !h.trim().is_empty()))
        // OWNER/REPO selects the default host, not a host named OWNER.
        .or_else(|| selected_repo.map(|_| "github.com".to_string()))
        .or_else(|| {
            repository
                .as_ref()
                .and_then(|repo| repo.split('/').next().map(str::to_string))
        })
        .or_else(|| detected.and_then(|d| d.host))
        .unwrap_or_else(|| "github.com".into());
    let host = clean_host(&host).ok_or("Invalid GitHub hostname")?;
    let mut pinned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    if let (Some(explicit), Some(repo)) = (explicit_repo, repository.as_ref()) {
        let repo_path = if repo.split('/').count() == 3 {
            repo.split_once('/').ok_or("Invalid GitHub repository")?.1
        } else {
            repo.as_str()
        };
        let qualified = format!("{host}/{repo_path}");
        if let Some(index) = args.iter().position(|arg| *arg == explicit) {
            pinned[index] = qualified;
        } else if let Some(index) = args
            .iter()
            .position(|arg| arg.starts_with("--repo=") || arg.starts_with("-R="))
        {
            let name = args[index].split_once('=').unwrap().0;
            pinned[index] = format!("{name}={qualified}");
        }
    }
    if args.first() == Some(&"api") && explicit_host.is_none() {
        pinned.extend(["--hostname".into(), host.clone()]);
    } else if matches!(args.first(), Some(&"pr" | &"issue" | &"run")) && explicit_repo.is_none() {
        if let Some(repo) = repository {
            // GH_HOST explicitly supplied should not silently query a different host.
            let repo_path = if repo.split('/').count() == 3 {
                repo.split_once('/').ok_or("Invalid GitHub repository")?.1
            } else {
                repo.as_str()
            };
            pinned.extend(["--repo".into(), format!("{host}/{repo_path}")]);
        }
    } else if args.first() == Some(&"repo")
        && args.get(1) == Some(&"view")
        && args.get(2).is_none_or(|arg| arg.starts_with('-'))
    {
        if let Some(repo) = repository {
            let repo_path = if repo.split('/').count() == 3 {
                repo.split_once('/').ok_or("Invalid GitHub repository")?.1
            } else {
                repo.as_str()
            };
            pinned.push(format!("{host}/{repo_path}"));
        }
    }
    Ok((host, pinned))
}

/// Auth and budget probes must use the same offline host resolution as reads.
pub(crate) fn host_for_path(path: &Path) -> Result<String, String> {
    context(path, &["pr", "list"]).map(|(host, _)| host)
}

fn is_read(args: &[&str], stdin: Option<&str>) -> bool {
    if stdin.is_some() {
        return false;
    }
    match args.first().copied() {
        Some("pr" | "issue" | "repo" | "run") => matches!(
            args.get(1).copied(),
            Some("list" | "view" | "checks" | "diff")
        ),
        Some("api") => {
            if args.get(1) == Some(&"graphql") {
                return graphql_document(args)
                    .is_some_and(|q| !q.trim_start().starts_with("mutation"));
            }
            let method = flag(args, &["--method", "-X"]);
            method
                .map(|m| m.eq_ignore_ascii_case("GET"))
                .unwrap_or(!args.iter().any(|a| {
                    matches!(*a, "-f" | "-F" | "--field" | "--raw-field" | "--input")
                        || a.starts_with("-f")
                        || a.starts_with("-F")
                        || a.starts_with("--field=")
                        || a.starts_with("--raw-field=")
                        || a.starts_with("--input=")
                }))
        }
        _ => false,
    }
}

fn bucket(args: &[&str]) -> Bucket {
    if args.first() == Some(&"api") && args.get(1) != Some(&"graphql")
        || args.first() == Some(&"run")
    {
        Bucket::Core
    } else {
        Bucket::Graphql
    }
}

fn uses_core_too(args: &[&str]) -> bool {
    // High-level gh commands may resolve repository context with REST before
    // their GraphQL read. Direct GraphQL and REST calls keep separate budgets.
    matches!(args.first().copied(), Some("pr" | "issue" | "repo"))
}

fn read_key(path: &Path, args: &[&str]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(repo) = repository_scope(args) {
        repo.hash(&mut hasher);
        // gh resolves an omitted PR number from this checkout's branch.
        if args.first() == Some(&"pr")
            && matches!(args.get(1).copied(), Some("view" | "checks" | "diff"))
            && args.get(2).is_none_or(|a| a.starts_with('-'))
        {
            match crate::git_provider::detect::run_git(path, &["branch", "--show-current"]) {
                Some(branch) => branch.hash(&mut hasher),
                None => path.hash(&mut hasher),
            }
        }
    } else {
        path.hash(&mut hasher);
    }
    args.hash(&mut hasher);
    hasher.finish()
}

fn scope_key(path: &Path, args: &[&str]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    if let Some(repo) = repository_scope(args) {
        repo.hash(&mut hasher);
    } else {
        path.hash(&mut hasher);
    }
    hasher.finish()
}

fn repository_scope(args: &[&str]) -> Option<String> {
    if let Some(repo) = flag(args, &["--repo", "-R"]) {
        return Some(repo.to_ascii_lowercase());
    }
    let host = flag(args, &["--hostname"])?;
    if args.first() == Some(&"api") {
        let endpoint = args.get(1)?;
        if let Some(rest) = endpoint.strip_prefix("repos/") {
            let mut parts = rest.split('/');
            let owner = parts.next()?;
            let name = parts.next()?.split('?').next()?;
            return Some(format!("{host}/{owner}/{name}").to_ascii_lowercase());
        }
        if *endpoint == "graphql" {
            let owner = args.iter().find_map(|arg| arg.strip_prefix("owner="))?;
            let name = args.iter().find_map(|arg| arg.strip_prefix("name="))?;
            return Some(format!("{host}/{owner}/{name}").to_ascii_lowercase());
        }
    }
    None
}

fn foreground_read(args: &[&str]) -> bool {
    // Keep one transport slot for explicitly selected detail/check/diff work.
    // Priority changes concurrency only, never the reserved quota floor.
    matches!(
        (args.first().copied(), args.get(1).copied()),
        (Some("pr" | "issue"), Some("view" | "checks" | "diff"))
    )
}

fn ttl(args: &[&str]) -> Duration {
    let seconds = match (args.first().copied(), args.get(1).copied()) {
        (Some("pr"), Some("checks")) => 30,
        (Some("api"), _) => 60,
        (Some("issue"), _) => 120,
        (Some("pr"), Some("list")) => 120,
        (Some("pr"), Some("view" | "diff")) => 60,
        _ => 60,
    };
    Duration::from_secs(seconds)
}

fn estimated_cost(args: &[&str]) -> u64 {
    // gh can resolve a repository and fetch nested connections internally.
    // Budget conservatively instead of assuming a CLI invocation is one point.
    let rows = flag(args, &["--limit", "-L"])
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(if args.get(1) == Some(&"list") { 30 } else { 1 })
        .max(1);
    let fields = flag(args, &["--json"]).unwrap_or("");
    let nested = [
        "statusCheckRollup",
        "latestReviews",
        "reviewRequests",
        "comments",
    ]
    .iter()
    .filter(|f| fields.contains(**f))
    .count() as u64;
    if args.contains(&"--paginate") {
        500
    } else if args.first() == Some(&"api") && args.get(1) == Some(&"graphql") {
        // A page of threads with nested comments is many connection requests.
        // Each page is separately admitted; its response reports actual cost.
        200
    } else {
        rows.div_ceil(100)
            .saturating_mul(4)
            .saturating_add(rows.saturating_mul(nested.max(1)))
    }
}

fn parse_snapshot(raw: String) -> Result<Snapshot, String> {
    let value: serde_json::Value =
        serde_json::from_str(&raw).map_err(|_| "Invalid GitHub quota response")?;
    let read = |name: &str| -> Result<Quota, String> {
        let q = &value["resources"][name];
        let limit = q["limit"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("Missing GitHub quota limit")?;
        let remaining = q["remaining"]
            .as_u64()
            .filter(|n| *n <= limit)
            .ok_or("Missing or invalid GitHub quota remaining")?;
        let reset = q["reset"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or("Missing GitHub quota reset")?;
        Ok(Quota {
            limit,
            remaining,
            reset,
            observed_remaining: remaining,
        })
    };
    let core = read("core")?;
    let graphql = read("graphql")?;
    Ok(Snapshot {
        core,
        graphql,
        raw,
        fetched: Instant::now(),
    })
}

fn quota_pause(host: &str, until: u64) -> String {
    format!("GitHub API rate limit: requests to {host} paused until {until}")
}

fn refusal(message: &str) -> bool {
    let message = message.to_ascii_lowercase();
    message.contains("rate limit") || message.contains("ratelimit") || message.contains("http 429")
}

fn retry_epoch(message: &str, now: u64) -> Option<u64> {
    static RETRY: OnceLock<regex::Regex> = OnceLock::new();
    let re = RETRY.get_or_init(|| {
        regex::Regex::new(
            r"(?i)(retry-after\s*[:=]\s*|retry (?:in|after)\s+|x-ratelimit-reset\s*[:=]\s*)(\d+)",
        )
        .unwrap()
    });
    re.captures_iter(message)
        .filter_map(|c| {
            let n: u64 = c[2].parse().ok()?;
            if c[1].to_ascii_lowercase().contains("reset") {
                (n > now).then_some(n)
            } else {
                now.checked_add(n.max(60))
            }
        })
        .max()
}

fn record_refusal(state: &mut HostState, message: &str, now: u64) {
    state.refusal_generation = state.refusal_generation.wrapping_add(1);
    state.refusals = state.refusals.saturating_add(1);
    let delay = (60_u64.saturating_mul(1_u64 << state.refusals.saturating_sub(1).min(4))).min(900);
    let until = retry_epoch(message, now).unwrap_or(now.saturating_add(delay));
    state.pause_until = state.pause_until.max(until);
    // A refusal might be a primary exhaustion by another program. Re-probe
    // after the pause instead of reusing our previously healthy snapshot.
    state.snapshot = None;
}

type HostKey = (String, Option<u64>);
type ReadKey = (HostKey, u64);

struct InFlight {
    scope: u64,
    selected: Bucket,
    cost: u64,
    core_cost: u64,
    core_reset: u64,
    graphql_reset: u64,
    foreground: bool,
    superseded: bool,
}

struct ReadPlan {
    key: ReadKey,
    args: Vec<String>,
    validator: Option<String>,
    previous: Option<String>,
}

enum Admission {
    Complete(Result<String, String>),
    Probe,
    Read(ReadPlan),
    Wait,
}

impl Coordinator {
    fn invalidate_key(&mut self, key: &ReadKey) {
        if self
            .reads
            .get(key)
            .is_some_and(|entry| entry.result.is_ok())
        {
            self.reads.remove(key);
        }
        if let Some(flight) = self.inflight.get_mut(key) {
            flight.superseded = true;
        }
    }

    fn invalidate_scope(&mut self, account: &HostKey, scope: u64) {
        self.reads.retain(|(key, _), entry| {
            key != account || entry.scope != scope || entry.result.is_err()
        });
        for (_, flight) in self
            .inflight
            .iter_mut()
            .filter(|(key, flight)| &key.0 == account && flight.scope == scope)
        {
            flight.superseded = true;
        }
    }

    fn prepare(
        &mut self,
        host: &str,
        identity: Option<u64>,
        key: u64,
        scope: u64,
        args: &[&str],
        foreground: bool,
    ) -> Admission {
        self.prepare_with_cost(
            host,
            identity,
            key,
            scope,
            args,
            foreground,
            estimated_cost(args),
        )
    }

    fn prepare_with_cost(
        &mut self,
        host: &str,
        identity: Option<u64>,
        key: u64,
        scope: u64,
        args: &[&str],
        foreground: bool,
        declared_cost: u64,
    ) -> Admission {
        let host_key = (host.to_string(), identity);
        if !self.hosts.contains_key(&host_key) && self.hosts.len() >= MAX_HOSTS {
            return Admission::Complete(Err(
                "GitHub budget coordinator has too many active accounts".into(),
            ));
        }
        let now = epoch();
        let key = (host_key.clone(), key);
        let selected = bucket(args);
        let diagnostic = args.first() == Some(&"api") && args.get(1) == Some(&"rate_limit");
        let state = self.hosts.entry(host_key.clone()).or_default();
        if state.pause_until > now {
            return Admission::Complete(if diagnostic {
                state
                    .snapshot
                    .as_ref()
                    .map(|s| s.raw.clone())
                    .ok_or_else(|| quota_pause(host, state.pause_until))
            } else {
                Err(quota_pause(host, state.pause_until))
            });
        }
        if !diagnostic {
            if let Some(entry) = self
                .reads
                .get(&key)
                .filter(|entry| entry.expires > Instant::now())
            {
                return Admission::Complete(entry.result.clone());
            }
        }
        let state = self.hosts.get_mut(&host_key).unwrap();
        if diagnostic && (state.core_pause_until > now || state.graphql_pause_until > now) {
            if let Some(snapshot) = &state.snapshot {
                return Admission::Complete(Ok(snapshot.raw.clone()));
            }
        }
        if !diagnostic {
            let paused = match selected {
                Bucket::Core => state.core_pause_until,
                Bucket::Graphql => state.graphql_pause_until,
            }
            .max(if uses_core_too(args) {
                state.core_pause_until
            } else {
                0
            });
            if paused > now {
                return Admission::Complete(Err(quota_pause(host, paused)));
            }
        }
        if let Some((at, message)) = &state.probe_error {
            let delay = Duration::from_secs(
                (60_u64 << state.probe_failures.saturating_sub(1).min(4)).min(900),
            );
            if at.elapsed() < delay {
                return Admission::Complete(Err(message.clone()));
            }
        }
        let needs_probe = state.snapshot.as_ref().is_none_or(|snapshot| {
            snapshot.fetched.elapsed() >= PROBE_TTL
                || match selected {
                    Bucket::Core => snapshot.core.reset,
                    Bucket::Graphql => snapshot.graphql.reset,
                } <= now
        });
        if needs_probe && state.probe_running {
            return Admission::Wait;
        }
        if self.inflight.contains_key(&key) {
            return Admission::Wait;
        }
        let probes = self
            .hosts
            .values()
            .filter(|state| state.probe_running)
            .count();
        let background = self
            .inflight
            .values()
            .filter(|read| !read.foreground)
            .count()
            + probes;
        if self.inflight.len() + probes >= MAX_PARALLEL_READS
            || (!foreground && background >= MAX_BACKGROUND_READS)
        {
            return Admission::Wait;
        }
        let state = self.hosts.get_mut(&host_key).unwrap();
        if needs_probe {
            state.probe_running = true;
            state.probe_boundary = Some((
                state.core_debt,
                state.graphql_debt,
                now,
                state.refusal_generation,
            ));
            return Admission::Probe;
        }
        if diagnostic {
            return Admission::Complete(Ok(state.snapshot.as_ref().unwrap().raw.clone()));
        }
        let snapshot = state.snapshot.as_mut().unwrap();
        let cost = declared_cost;
        let core_cost = if uses_core_too(args) { 2 } else { 0 };
        let quota = match selected {
            Bucket::Core => snapshot.core,
            Bucket::Graphql => snapshot.graphql,
        };
        let floor = quota.limit.div_ceil(10);
        if quota.remaining < cost || quota.remaining.saturating_sub(cost) < floor {
            if quota.remaining > floor {
                return Admission::Complete(Err(format!("GitHub read needs {cost} estimated points but only {} are available above the 10% reserve", quota.remaining - floor)));
            }
            match selected {
                Bucket::Core => state.core_pause_until = quota.reset,
                Bucket::Graphql => state.graphql_pause_until = quota.reset,
            }
            return Admission::Complete(Err(quota_pause(host, quota.reset)));
        }
        let core_floor = snapshot.core.limit.div_ceil(10);
        if core_cost > 0
            && (snapshot.core.remaining < core_cost
                || snapshot.core.remaining.saturating_sub(core_cost) < core_floor)
        {
            if snapshot.core.remaining > core_floor {
                return Admission::Complete(Err(
                    "GitHub repository lookup cannot fit above the 10% core reserve".into(),
                ));
            }
            state.core_pause_until = snapshot.core.reset;
            return Admission::Complete(Err(quota_pause(host, snapshot.core.reset)));
        }
        match selected {
            Bucket::Core => snapshot.core.remaining -= cost,
            Bucket::Graphql => snapshot.graphql.remaining -= cost,
        }
        snapshot.core.remaining = snapshot.core.remaining.saturating_sub(core_cost);
        let mut measured: Vec<String> = args.iter().map(|arg| arg.to_string()).collect();
        if args.first() == Some(&"api") && args.get(1) == Some(&"graphql") {
            if let Some(query) = graphql_document(args).filter(|query| !query.contains("rateLimit"))
            {
                if let Some(end) = query.rfind('}') {
                    let metered = format!(
                        "{} rateLimit {{ cost limit remaining resetAt }} {}",
                        &query[..end],
                        &query[end..]
                    );
                    if let Some(index) = args.iter().position(|arg| arg.contains("query=")) {
                        let prefix = &args[index][..args[index].find("query=").unwrap()];
                        measured[index] = format!("{prefix}query={metered}");
                    }
                }
            }
        }
        let validator = self
            .reads
            .get(&key)
            .and_then(|entry| entry.validator.clone());
        let previous = self
            .reads
            .get(&key)
            .and_then(|entry| entry.result.as_ref().ok().cloned());
        self.inflight.insert(
            key.clone(),
            InFlight {
                scope,
                selected,
                cost,
                core_cost,
                core_reset: snapshot.core.reset,
                graphql_reset: snapshot.graphql.reset,
                foreground,
                superseded: false,
            },
        );
        Admission::Read(ReadPlan {
            key,
            args: measured,
            validator,
            previous,
        })
    }

    fn probe_finished(&mut self, host: &str, identity: Option<u64>, output: Executed) {
        let state = self.hosts.get_mut(&(host.to_string(), identity)).unwrap();
        state.probe_running = false;
        let boundary = state.probe_boundary.take();
        match output.result.and_then(parse_snapshot) {
            Ok(snapshot) => {
                // A read may have refused this account since probe admission.
                // Its older success cannot supply post-refusal quota evidence.
                if boundary
                    .is_some_and(|(_, _, _, generation)| generation != state.refusal_generation)
                {
                    return;
                }
                let core = snapshot.core;
                let graphql = snapshot.graphql;
                if let Some((core_debt, graphql_debt, started, _)) = boundary {
                    if state
                        .snapshot
                        .as_ref()
                        .is_none_or(|old| core.reset >= old.core.reset)
                    {
                        state.core_debt.reconcile(core_debt, started, core.reset);
                    }
                    if state
                        .snapshot
                        .as_ref()
                        .is_none_or(|old| graphql.reset >= old.graphql.reset)
                    {
                        state
                            .graphql_debt
                            .reconcile(graphql_debt, started, graphql.reset);
                    }
                }
                // A probe runs outside the lock: merge its observations rather
                // than replacing reservations or replies received meanwhile.
                if state.snapshot.is_none() {
                    state.snapshot = Some(snapshot);
                } else {
                    state.snapshot.as_mut().unwrap().fetched = snapshot.fetched;
                }
                state.probe_error = None;
                state.probe_failures = 0;
                let account = (host.to_string(), identity);
                self.observe(
                    &account,
                    Bucket::Core,
                    core.limit,
                    core.remaining,
                    core.reset,
                );
                self.observe(
                    &account,
                    Bucket::Graphql,
                    graphql.limit,
                    graphql.remaining,
                    graphql.reset,
                );
            }
            Err(mut message) => {
                if refusal(&message) {
                    record_refusal(state, &message, epoch());
                    message = quota_pause(host, state.pause_until);
                }
                state.probe_error = Some((Instant::now(), message));
                state.probe_failures = state.probe_failures.saturating_add(1);
            }
        }
    }

    fn observe(
        &mut self,
        host: &HostKey,
        selected: Bucket,
        limit: u64,
        remaining: u64,
        reset: u64,
    ) -> bool {
        if limit == 0 || remaining > limit || reset == 0 {
            return false;
        }
        let reserved = self
            .inflight
            .iter()
            .filter(|(key, _)| &key.0 == host)
            .map(|(_, read)| match selected {
                Bucket::Core => {
                    read.core_cost
                        + if matches!(read.selected, Bucket::Core) {
                            read.cost
                        } else {
                            0
                        }
                }
                Bucket::Graphql => {
                    if matches!(read.selected, Bucket::Graphql) {
                        read.cost
                    } else {
                        0
                    }
                }
            })
            .sum::<u64>();
        let Some(state) = self.hosts.get_mut(host) else {
            return false;
        };
        let debt = match selected {
            Bucket::Core => state.core_debt.total(),
            Bucket::Graphql => state.graphql_debt.total(),
        };
        let Some(snapshot) = state.snapshot.as_mut() else {
            return false;
        };
        let quota = match selected {
            Bucket::Core => &mut snapshot.core,
            Bucket::Graphql => &mut snapshot.graphql,
        };
        if reset < quota.reset {
            return false;
        }
        let observed_remaining = if reset == quota.reset {
            quota.observed_remaining.min(remaining)
        } else {
            remaining
        };
        *quota = Quota {
            limit,
            remaining: observed_remaining
                .saturating_sub(reserved)
                .saturating_sub(debt),
            reset,
            observed_remaining,
        };
        // Only inferred bucket pauses can be lifted by conservative quota
        // evidence; the account-wide refusal cooldown remains untouched.
        if quota.remaining > quota.limit.div_ceil(10) {
            match selected {
                Bucket::Core => state.core_pause_until = 0,
                Bucket::Graphql => state.graphql_pause_until = 0,
            }
        }
        if let Ok(mut raw) = serde_json::from_str::<serde_json::Value>(&snapshot.raw) {
            let resource = match selected {
                Bucket::Core => "core",
                Bucket::Graphql => "graphql",
            };
            raw["resources"][resource] =
                serde_json::json!({"limit": limit, "remaining": quota.remaining, "reset": reset});
            snapshot.raw = raw.to_string();
        }
        true
    }

    fn finish(
        &mut self,
        plan: ReadPlan,
        args: &[&str],
        output: Executed,
    ) -> Result<String, String> {
        let flight = self
            .inflight
            .remove(&plan.key)
            .expect("admitted GitHub read");
        let host_key = &plan.key.0;
        let host = &host_key.0;
        let mut core_observed = false;
        let mut graphql_observed = false;
        if let Some(headers) = &output.headers {
            let number = |name: &str| headers.get(name)?.to_str().ok()?.parse::<u64>().ok();
            let resource = headers
                .get("x-ratelimit-resource")
                .and_then(|value| value.to_str().ok());
            if let (Some(resource), Some(limit), Some(remaining), Some(reset)) = (
                resource,
                number("x-ratelimit-limit"),
                number("x-ratelimit-remaining"),
                number("x-ratelimit-reset"),
            ) {
                if let Some(selected) = match resource {
                    "core" => Some(Bucket::Core),
                    "graphql" => Some(Bucket::Graphql),
                    _ => None,
                } {
                    let observed = self.observe(host_key, selected, limit, remaining, reset);
                    match selected {
                        Bucket::Core => core_observed |= observed,
                        Bucket::Graphql => graphql_observed |= observed,
                    }
                }
            }
        }
        let validator = output
            .headers
            .as_ref()
            .and_then(|headers| headers.get("etag"))
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
            .or_else(|| {
                output
                    .not_modified
                    .then(|| plan.validator.clone())
                    .flatten()
            });
        let mut result = output.result;
        if output.not_modified {
            result = plan
                .previous
                .ok_or_else(|| "GitHub returned an unexpected conditional response".to_string());
        }
        if let Ok(raw) = &result {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
                let live = &value["data"]["rateLimit"];
                if let (Some(limit), Some(remaining), Some(reset), Some(_)) = (
                    live["limit"].as_u64(),
                    live["remaining"].as_u64(),
                    live["resetAt"]
                        .as_str()
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                        .and_then(|date| u64::try_from(date.timestamp()).ok()),
                    live["cost"].as_u64(),
                ) {
                    graphql_observed |=
                        self.observe(host_key, Bucket::Graphql, limit, remaining, reset);
                }
            }
        }
        let state = self.hosts.get_mut(host_key).unwrap();
        // Returned rows are not quota evidence: CLI context resolution and
        // nested reads may already have spent the full admission estimate.
        if !core_observed {
            let cost =
                flight
                    .core_cost
                    .saturating_add(if matches!(flight.selected, Bucket::Core) {
                        flight.cost
                    } else {
                        0
                    });
            state.core_debt.add(cost, result.is_ok(), flight.core_reset);
        }
        if !graphql_observed && matches!(flight.selected, Bucket::Graphql) {
            state
                .graphql_debt
                .add(flight.cost, result.is_ok(), flight.graphql_reset);
        }
        if let Err(message) = &result {
            if refusal(message) {
                record_refusal(state, message, epoch());
                result = Err(quota_pause(host, state.pause_until));
            }
        } else {
            state.refusals = 0;
        }
        if flight.superseded && result.is_ok() {
            return Err("GitHub read was superseded by a refresh or write; try again".into());
        }
        let failures = if result.is_err() {
            self.reads
                .get(&plan.key)
                .map_or(1, |entry| entry.failures.saturating_add(1))
        } else {
            0
        };
        let expiry = if failures > 0 {
            Duration::from_secs((60_u64 << failures.saturating_sub(1).min(4)).min(900))
        } else {
            ttl(args)
        };
        let size = result
            .as_ref()
            .map_or_else(|error| error.len(), |body| body.len());
        self.reads.retain(|_, entry| {
            entry.expires > Instant::now() || entry.failures > 0 || entry.validator.is_some()
        });
        if size <= MAX_ENTRY_BYTES {
            while !self.reads.is_empty()
                && (self.reads.len() >= MAX_CACHE_ENTRIES
                    || self.cache_bytes().saturating_add(size) > MAX_CACHE_BYTES)
            {
                let oldest = self
                    .reads
                    .iter()
                    .min_by_key(|(_, entry)| entry.expires)
                    .map(|(key, _)| key.clone())
                    .unwrap();
                self.reads.remove(&oldest);
            }
            if self.reads.len() < MAX_CACHE_ENTRIES
                && self.cache_bytes().saturating_add(size) <= MAX_CACHE_BYTES
            {
                self.reads.insert(
                    plan.key,
                    Entry {
                        expires: Instant::now() + expiry,
                        result: result.clone(),
                        failures,
                        scope: flight.scope,
                        validator,
                    },
                );
            }
        }
        result
    }

    fn cache_bytes(&self) -> usize {
        self.reads
            .values()
            .map(|entry| {
                entry
                    .result
                    .as_ref()
                    .map_or_else(|error| error.len(), |body| body.len())
            })
            .sum()
    }

    #[cfg(test)]
    fn read(
        &mut self,
        host: &str,
        identity: Option<u64>,
        path: &Path,
        args: &[&str],
        mut execute: impl FnMut(&[&str]) -> Result<String, String>,
    ) -> Result<String, String> {
        loop {
            match self.prepare(
                host,
                identity,
                read_key(path, args),
                scope_key(path, args),
                args,
                false,
            ) {
                Admission::Complete(result) => return result,
                Admission::Probe => self.probe_finished(
                    host,
                    identity,
                    Executed::plain(execute(&["api", "rate_limit", "--hostname", host])),
                ),
                Admission::Read(plan) => {
                    let measured: Vec<&str> = plan.args.iter().map(String::as_str).collect();
                    let output = Executed::plain(execute(&measured));
                    return self.finish(plan, args, output);
                }
                Admission::Wait => return Err("GitHub read is already running".into()),
            }
        }
    }

    fn mutation_result(
        &mut self,
        host: &str,
        identity: Option<u64>,
        result: &Result<String, String>,
    ) {
        let key = (host.to_string(), identity);
        if result.is_ok() {
            self.reads.retain(|(account, _), _| account != &key);
            for (_, flight) in self.inflight.iter_mut().filter(|(read, _)| read.0 == key) {
                flight.superseded = true;
            }
        } else if let Err(message) = result {
            if refusal(message) && (self.hosts.contains_key(&key) || self.hosts.len() < MAX_HOSTS) {
                record_refusal(self.hosts.entry(key).or_default(), message, epoch());
            }
        }
    }
}

fn execute(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    stdin: Option<&str>,
) -> Result<String, String> {
    let mut command = crate::execution::host_command("gh");
    command.args(args).current_dir(path);
    crate::execution::sanitize_gui_env_std_keep_dbus(&mut command);
    let output = if let Some(body) = stdin {
        run_with_stdin(command, body, timeout)
    } else {
        crate::git_provider::exec::run_timed(command, timeout)
    }
    .map_err(|failure| match failure {
        TimedFailure::Timeout => format!("gh command timed out after {}s", timeout.as_secs()),
        TimedFailure::Spawn(_) => "Failed to start GitHub CLI".into(),
        TimedFailure::Wait(_) => "Failed to wait for GitHub CLI".into(),
    })?;
    output_result(args, output)
}

fn output_result(args: &[&str], output: TimedOutput) -> Result<String, String> {
    if !output.success {
        let checks = args.first() == Some(&"pr") && args.get(1) == Some(&"checks");
        let stderr = output.stderr.to_ascii_lowercase();
        let auth_error = [
            "authentication",
            "bad credentials",
            "not logged in",
            "http 401",
            "http 403",
        ]
        .iter()
        .any(|message| stderr.contains(message));
        let valid_checks = checks
            && !auth_error
            && !refusal(&output.stderr)
            && serde_json::from_str::<serde_json::Value>(&output.stdout)
                .ok()
                .and_then(|v| {
                    v.as_array().map(|a| {
                        !a.is_empty()
                            && a.iter()
                                .all(|row| row["name"].is_string() && row["bucket"].is_string())
                    })
                })
                == Some(true);
        if !valid_checks {
            return Err(format!(
                "gh {} failed: {}",
                args.first().unwrap_or(&""),
                output.stderr.trim()
            ));
        }
    }
    Ok(output.stdout.trim_end().to_string())
}

/// API errors are never retried through gh: that would double-charge quota
/// and make a refusal on one transport invisible to other surfaces.
fn execute_api(
    host: &str,
    token: &str,
    args: &[&str],
    timeout: Duration,
    validator: Option<&str>,
) -> Executed {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    let request = match http::parse_request(host, args, None) {
        Ok(request) => request,
        Err(error) => return Executed::plain(Err(error)),
    };
    let client = CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| "Could not initialize GitHub HTTP client".to_string())
    });
    let response = match client
        .as_ref()
        .map_err(Clone::clone)
        .and_then(|client| http::send(client, request, token, timeout, validator))
    {
        Ok(response) => response,
        Err(error) => return Executed::plain(Err(error)),
    };
    api_response(args, response)
}

fn api_response(args: &[&str], response: http::Response) -> Executed {
    let not_modified = response.status == 304;
    let graphql = args.get(1) == Some(&"graphql");
    let error_message = || {
        let message = serde_json::from_str::<serde_json::Value>(&response.body)
            .ok()
            .and_then(|value| {
                value["message"].as_str().map(str::to_owned).or_else(|| {
                    value["errors"]
                        .as_array()
                        .and_then(|errors| errors.first())
                        .and_then(|error| error["message"].as_str())
                        .map(str::to_owned)
                })
            })
            .unwrap_or_else(|| "request rejected".into());
        let mut error = format!(
            "GitHub API read failed (HTTP {}): {}",
            response.status,
            message.chars().take(512).collect::<String>()
        );
        if let Some(retry) = response
            .headers
            .get("retry-after")
            .and_then(|value| value.to_str().ok())
        {
            error.push_str(&format!("; Retry-After: {retry}"));
        }
        if response.status == 429 {
            error.push_str("; rate limit exceeded");
        }
        error
    };
    let result = if not_modified {
        Ok(String::new())
    } else if !(200..300).contains(&response.status) {
        Err(error_message())
    } else if graphql {
        match serde_json::from_str::<serde_json::Value>(&response.body) {
            Ok(value)
                if value["errors"]
                    .as_array()
                    .is_some_and(|errors| !errors.is_empty()) =>
            {
                Err(error_message())
            }
            Ok(_) => Ok(response.body.trim_end().to_owned()),
            Err(_) => Err("Invalid GitHub GraphQL response".into()),
        }
    } else {
        Ok(response.body.trim_end().to_owned())
    };
    Executed {
        result,
        headers: Some(response.headers),
        not_modified,
    }
}

pub fn run(path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    run_inner(path, args, timeout, None)
}

/// Only trusted, statically bounded query builders may declare their cost.
/// Unknown/nested GraphQL still uses the conservative generic estimate.
pub(crate) fn run_bounded_graphql(
    path: &Path,
    args: &[&str],
    cost: u64,
    timeout: Duration,
) -> Result<String, String> {
    if args.first() != Some(&"api")
        || args.get(1) != Some(&"graphql")
        || !is_read(args, None)
        || args.contains(&"--paginate")
        || !(1..=200).contains(&cost)
    {
        return Err("Invalid bounded GitHub read".into());
    }
    run_inner_with_cost(path, args, timeout, None, false, Some(cost))
}

/// Revalidate a changing resource without discarding a cached failure or
/// relaxing the account's cooldown. Used by the live review-diff surface.
pub fn run_fresh(path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    run_inner_mode(path, args, timeout, None, true)
}

/// A deliberate refresh can bypass successful reads for this repository.
/// Keep failed reads and host pauses, so repeatedly pressing Refresh cannot
/// turn a refusal into a request storm.
pub fn invalidate_read_cache(path: &Path) -> Result<(), String> {
    let (host, args) = context(path, &["pr", "list"])?;
    let identity = crate::github::gh_credential_identity(&host);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let scope = scope_key(&path, &args);
    let host_key = (host, identity);
    COORDINATOR
        .get_or_init(|| Mutex::new(Coordinator::default()))
        .lock()
        .map_err(|_| "GitHub budget lock poisoned")?
        .invalidate_scope(&host_key, scope);
    Ok(())
}

pub fn run_stdin(
    path: &Path,
    args: &[&str],
    body: &str,
    timeout: Duration,
) -> Result<String, String> {
    run_inner(path, args, timeout, Some(body))
}

fn run_inner(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    stdin: Option<&str>,
) -> Result<String, String> {
    run_inner_mode(path, args, timeout, stdin, false)
}

fn run_inner_mode(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    stdin: Option<&str>,
    fresh: bool,
) -> Result<String, String> {
    run_inner_with_cost(path, args, timeout, stdin, fresh, None)
}

fn run_inner_with_cost(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    stdin: Option<&str>,
    fresh: bool,
    declared_cost: Option<u64>,
) -> Result<String, String> {
    let deadline = Instant::now() + timeout;
    if stdin.is_none() && is_read(args, None) && args.contains(&"--paginate") {
        return run_paginated(path, args, timeout, fresh);
    }
    let (host, pinned) = context(path, args)?;
    let token = if args.first() == Some(&"api") && is_read(args, stdin) {
        crate::github::gh_api_token(&host)
    } else {
        None
    };
    // Bind the request and cache identity to the SAME credential snapshot.
    let identity = if let Some(token) = &token {
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        token.hash(&mut hash);
        Some(hash.finish())
    } else {
        crate::github::gh_credential_identity(&host)
    };
    let pinned: Vec<&str> = pinned.iter().map(String::as_str).collect();
    if is_read(args, stdin) {
        let path: PathBuf = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        run_read_mode(
            &host,
            identity,
            &path,
            &pinned,
            deadline,
            fresh,
            declared_cost,
            |a, validator| match time_left(deadline) {
                Ok(remaining) => {
                    if let Some(token) = &token {
                        execute_api(&host, token, a, remaining, validator)
                    } else {
                        Executed::plain(execute(&path, a, remaining, None))
                    }
                }
                Err(error) => Executed::plain(Err(error)),
            },
        )
    } else {
        let result = execute(path, &pinned, time_left(deadline)?, stdin);
        COORDINATOR
            .get_or_init(|| Mutex::new(Coordinator::default()))
            .lock()
            .map_err(|_| "GitHub budget lock poisoned")?
            .mutation_result(&host, identity, &result);
        if result.is_ok() {
            crate::github::invalidate_github_reads();
        }
        result
    }
}

fn time_left(deadline: Instant) -> Result<Duration, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err("GitHub read timed out".into())
    } else {
        Ok(remaining)
    }
}

fn lock_until(deadline: Instant) -> Result<std::sync::MutexGuard<'static, Coordinator>, String> {
    let coordinator = COORDINATOR.get_or_init(|| Mutex::new(Coordinator::default()));
    loop {
        match coordinator.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err("GitHub budget lock poisoned".into())
            }
            Err(std::sync::TryLockError::WouldBlock) => {
                time_left(deadline)?;
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}

/// gh's automatic pagination can spend an arbitrary number of requests in a
/// single subprocess. Admit, meter and cache each page instead. Incomplete
/// reads are errors so callers retain their last complete result.
fn run_paginated(
    path: &Path,
    args: &[&str],
    timeout: Duration,
    fresh: bool,
) -> Result<String, String> {
    const MAX_PAGES: usize = 20;
    let deadline = Instant::now() + timeout;
    let base: Vec<String> = args
        .iter()
        .filter(|a| **a != "--paginate")
        .map(|a| a.to_string())
        .collect();
    let graphql = args.get(1) == Some(&"graphql");
    let mut cursor: Option<String> = None;
    let mut documents = Vec::new();
    let mut rows = Vec::new();
    for page in 1..=MAX_PAGES {
        let mut request = base.clone();
        if graphql {
            if let Some(cursor) = &cursor {
                request.extend(["-f".into(), format!("endCursor={cursor}")]);
            }
        } else {
            let endpoint = request.get_mut(1).ok_or("Missing GitHub API endpoint")?;
            let join = if endpoint.contains('?') { '&' } else { '?' };
            // These readers use array endpoints. Preserve existing query
            // parameters while selecting one explicitly bounded page.
            endpoint.push_str(&format!("{join}per_page=100&page={page}"));
        }
        let request: Vec<&str> = request.iter().map(String::as_str).collect();
        let raw = run_inner_mode(path, &request, time_left(deadline)?, None, fresh)?;
        let value: serde_json::Value =
            serde_json::from_str(&raw).map_err(|_| "Invalid GitHub paginated response")?;
        if graphql {
            let info = outer_page_info(&value).ok_or("Missing GitHub pagination state")?;
            let next = info["hasNextPage"]
                .as_bool()
                .ok_or("Invalid GitHub pagination state")?;
            cursor = info["endCursor"].as_str().map(str::to_string);
            if next && cursor.is_none() {
                return Err("Missing GitHub pagination cursor".into());
            }
            documents.push(raw);
            if !next {
                return Ok(documents.join("\n"));
            }
        } else {
            let page_rows = value.as_array().ok_or("Expected GitHub array page")?;
            let complete = page_rows.len() < 100;
            rows.extend(page_rows.iter().cloned());
            if complete {
                return serde_json::to_string(&rows)
                    .map_err(|_| "Failed to combine GitHub pages".into());
            }
        }
    }
    Err(
        "GitHub response exceeded the 20-page read limit; keeping the previous complete result"
            .into(),
    )
}

fn outer_page_info(value: &serde_json::Value) -> Option<&serde_json::Value> {
    fn find(value: &serde_json::Value, depth: usize) -> Option<(usize, &serde_json::Value)> {
        let object = value.as_object()?;
        if let Some(info) = object.get("pageInfo") {
            return Some((depth, info));
        }
        object
            .values()
            .filter_map(|child| find(child, depth + 1))
            .min_by_key(|(depth, _)| *depth)
    }
    find(value, 0).map(|(_, info)| info)
}

/// The shared timed runner uses null stdin. Mutations with JSON bodies need
/// the same drain/deadline/kill/reap contract while feeding their own pipe.
fn run_with_stdin(
    mut command: std::process::Command,
    body: &str,
    timeout: Duration,
) -> Result<TimedOutput, TimedFailure> {
    use std::io::{Read, Write};
    use std::process::Stdio;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(TimedFailure::Spawn)?;
    let deadline = Instant::now() + timeout;
    let input = child.stdin.take();
    let body = body.as_bytes().to_vec();
    let (written_tx, writer) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let result = input
            .ok_or_else(|| std::io::Error::other("gh stdin unavailable"))
            .and_then(|mut pipe| pipe.write_all(&body));
        let _ = written_tx.send(result);
    });
    let drain = |pipe: Option<Box<dyn Read + Send>>| {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            if let Some(mut pipe) = pipe {
                let _ = pipe.read_to_end(&mut bytes);
            }
            let _ = tx.send(String::from_utf8_lossy(&bytes).into_owned());
        });
        rx
    };
    let out = drain(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let err = drain(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let written = writer
                    .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                    .map_err(|_| TimedFailure::Timeout)?;
                written.map_err(TimedFailure::Wait)?;
                return Ok(TimedOutput {
                    success: status.success(),
                    stdout: out
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .map_err(|_| TimedFailure::Timeout)?,
                    stderr: err
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .map_err(|_| TimedFailure::Timeout)?,
                });
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(5)),
            other => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(match other {
                    Err(error) => TimedFailure::Wait(error),
                    _ => TimedFailure::Timeout,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_reservation_pause_recovers_graphql_headroom() {
        let args = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        for concurrent in [false, true] {
            let mut c = Coordinator::default();
            let host = "review-graphql-reservation.example.test";
            let account = (host.to_string(), Some(17));
            let initial = if concurrent { 900 } else { 700 };
            let snapshot = parse_snapshot(quota(4999, initial)).unwrap();
            let reset = snapshot.graphql.reset;
            c.hosts.entry(account.clone()).or_default().snapshot = Some(snapshot);
            let a = match c.prepare(host, Some(17), 1, 1, &args, false) {
                Admission::Read(plan) => plan,
                _ => panic!("A must reserve 200 points"),
            };
            let b = if concurrent {
                match c.prepare(host, Some(17), 2, 2, &args, false) {
                    Admission::Read(plan) => Some(plan),
                    _ => panic!("B must retain its separate reservation"),
                }
            } else {
                None
            };
            assert_eq!(
                c.hosts[&account]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .graphql
                    .remaining,
                500
            );
            assert!(
                !matches!(
                    c.prepare_with_cost(host, Some(17), 3, 3, &args, false, 2),
                    Admission::Read(_)
                ),
                "temporary reservations must still protect the reserve"
            );
            let date = chrono::DateTime::from_timestamp(reset as i64, 0)
                .unwrap()
                .to_rfc3339();
            c.finish(
                a,
                &args,
                Executed::plain(Ok(serde_json::json!({"data":{"rateLimit":{
                    "limit":5000,"remaining":initial - 1,"cost":1,"resetAt":date
                }}})
                .to_string())),
            )
            .unwrap();
            assert_eq!(
                c.hosts[&account]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .graphql
                    .remaining,
                699,
                "a live B reservation must remain subtracted"
            );
            assert!(matches!(c.prepare_with_cost(host, Some(17), 4, 4, &args, false, 2), Admission::Read(_)),
                "authoritative headroom must recover bounded discovery after a reservation-only pause");
            assert_eq!(
                c.hosts[&account]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .graphql
                    .remaining,
                697
            );
            if let Some(b) = b {
                c.finish(b, &args, metered_output("graphql", initial - 2, reset))
                    .unwrap();
                assert_eq!(
                    c.hosts[&account]
                        .snapshot
                        .as_ref()
                        .unwrap()
                        .graphql
                        .remaining,
                    896
                );
            }
        }
    }

    #[test]
    fn review_reservation_pause_recovers_core_lookup_headroom() {
        let mut c = Coordinator::default();
        let host = "review-core-reservation.example.test";
        let account = (host.to_string(), None);
        let snapshot = parse_snapshot(quota(700, 4999)).unwrap();
        let reset = snapshot.core.reset;
        c.hosts.entry(account.clone()).or_default().snapshot = Some(snapshot);
        let core = ["api", "repos/fixture/repo"];
        let cli = ["pr", "list", "--limit", "1", "--json", "number"];
        let a = match c.prepare_with_cost(host, None, 1, 1, &core, false, 200) {
            Admission::Read(plan) => plan,
            _ => panic!("A must reserve 200 core points"),
        };
        assert!(
            !matches!(c.prepare(host, None, 2, 2, &cli, false), Admission::Read(_)),
            "the two-point repository lookup must not spend the core floor"
        );
        c.finish(a, &core, metered_output("core", 699, reset))
            .unwrap();
        assert!(matches!(c.prepare(host, None, 3, 3, &cli, false), Admission::Read(_)),
            "authoritative core headroom must recover the repository lookup after a reservation-only pause");
        assert_eq!(
            c.hosts[&account].snapshot.as_ref().unwrap().core.remaining,
            697
        );
    }

    #[test]
    fn review_pre_refusal_probe_cannot_restore_snapshot_or_reconcile_debt() {
        let args = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        for late_success in [false, true] {
            let mut c = Coordinator::default();
            let host = "review-probe-refusal.example.test";
            let account = (host.to_string(), Some(23));
            c.hosts.entry(account.clone()).or_default().snapshot =
                Some(parse_snapshot(quota(4999, 1500)).unwrap());
            let admit = |c: &mut Coordinator, key| match c.prepare(
                host,
                Some(23),
                key,
                key,
                &args,
                false,
            ) {
                Admission::Read(plan) => plan,
                _ => panic!("read must be admitted"),
            };
            let completed = admit(&mut c, 1);
            c.finish(completed, &args, Executed::plain(Ok("[]".into())))
                .unwrap();
            let a = admit(&mut c, 2);
            let b = if late_success {
                Some(admit(&mut c, 3))
            } else {
                None
            };
            c.hosts
                .get_mut(&account)
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .fetched = Instant::now() - PROBE_TTL;
            assert!(matches!(
                c.prepare(host, Some(23), 4, 4, &args, false),
                Admission::Probe
            ));
            let sampled_before_refusal = quota(4999, 1300);
            assert!(c
                .finish(
                    a,
                    &args,
                    Executed::plain(Err("HTTP 429; Retry-After: 1".into()))
                )
                .is_err());
            if let Some(b) = b {
                c.finish(b, &args, Executed::plain(Ok("[]".into())))
                    .unwrap();
                assert_eq!(
                    c.hosts[&account].refusals, 0,
                    "the refusal streak is not a stable probe fence"
                );
            }
            assert!(c.hosts[&account].snapshot.is_none());
            let before = c.hosts[&account].graphql_debt;
            let pause = c.hosts[&account].pause_until;
            c.probe_finished(host, Some(23), Executed::plain(Ok(sampled_before_refusal)));
            let state = &c.hosts[&account];
            assert!(
                state.snapshot.is_none(),
                "a pre-refusal successful probe must not install a fresh snapshot"
            );
            assert_eq!(
                state.graphql_debt.confirmed, before.confirmed,
                "superseded quota evidence must not reconcile confirmed debt"
            );
            assert_eq!(state.graphql_debt.uncertain, before.uncertain);
            assert_eq!(state.graphql_debt.uncertain_until, before.uncertain_until);
            assert_eq!(state.pause_until, pause);
            assert!(
                !state.probe_running,
                "discarded probes must release the transport slot"
            );
            assert!(state.probe_boundary.is_none());
            assert!(
                matches!(
                    c.prepare(host, Some(23), 5, 5, &args, false),
                    Admission::Complete(Err(_))
                ),
                "discarding the probe must not lift the genuine refusal cooldown"
            );
            // Advance only the cooldown boundary, avoiding a real clock sleep.
            c.hosts.get_mut(&account).unwrap().pause_until = epoch() - 1;
            assert!(
                matches!(
                    c.prepare(host, Some(23), 6, 6, &args, false),
                    Admission::Probe
                ),
                "the first uncached read after cooldown must admit a post-refusal probe"
            );
            c.probe_finished(host, Some(23), Executed::plain(Ok(quota(4999, 1100))));
            assert!(c.hosts[&account].snapshot.is_some());
            assert!(!c.hosts[&account].probe_running);
            assert_eq!(c.hosts[&account].graphql_debt.confirmed, 0);
            assert_eq!(c.hosts[&account].graphql_debt.uncertain, before.uncertain);
        }
    }

    #[test]
    fn review_bucket_recovery_keeps_debt_floor_and_refusal_cooldown() {
        for selected in [Bucket::Core, Bucket::Graphql] {
            let mut c = Coordinator::default();
            let host = "review-pause-controls.example.test";
            let account = (host.to_string(), None);
            let snapshot = parse_snapshot(quota(500, 500)).unwrap();
            let reset = snapshot.core.reset;
            let state = c.hosts.entry(account.clone()).or_default();
            state.snapshot = Some(snapshot);
            state.core_pause_until = reset;
            state.graphql_pause_until = reset;
            state.pause_until = epoch() + 120;
            let cooldown = state.pause_until;
            let paused = |c: &Coordinator| match selected {
                Bucket::Core => c.hosts[&account].core_pause_until,
                Bucket::Graphql => c.hosts[&account].graphql_pause_until,
            };
            assert!(c.observe(&account, selected, 5000, 500, reset));
            assert_eq!(
                paused(&c),
                reset,
                "the exact reserve does not clear a pause"
            );
            assert!(!c.observe(&account, selected, 5000, 4999, reset - 1));
            assert_eq!(paused(&c), reset, "stale evidence cannot clear a pause");
            // Establish a newer epoch, but retain an unobserved charge.
            let state = c.hosts.get_mut(&account).unwrap();
            match selected {
                Bucket::Core => state.core_debt.confirmed = 200,
                Bucket::Graphql => state.graphql_debt.confirmed = 200,
            }
            assert!(c.observe(&account, selected, 5000, 700, reset + 1));
            assert_eq!(
                paused(&c),
                reset,
                "debt-adjusted headroom still stops at the floor"
            );
            assert!(c.observe(&account, selected, 5000, 699, reset + 1));
            assert_eq!(
                paused(&c),
                reset,
                "older remaining samples cannot refill quota"
            );
            assert!(c.observe(&account, selected, 5000, 701, reset + 2));
            assert_eq!(paused(&c), 0);
            assert_eq!(
                c.hosts[&account].pause_until, cooldown,
                "bucket recovery must not lift an account refusal cooldown"
            );
            let args = match selected {
                Bucket::Core => vec!["api", "repos/fixture/repo"],
                Bucket::Graphql => vec!["api", "graphql", "-f", "query=query { viewer { login } }"],
            };
            assert!(matches!(
                c.prepare_with_cost(host, None, 1, 1, &args, false, 2),
                Admission::Complete(Err(_))
            ));
            c.hosts.get_mut(&account).unwrap().pause_until = epoch() - 1;
            assert!(
                matches!(
                    c.prepare_with_cost(host, None, 2, 2, &args, false, 2),
                    Admission::Complete(Err(_))
                ),
                "one point of recovered headroom cannot admit a two-point read"
            );
        }
    }

    #[test]
    fn review_superseded_probe_wakes_existing_waiters() {
        let host = "review-probe-waiter.example.test";
        let account = (host.to_string(), Some(29));
        let active_args = ["api", "repos/fixture/active"];
        let a = {
            let mut c = lock_until(Instant::now() + Duration::from_secs(3)).unwrap();
            c.hosts.entry(account.clone()).or_default().snapshot =
                Some(parse_snapshot(quota(4999, 4999)).unwrap());
            let a = match c.prepare(host, Some(29), 1, 1, &active_args, false) {
                Admission::Read(plan) => plan,
                _ => panic!("A not admitted"),
            };
            c.hosts
                .get_mut(&account)
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .fetched = Instant::now() - PROBE_TTL;
            a
        };
        let (sample_tx, sample_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let (wait_tx, wait_rx) = std::sync::mpsc::channel();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::scope(|threads| {
            let probe = threads.spawn(move || {
                run_read_with(
                    host,
                    Some(29),
                    Path::new("/fixture-probe"),
                    &["api", "repos/fixture/probe"],
                    Instant::now() + Duration::from_secs(4),
                    |args, _| {
                        assert_eq!(args.get(1), Some(&"rate_limit"));
                        let sample = quota(4999, 4999);
                        sample_tx.send(()).unwrap();
                        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
                        Executed::plain(Ok(sample))
                    },
                )
            });
            sample_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let waiter = threads.spawn(move || {
                READ_WAIT_HOOK.with(|hook| {
                    *hook.borrow_mut() = Some(Box::new(move || {
                        wait_tx.send(()).unwrap();
                    }))
                });
                let result = run_read_with(
                    host,
                    Some(29),
                    Path::new("/fixture-waiter"),
                    &["api", "repos/fixture/waiter"],
                    Instant::now() + Duration::from_secs(4),
                    |_, _| panic!("cooldown must prevent waiter transport"),
                );
                done_tx.send(result).unwrap();
            });
            wait_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            // Deliberately do not notify here: only the real probe-completion
            // path can wake this already sleeping waiter.
            assert!(lock_until(Instant::now() + Duration::from_secs(2))
                .unwrap()
                .finish(a, &active_args, Executed::plain(Err("HTTP 429".into())))
                .is_err());
            release_tx.send(()).unwrap();
            let result = done_rx
                .recv_timeout(Duration::from_secs(2))
                .expect("discarded probe did not notify existing waiters");
            assert!(result.unwrap_err().contains("paused until"));
            assert!(probe.join().unwrap().unwrap_err().contains("paused until"));
            waiter.join().unwrap();
        });
        let c = lock_until(Instant::now() + Duration::from_secs(2)).unwrap();
        assert!(c.hosts[&account].snapshot.is_none());
        assert!(!c.hosts[&account].probe_running);
    }

    #[test]
    fn bounded_query_cost_cannot_be_used_for_writes_or_unbounded_reads() {
        let path = Path::new("/nonexistent-bounded-query-fixture");
        let query = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        for cost in [0, 201] {
            assert!(
                run_bounded_graphql(path, &query, cost, Duration::from_secs(1))
                    .unwrap_err()
                    .contains("Invalid bounded")
            );
        }
        for args in [
            vec!["pr", "view", "42"],
            vec!["api", "repos/fixture/repo"],
            vec!["api", "graphql", "-f", "query=mutation { writeSomething }"],
            vec![
                "api",
                "graphql",
                "--paginate",
                "-f",
                "query=query { viewer { login } }",
            ],
        ] {
            assert!(run_bounded_graphql(path, &args, 2, Duration::from_secs(1))
                .unwrap_err()
                .contains("Invalid bounded"));
        }
    }

    #[test]
    fn lightweight_bounded_queries_fit_without_spending_the_reserve() {
        let mut c = Coordinator::default();
        let host = "bounded-budget.example.test";
        let account = (host.to_string(), None);
        c.hosts.entry(account.clone()).or_default().snapshot =
            Some(parse_snapshot(quota(4999, 699)).unwrap());
        let args = [
            "api",
            "graphql",
            "-f",
            "query=query { repository { pullRequests(first:100) { nodes { number } } } }",
        ];
        // Large, unknown reads remain denied, without falsely drying up this
        // entire resource for the bounded discovery owner.
        assert!(matches!(
            c.prepare(host, None, 1, 1, &args, false),
            Admission::Complete(Err(_))
        ));
        let admitted = c.prepare_with_cost(host, None, 2, 1, &args, false, 2);
        assert!(
            matches!(admitted, Admission::Read(_)),
            "bounded flat discovery was charged as a nested 200-point query"
        );
        assert_eq!(
            c.hosts[&account]
                .snapshot
                .as_ref()
                .unwrap()
                .graphql
                .remaining,
            697
        );
        assert!(
            c.hosts[&account]
                .snapshot
                .as_ref()
                .unwrap()
                .graphql
                .remaining
                >= 500
        );
        let snapshot = c
            .hosts
            .get_mut(&account)
            .unwrap()
            .snapshot
            .as_mut()
            .unwrap();
        snapshot.graphql.remaining = 500;
        assert!(matches!(
            c.prepare_with_cost(host, None, 3, 1, &args, false, 2),
            Admission::Complete(Err(_))
        ));
    }

    #[test]
    fn overlapping_probe_preserves_reservations_and_newer_observations() {
        let args = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        for completed_during_probe in [false, true] {
            let mut c = Coordinator::default();
            let host = "probe-overlap.example.test";
            let account = (host.to_string(), None);
            let raw = quota(4999, 700);
            c.hosts.entry(account.clone()).or_default().snapshot =
                Some(parse_snapshot(raw.clone()).unwrap());
            let a = match c.prepare(host, None, 1, 1, &args, false) {
                Admission::Read(plan) => plan,
                _ => panic!("first read must be admitted"),
            };
            c.hosts
                .get_mut(&account)
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .fetched = Instant::now() - PROBE_TTL;
            assert!(matches!(
                c.prepare(host, None, 2, 2, &args, false),
                Admission::Probe
            ));
            if completed_during_probe {
                let reset = c.hosts[&account].snapshot.as_ref().unwrap().graphql.reset;
                let date = chrono::DateTime::from_timestamp(reset as i64, 0)
                    .unwrap()
                    .to_rfc3339();
                c.finish(
                    a,
                    &args,
                    Executed::plain(Ok(serde_json::json!({"data":{"rateLimit":{
                        "limit":5000,"remaining":500,"cost":200,"resetAt":date
                    }}})
                    .to_string())),
                )
                .unwrap();
            }
            // The probe was sampled before A's charge and arrived later.
            c.probe_finished(host, None, Executed::plain(Ok(raw)));
            assert_eq!(
                c.hosts[&account]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .graphql
                    .remaining,
                500,
                "a late probe must not replace either a live reservation or newer metering"
            );
            assert!(
                matches!(
                    c.prepare(host, None, 2, 2, &args, false),
                    Admission::Complete(Err(_))
                ),
                "the second 200-point read must not spend the 500-point reserve"
            );
        }
    }

    fn metered_output(resource: &str, remaining: u64, reset: u64) -> Executed {
        let mut headers = reqwest::header::HeaderMap::new();
        for (name, value) in [
            ("x-ratelimit-resource", resource.to_string()),
            ("x-ratelimit-limit", "5000".to_string()),
            ("x-ratelimit-remaining", remaining.to_string()),
            ("x-ratelimit-reset", reset.to_string()),
        ] {
            headers.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        Executed {
            result: Ok("[]".into()),
            headers: Some(headers),
            not_modified: false,
        }
    }

    #[test]
    fn delayed_metering_cannot_forgive_completed_unobserved_reads() {
        let graphql = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        let cli = ["pr", "list", "--limit", "192", "--json", "number"];
        assert_eq!(estimated_cost(&cli), 200);
        for kind in ["timeout", "unmetered-api", "unmetered-cli"] {
            let mut c = Coordinator::default();
            let host = "charge-debt.example.test";
            let account = (host.to_string(), None);
            let snapshot = parse_snapshot(quota(4999, 900)).unwrap();
            let reset = snapshot.graphql.reset;
            c.hosts.entry(account.clone()).or_default().snapshot = Some(snapshot);
            let a_args = if kind == "unmetered-cli" {
                &cli[..]
            } else {
                &graphql[..]
            };
            let a = match c.prepare(host, None, 1, 1, a_args, false) {
                Admission::Read(plan) => plan,
                _ => panic!("A not admitted"),
            };
            let b = match c.prepare(host, None, 2, 2, &graphql, false) {
                Admission::Read(plan) => plan,
                _ => panic!("B not admitted"),
            };
            assert_eq!(
                c.hosts[&account]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .graphql
                    .remaining,
                500
            );
            let a_result = if kind == "timeout" {
                Err("transport timed out".into())
            } else {
                Ok("[]".into())
            };
            let _ = c.finish(a, a_args, Executed::plain(a_result));
            // B's 700-point report was sampled before A was charged.
            c.finish(b, &graphql, metered_output("graphql", 700, reset))
                .unwrap();
            assert_eq!(c.hosts[&account].snapshot.as_ref().unwrap().graphql.remaining, 500,
                "{kind}: completed reads without quota evidence must keep their conservative charge");
            assert!(
                matches!(
                    c.prepare(host, None, 3, 3, &graphql, false),
                    Admission::Complete(Err(_))
                ),
                "{kind}: C must not consume the reserve"
            );
            if kind == "unmetered-cli" {
                c.observe(&account, Bucket::Core, 5000, 4999, reset);
                assert_eq!(
                    c.hosts[&account].snapshot.as_ref().unwrap().core.remaining,
                    4997,
                    "the unobserved CLI repository lookup also keeps its charge"
                );
            }
        }
    }

    #[test]
    fn probes_reconcile_only_preboundary_confirmed_charges_not_uncertain_timeouts() {
        let args = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        for timeout in [false, true] {
            let mut c = Coordinator::default();
            let host = "debt-probe.example.test";
            let account = (host.to_string(), None);
            c.hosts.entry(account.clone()).or_default().snapshot =
                Some(parse_snapshot(quota(4999, 1100)).unwrap());
            let admit =
                |c: &mut Coordinator, key| match c.prepare(host, None, key, key, &args, false) {
                    Admission::Read(plan) => plan,
                    _ => panic!("read not admitted"),
                };
            let a = admit(&mut c, 1);
            let _ = c.finish(
                a,
                &args,
                Executed::plain(if timeout {
                    Err("timeout".into())
                } else {
                    Ok("[]".into())
                }),
            );
            let b = admit(&mut c, 2);
            c.hosts
                .get_mut(&account)
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .fetched = Instant::now() - PROBE_TTL;
            assert!(matches!(
                c.prepare(host, None, 3, 3, &args, false),
                Admission::Probe
            ));
            // B completes after the probe boundary; a pre-B sample cannot reconcile it.
            c.finish(b, &args, Executed::plain(Ok("[]".into())))
                .unwrap();
            c.probe_finished(host, None, Executed::plain(Ok(quota(4999, 900))));
            let expected = if timeout { 500 } else { 700 };
            assert_eq!(
                c.hosts[&account]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .graphql
                    .remaining,
                expected,
                "fresh probes reconcile confirmed preboundary completions only"
            );
            // Another same-epoch probe now includes B, but cannot settle an uncertain A.
            c.hosts
                .get_mut(&account)
                .unwrap()
                .snapshot
                .as_mut()
                .unwrap()
                .fetched = Instant::now() - PROBE_TTL;
            assert!(matches!(
                c.prepare(host, None, 4, 4, &args, false),
                Admission::Probe
            ));
            c.probe_finished(host, None, Executed::plain(Ok(quota(4999, 700))));
            assert_eq!(
                c.hosts[&account]
                    .snapshot
                    .as_ref()
                    .unwrap()
                    .graphql
                    .remaining,
                expected
            );
        }
    }

    #[test]
    fn uncertain_charge_survives_cache_eviction_and_read_headers_until_a_safe_probe_epoch() {
        let mut c = Coordinator::default();
        let host = "safe-epoch.example.test";
        let account = (host.to_string(), None);
        let args = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        c.hosts.entry(account.clone()).or_default().snapshot =
            Some(parse_snapshot(quota(4999, 900)).unwrap());
        let a = match c.prepare(host, None, 1, 1, &args, false) {
            Admission::Read(plan) => plan,
            _ => panic!("not admitted"),
        };
        assert!(c
            .finish(a, &args, Executed::plain(Err("timeout".into())))
            .is_err());
        c.reads.clear();
        let reset = c.hosts[&account].snapshot.as_ref().unwrap().graphql.reset;
        c.observe(&account, Bucket::Graphql, 5000, 900, reset);
        assert_eq!(
            c.hosts[&account]
                .snapshot
                .as_ref()
                .unwrap()
                .graphql
                .remaining,
            700
        );
        // Advancing a quota timestamp, like fetched above, avoids a clock sleep.
        let past_reset = epoch() - 1;
        let state = c.hosts.get_mut(&account).unwrap();
        state.graphql_debt.uncertain_until = past_reset;
        state.snapshot.as_mut().unwrap().graphql.reset = past_reset;
        assert!(matches!(
            c.prepare(host, None, 2, 2, &args, false),
            Admission::Probe
        ));
        c.probe_finished(host, None, Executed::plain(Ok(quota(4999, 4999))));
        assert_eq!(
            c.hosts[&account]
                .snapshot
                .as_ref()
                .unwrap()
                .graphql
                .remaining,
            4999,
            "only a probe begun after the protected reset can settle old uncertainty"
        );
        assert_eq!(c.hosts[&account].graphql_debt.total(), 0);
    }

    #[test]
    fn timeout_completing_across_reset_requires_an_established_then_safe_epoch() {
        let mut c = Coordinator::default();
        let host = "cross-epoch.example.test";
        let account = (host.to_string(), None);
        let args = ["api", "graphql", "-f", "query=query { viewer { login } }"];
        c.hosts.entry(account.clone()).or_default().snapshot =
            Some(parse_snapshot(quota(4999, 900)).unwrap());
        let a = match c.prepare(host, None, 1, 1, &args, false) {
            Admission::Read(plan) => plan,
            _ => panic!("not admitted"),
        };
        c.inflight.get_mut(&a.key).unwrap().graphql_reset = epoch() - 1;
        assert!(c
            .finish(a, &args, Executed::plain(Err("timeout".into())))
            .is_err());
        c.hosts
            .get_mut(&account)
            .unwrap()
            .snapshot
            .as_mut()
            .unwrap()
            .fetched = Instant::now() - PROBE_TTL;
        assert!(matches!(
            c.prepare(host, None, 2, 2, &args, false),
            Admission::Probe
        ));
        let raw = quota(4999, 900);
        let next_reset = parse_snapshot(raw.clone()).unwrap().graphql.reset;
        c.probe_finished(host, None, Executed::plain(Ok(raw)));
        assert_eq!(
            c.hosts[&account]
                .snapshot
                .as_ref()
                .unwrap()
                .graphql
                .remaining,
            700
        );
        assert_eq!(c.hosts[&account].graphql_debt.uncertain_until, next_reset);
        c.hosts
            .get_mut(&account)
            .unwrap()
            .snapshot
            .as_mut()
            .unwrap()
            .fetched = Instant::now() - PROBE_TTL;
        assert!(matches!(
            c.prepare(host, None, 3, 3, &args, false),
            Admission::Probe
        ));
        c.probe_finished(host, None, Executed::plain(Ok(quota(4999, 900))));
        assert_eq!(
            c.hosts[&account]
                .snapshot
                .as_ref()
                .unwrap()
                .graphql
                .remaining,
            700
        );
    }

    #[cfg(unix)]
    #[test]
    fn fresh_read_requires_postboundary_work() {
        use std::os::unix::fs::PermissionsExt;
        if std::env::var_os("CODEMUX_BUDGET_FRESH_FIXTURE").is_none() {
            // PATH, HOME and credential isolation apply to this child only.
            let fixture = tempfile::tempdir().unwrap();
            let gh = fixture.path().join("gh");
            std::fs::write(&gh, "#!/bin/sh\ncase \"$1\" in auth|config) exit 1;; pr) printf 'read\\n' >> calls; printf fresh;; *) exit 99;; esac\n").unwrap();
            std::fs::set_permissions(&gh, std::fs::Permissions::from_mode(0o755)).unwrap();
            let mut child = std::process::Command::new(std::env::current_exe().unwrap());
            child
                .args([
                    "--exact",
                    "github_budget::tests::fresh_read_requires_postboundary_work",
                    "--test-threads=1",
                    "--nocapture",
                ])
                .env("CODEMUX_BUDGET_FRESH_FIXTURE", fixture.path())
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        fixture.path().display(),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                )
                .env("HOME", fixture.path())
                .env("GH_CONFIG_DIR", fixture.path());
            for key in [
                "GH_TOKEN",
                "GITHUB_TOKEN",
                "GH_ENTERPRISE_TOKEN",
                "GITHUB_ENTERPRISE_TOKEN",
                "GH_HOST",
                "GH_REPO",
            ] {
                child.env_remove(key);
            }
            let output = child.output().unwrap();
            assert!(
                output.status.success(),
                "public fresh-read fixture failed:\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let root = PathBuf::from(std::env::var_os("CODEMUX_BUDGET_FRESH_FIXTURE").unwrap());
        let schedules = if std::env::var_os("CODEMUX_BUDGET_FRESH_WINDOW_FIRST").is_some() {
            [true, false]
        } else {
            [false, true]
        };
        for finish_in_window in schedules {
            let path = root.join(if finish_in_window {
                "window"
            } else {
                "inflight"
            });
            std::fs::create_dir(&path).unwrap();
            let host = if finish_in_window {
                "fresh-window.example.test"
            } else {
                "fresh-inflight.example.test"
            };
            let repo = format!("{host}/fixture/repo");
            let args = ["pr", "list", "--limit", "1", "--repo", &repo];
            let (resolved, pinned) = context(&path, &args).unwrap();
            let identity = crate::github::gh_credential_identity(&resolved);
            let pinned: Vec<&str> = pinned.iter().map(String::as_str).collect();
            let account = (resolved.clone(), identity);
            let key = read_key(&path, &pinned);
            let a = {
                let mut c = lock_until(Instant::now() + Duration::from_secs(3)).unwrap();
                c.hosts.entry(account.clone()).or_default().snapshot =
                    Some(parse_snapshot(quota(4999, 4999)).unwrap());
                match c.prepare(
                    &resolved,
                    identity,
                    key,
                    scope_key(&path, &pinned),
                    &pinned,
                    false,
                ) {
                    Admission::Read(plan) => plan,
                    _ => panic!("A not admitted"),
                }
            };
            let (boundary_tx, boundary_rx) = std::sync::mpsc::channel();
            let (resume_tx, resume_rx) = std::sync::mpsc::channel();
            let (wait_tx, wait_rx) = std::sync::mpsc::channel();
            let fresh = std::thread::scope(|threads| {
                let b = threads.spawn(|| {
                    FRESH_BOUNDARY_HOOK.with(|hook| {
                        *hook.borrow_mut() = Some(Box::new(move || {
                            boundary_tx.send(()).unwrap();
                            if finish_in_window {
                                resume_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                            }
                        }))
                    });
                    READ_WAIT_HOOK.with(|hook| {
                        *hook.borrow_mut() = Some(Box::new(move || {
                            wait_tx.send(()).unwrap();
                        }))
                    });
                    run_fresh(&path, &args, Duration::from_secs(3))
                });
                boundary_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                if !finish_in_window {
                    wait_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                }
                let _ = lock_until(Instant::now() + Duration::from_secs(2))
                    .unwrap()
                    .finish(a, &pinned, Executed::plain(Ok("old response".into())));
                READ_CHANGED.get_or_init(Condvar::new).notify_all();
                if finish_in_window {
                    resume_tx.send(()).unwrap();
                }
                b.join().unwrap()
            });
            assert_eq!(fresh, Ok("fresh".into()),
                "fresh must neither coalesce with nor reuse pre-boundary work (window={finish_in_window})");
            assert_eq!(
                std::fs::read_to_string(path.join("calls")).unwrap(),
                "read\n"
            );
            let failure = {
                let mut c = lock_until(Instant::now() + Duration::from_secs(2)).unwrap();
                c.reads.remove(&(account.clone(), key));
                let plan = match c.prepare(&resolved, identity, key, 1, &pinned, false) {
                    Admission::Read(plan) => plan,
                    _ => panic!("failure read not admitted"),
                };
                c.finish(
                    plan,
                    &pinned,
                    Executed::plain(Err("offline fixture".into())),
                )
                .unwrap_err()
            };
            assert_eq!(
                run_fresh(&path, &args, Duration::from_secs(2)),
                Err(failure)
            );
            lock_until(Instant::now() + Duration::from_secs(2))
                .unwrap()
                .hosts
                .get_mut(&account)
                .unwrap()
                .pause_until = epoch() + 120;
            assert!(run_fresh(&path, &args, Duration::from_secs(2))
                .unwrap_err()
                .contains("paused"));
            assert_eq!(
                std::fs::read_to_string(path.join("calls")).unwrap(),
                "read\n",
                "fresh must not bypass failed cache or cooldown"
            );
        }
    }

    #[test]
    fn native_responses_keep_headers_and_reject_partial_graphql_or_rate_refusals() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("retry-after", "120".parse().unwrap());
        let output = api_response(
            &["api", "repos/fixture/repo"],
            http::Response {
                status: 429,
                headers: headers.clone(),
                body: r#"{"message":"slow down"}"#.into(),
            },
        );
        let message = output.result.unwrap_err();
        assert!(refusal(&message));
        let now = epoch();
        assert_eq!(retry_epoch(&message, now), Some(now + 120));
        assert_eq!(output.headers.unwrap()["retry-after"], "120");
        let partial = api_response(
            &["api", "graphql"],
            http::Response {
                status: 200,
                headers: headers.clone(),
                body: r#"{"data":{"repository":null},"errors":[{"message":"Unavailable"}]}"#.into(),
            },
        );
        assert!(partial.result.unwrap_err().contains("Unavailable"));
        let unchanged = api_response(
            &["api", "repos/fixture/repo"],
            http::Response {
                status: 304,
                headers: headers.clone(),
                body: String::new(),
            },
        );
        assert!(unchanged.not_modified);
        assert_eq!(unchanged.result.unwrap(), "");
        let success = api_response(
            &["api", "repos/fixture/repo"],
            http::Response {
                status: 200,
                headers,
                body: "[1,2]\n".into(),
            },
        );
        assert_eq!(success.result.unwrap(), "[1,2]");
    }

    #[test]
    fn foreground_reads_keep_the_same_quota_reserve() {
        let mut c = Coordinator::default();
        let host = "foreground-reserve.example.test";
        c.hosts.entry((host.into(), None)).or_default().snapshot =
            Some(parse_snapshot(quota(4999, 699)).unwrap());
        let args = [
            "api",
            "graphql",
            "-f",
            "query=query { reviewThreads(first:50) { nodes { id } } }",
        ];
        assert!(
            matches!(
                c.prepare(host, None, 1, 1, &args, true),
                Admission::Complete(Err(_))
            ),
            "foreground admission must not spend the reserved 10%"
        );
    }

    #[test]
    fn refresh_fences_a_late_success_but_keeps_errors_and_other_repositories() {
        let mut c = Coordinator::default();
        let host = "refresh-race.example.test";
        let account = (host.to_string(), Some(7));
        c.hosts.entry(account.clone()).or_default().snapshot =
            Some(parse_snapshot(quota(4999, 4999)).unwrap());
        let args = ["api", "repos/fixture/repo", "--hostname", host];
        let plan = match c.prepare(host, Some(7), 1, 10, &args, false) {
            Admission::Read(plan) => plan,
            _ => panic!("read was not admitted"),
        };
        for (key, scope, result) in [
            (2, 10, Err("network failure".into())),
            (3, 20, Ok("neighbor".into())),
        ] {
            c.reads.insert(
                (account.clone(), key),
                Entry {
                    expires: Instant::now() + Duration::from_secs(60),
                    result,
                    failures: 0,
                    scope,
                    validator: None,
                },
            );
        }
        c.invalidate_scope(&account, 10);
        assert!(
            c.finish(plan, &args, Executed::plain(Ok("old response".into())))
                .is_err(),
            "a pre-refresh success must not repopulate the cache"
        );
        assert!(!c.reads.contains_key(&(account.clone(), 1)));
        assert!(c.reads[&(account.clone(), 2)].result.is_err());
        assert_eq!(c.reads[&(account, 3)].result.as_ref().unwrap(), "neighbor");
    }

    #[test]
    fn header_metering_reuses_conditional_data_and_never_increases_from_an_older_reply() {
        let mut c = Coordinator::default();
        let host = "metering.example.test";
        let account = (host.to_string(), Some(7));
        let args = ["api", "repos/fixture/repo", "--hostname", host];
        let mut snapshot = parse_snapshot(quota(4999, 4999)).unwrap();
        // Keep one epoch for every observation in this test.
        let reset = snapshot.core.reset;
        snapshot.core.remaining = 4999;
        c.hosts.entry(account.clone()).or_default().snapshot = Some(snapshot);
        let admit = |c: &mut Coordinator, key| match c.prepare(host, Some(7), key, 1, &args, false)
        {
            Admission::Read(plan) => plan,
            _ => panic!("read was not admitted"),
        };
        let first = admit(&mut c, 1);
        let second = admit(&mut c, 2);
        let response = |remaining: u64, conditional| {
            let mut headers = reqwest::header::HeaderMap::new();
            for (name, value) in [
                ("x-ratelimit-resource", "core".to_string()),
                ("x-ratelimit-limit", "5000".to_string()),
                ("x-ratelimit-remaining", remaining.to_string()),
                ("x-ratelimit-reset", reset.to_string()),
                ("etag", "\"fixture\"".into()),
            ] {
                headers.insert(
                    reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                    value.parse().unwrap(),
                );
            }
            Executed {
                result: Ok(if conditional {
                    "".into()
                } else {
                    "[42]".into()
                }),
                headers: Some(headers),
                not_modified: conditional,
            }
        };
        assert_eq!(
            c.finish(second, &args, response(4997, false)).unwrap(),
            "[42]"
        );
        assert_eq!(
            c.hosts[&account].snapshot.as_ref().unwrap().core.remaining,
            4997 - estimated_cost(&args)
        );
        c.finish(first, &args, response(4998, false)).unwrap();
        assert_eq!(
            c.hosts[&account].snapshot.as_ref().unwrap().core.remaining,
            4997,
            "late headers must not put spent quota back"
        );
        c.reads.get_mut(&(account.clone(), 1)).unwrap().expires = Instant::now();
        let conditional = admit(&mut c, 1);
        assert_eq!(conditional.validator.as_deref(), Some("\"fixture\""));
        assert_eq!(
            c.finish(conditional, &args, response(4997, true)).unwrap(),
            "[42]"
        );
    }

    #[test]
    fn slow_read_does_not_hold_up_an_unrelated_repository() {
        for (index, same_host) in [false, true].into_iter().enumerate() {
            let host = format!("concurrency-{index}.example.test");
            let neighbor = if same_host {
                host.clone()
            } else {
                format!("neighbor-{index}.example.test")
            };
            let (started_tx, started_rx) = std::sync::mpsc::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            std::thread::scope(|threads| {
                threads.spawn(move || {
                    run_read_with(
                        &host,
                        Some(1),
                        Path::new("/fixture"),
                        &["api", "repos/fixture/slow", "--hostname", &host],
                        Instant::now() + Duration::from_secs(2),
                        |args, _| {
                            if args.get(1) == Some(&"rate_limit") {
                                return Executed::plain(Ok(quota(4999, 4999)));
                            }
                            started_tx.send(()).unwrap();
                            release_rx.recv_timeout(Duration::from_secs(1)).unwrap();
                            Executed::plain(Ok("slow".into()))
                        },
                    )
                    .unwrap();
                });
                started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
                threads.spawn(move || {
                    let result = run_read_with(
                        &neighbor,
                        Some(1),
                        Path::new("/fixture"),
                        &["api", "repos/fixture/fast", "--hostname", &neighbor],
                        Instant::now() + Duration::from_secs(2),
                        |args, _| {
                            Executed::plain(Ok(if args.get(1) == Some(&"rate_limit") {
                                quota(4999, 4999)
                            } else {
                                "fast".into()
                            }))
                        },
                    );
                    done_tx.send(result).unwrap();
                });
                let completed = done_rx.recv_timeout(Duration::from_millis(200));
                release_tx.send(()).unwrap();
                assert_eq!(
                    completed.ok(),
                    Some(Ok("fast".into())),
                    "an unrelated read waited behind a slow request (same host: {same_host})"
                );
            });
        }
    }

    fn quota(core: u64, graphql: u64) -> String {
        format!(
            r#"{{"resources":{{"core":{{"limit":5000,"remaining":{core},"reset":{reset}}},"graphql":{{"limit":5000,"remaining":{graphql},"reset":{reset}}}}}}}"#,
            reset = epoch() + 3600
        )
    }
    #[test]
    fn cold_probe_cache_and_independent_buckets() {
        let mut c = Coordinator::default();
        let mut calls = Vec::new();
        let mut exec = |args: &[&str]| {
            calls.push(args.join(" "));
            Ok(if args.get(1) == Some(&"rate_limit") {
                quota(4000, 501)
            } else {
                "[]".into()
            })
        };
        assert!(c
            .read(
                "github.com",
                None,
                Path::new("/repo"),
                &["pr", "list"],
                &mut exec
            )
            .is_err());
        assert_eq!(
            c.read(
                "github.com",
                None,
                Path::new("/repo"),
                &["api", "repos/a/b"],
                &mut exec
            )
            .unwrap(),
            "[]"
        );
        assert_eq!(
            c.read(
                "github.com",
                None,
                Path::new("/repo"),
                &["api", "repos/a/b"],
                &mut exec
            )
            .unwrap(),
            "[]"
        );
        assert_eq!(calls.len(), 2);
    }
    #[test]
    fn shared_refusal_stops_sibling_and_mutation_success_does_not_lift_it() {
        let mut c = Coordinator::default();
        assert!(c
            .read(
                "github.com",
                Some(1),
                Path::new("/a"),
                &["pr", "list"],
                |a| if a.get(1) == Some(&"rate_limit") {
                    Ok(quota(4000, 4000))
                } else {
                    Err("secondary rate limit, Retry-After: 120".into())
                }
            )
            .is_err());
        c.mutation_result("github.com", Some(1), &Ok(String::new()));
        assert!(c
            .read(
                "github.com",
                Some(1),
                Path::new("/b"),
                &["issue", "view"],
                |_| panic!("must not spawn")
            )
            .is_err());
        assert!(c
            .read(
                "github.com",
                Some(2),
                Path::new("/b"),
                &["issue", "view"],
                |a| Ok(if a.get(1) == Some(&"rate_limit") {
                    quota(4000, 4000)
                } else {
                    "ok".into()
                })
            )
            .is_ok());
    }
    #[test]
    fn failures_cached_and_writes_invalidate_success() {
        let mut c = Coordinator::default();
        assert!(c
            .read("github.com", None, Path::new("/a"), &["pr", "view"], |a| {
                if a.get(1) == Some(&"rate_limit") {
                    Ok(quota(4000, 4000))
                } else {
                    Err("network unavailable".into())
                }
            })
            .is_err());
        assert!(c
            .read(
                "github.com",
                None,
                Path::new("/a"),
                &["pr", "view"],
                |_| panic!("cached failure")
            )
            .is_err());
        c.mutation_result("github.com", None, &Ok(String::new()));
        assert!(c
            .read(
                "github.com",
                None,
                Path::new("/a"),
                &["pr", "view"],
                |_| Ok("fresh".into())
            )
            .is_ok());
    }
    #[test]
    fn checks_only_accept_real_results_on_nonzero_exit() {
        let output = |stdout: &str, stderr: &str| TimedOutput {
            success: false,
            stdout: stdout.into(),
            stderr: stderr.into(),
        };
        assert!(output_result(&["pr", "checks"], output("", "API rate limit exceeded")).is_err());
        assert!(output_result(&["pr", "checks"], output("[]", "authentication failed")).is_err());
        assert!(output_result(
            &["pr", "checks"],
            output(r#"[{"name":"build","bucket":"fail"}]"#, "")
        )
        .is_ok());
        assert!(output_result(
            &["pr", "checks"],
            output(
                r#"[{"name":"build","bucket":"fail"}]"#,
                "API rate limit exceeded"
            )
        )
        .is_err());
    }
    #[test]
    fn read_classification_does_not_cache_writes() {
        assert!(is_read(
            &["api", "graphql", "-f", "query=query { viewer { login } }"],
            None
        ));
        assert!(!is_read(
            &["api", "graphql", "-f", "query=mutation { change }"],
            None
        ));
        assert!(!is_read(&["api", "repos/a/b/comments", "-X", "POST"], None));
        assert!(!is_read(&["pr", "edit", "1", "--body", "secret"], None));
        assert!(!is_read(&["api", "repos/a/b/comments", "-XPOST"], None));
        assert!(!is_read(
            &["api", "repos/a/b/comments", "-fbody=private"],
            None
        ));
        assert!(is_read(&["api", "repos/a/b", "--method=GET"], None));
        assert!(is_read(
            &["api", "graphql", "-fquery=query { viewer { login } }"],
            None
        ));
        assert!(
            estimated_cost(&["pr", "list", "--limit", "1000", "--json", "latestReviews"])
                > estimated_cost(&["pr", "list", "--limit", "100", "--json", "latestReviews"])
        );
    }

    #[test]
    fn sibling_repository_reads_share_a_key_but_implicit_branches_do_not() {
        assert_eq!(
            read_key(
                Path::new("/a"),
                &[
                    "pr",
                    "list",
                    "--head",
                    "feature",
                    "--repo",
                    "github.com/a/b"
                ]
            ),
            read_key(
                Path::new("/b"),
                &[
                    "pr",
                    "list",
                    "--head",
                    "feature",
                    "--repo",
                    "github.com/a/b"
                ]
            ),
        );
        assert_ne!(
            read_key(
                Path::new("/a"),
                &[
                    "pr",
                    "list",
                    "--head",
                    "feature",
                    "--repo",
                    "github.com/a/b"
                ]
            ),
            read_key(
                Path::new("/a"),
                &["pr", "list", "--head", "other", "--repo", "github.com/a/b"]
            ),
        );
        // If no branch can be resolved, never reuse another checkout's result.
        assert_ne!(
            read_key(
                Path::new("/nonexistent-a"),
                &["pr", "checks", "--json", "name", "--repo", "github.com/a/b"]
            ),
            read_key(
                Path::new("/nonexistent-b"),
                &["pr", "checks", "--json", "name", "--repo", "github.com/a/b"]
            ),
        );
    }

    #[test]
    fn invalid_quota_fails_closed_and_caches_the_probe_failure() {
        let mut c = Coordinator::default();
        let mut calls = 0;
        assert!(c
            .read("github.com", None, Path::new("/a"), &["pr", "list"], |_| {
                calls += 1;
                Ok("{}".into())
            })
            .is_err());
        assert!(c
            .read(
                "github.com",
                None,
                Path::new("/b"),
                &["pr", "checks"],
                |_| panic!("probe failure must be shared")
            )
            .is_err());
        assert_eq!(calls, 1);
    }

    #[test]
    fn cached_failures_back_off_and_cache_size_stays_bounded() {
        let mut c = Coordinator::default();
        let args = ["api", "repos/a/b"];
        let key = (
            ("github.com".into(), None),
            read_key(Path::new("/a"), &args),
        );
        for streak in 1..=6 {
            assert!(c
                .read("github.com", None, Path::new("/a"), &args, |a| {
                    if a.get(1) == Some(&"rate_limit") {
                        Ok(quota(4999, 4999))
                    } else {
                        Err("offline".into())
                    }
                })
                .is_err());
            let entry = c.reads.get_mut(&key).unwrap();
            assert_eq!(entry.failures, streak);
            let minimum = (60_u64 << (streak - 1).min(4)).min(900);
            assert!(
                entry
                    .expires
                    .saturating_duration_since(Instant::now())
                    .as_secs()
                    >= minimum - 1
            );
            entry.expires = Instant::now() - Duration::from_secs(1);
        }
        for n in 0..300 {
            let endpoint = format!("repos/a/{n}");
            c.read(
                "github.com",
                None,
                Path::new("/a"),
                &["api", &endpoint],
                |a| {
                    Ok(if a.get(1) == Some(&"rate_limit") {
                        quota(4999, 4999)
                    } else {
                        "ok".into()
                    })
                },
            )
            .unwrap();
        }
        assert!(c.reads.len() <= MAX_CACHE_ENTRIES);
    }

    #[test]
    fn hosts_and_retry_headers_are_normalized_without_remote_credentials() {
        assert_eq!(
            remote_repository("https://user:secret@github.example:8443/a/b.git"),
            Some("github.example:8443/a/b".into())
        );
        assert_eq!(
            remote_repository("git@github.example:a/b.git"),
            Some("github.example/a/b".into())
        );
        assert_eq!(retry_epoch("Retry-After: 120", 1000), Some(1120));
        assert_eq!(retry_epoch("x-ratelimit-reset: 2000", 1000), Some(2000));
        assert_eq!(retry_epoch("Retry after 2 seconds", 1000), Some(1060));
        assert_eq!(retry_epoch("x-ratelimit-reset: 999", 1000), None);
    }

    #[test]
    fn short_repository_selectors_use_the_default_host() {
        let expected = std::env::var("GH_HOST")
            .ok()
            .filter(|host| !host.trim().is_empty())
            .unwrap_or_else(|| "github.com".into())
            .to_ascii_lowercase();
        for selector in ["--repo", "-R"] {
            let (host, pinned) = context(
                Path::new("/nonexistent"),
                &["pr", "list", selector, "owner/repo"],
            )
            .unwrap();
            assert_eq!(host, expected);
            assert_eq!(pinned[3], format!("{expected}/owner/repo"));
        }
        let (host, pinned) = context(
            Path::new("/nonexistent"),
            &["pr", "list", "--repo", "github.example/owner/repo"],
        )
        .unwrap();
        assert_eq!(host, "github.example");
        assert_eq!(pinned[3], "github.example/owner/repo");
    }

    #[test]
    fn expensive_graphql_reads_are_reserved_before_spawning_and_metered_afterwards() {
        let mut c = Coordinator::default();
        let args = ["api", "graphql", "-f", "query=query { repository { reviewThreads(first:50) { comments(first:100) { nodes { id } } } } }"];
        let mut calls = 0;
        assert!(c
            .read("low.example", None, Path::new("/a"), &args, |_| {
                calls += 1;
                Ok(quota(4999, 699))
            })
            .is_err());
        assert_eq!(
            calls, 1,
            "only the quota probe can run when a nested query would consume the reserve"
        );

        let reset = chrono::DateTime::from_timestamp((epoch() + 3600) as i64, 0)
            .unwrap()
            .to_rfc3339();
        let mut exec = |args: &[&str]| {
            if args.get(1) == Some(&"rate_limit") {
                return Ok(quota(4999, 800));
            }
            assert!(graphql_document(args)
                .unwrap()
                .contains("rateLimit { cost limit remaining resetAt }"));
            Ok(serde_json::json!({"data":{"rateLimit":{"cost":10,"limit":5000,"remaining":790,"resetAt":reset}}}).to_string())
        };
        c.read("healthy.example", None, Path::new("/a"), &args, &mut exec)
            .unwrap();
        // A different key would be blocked if the upper admission estimate
        // replaced the actual remaining quota indefinitely.
        c.read("healthy.example", None, Path::new("/b"), &args, &mut exec)
            .unwrap();
    }

    #[test]
    fn pagination_follows_the_outer_connection_and_rejects_missing_state() {
        let value = serde_json::json!({"data":{"repository":{"reviewThreads":{
            "pageInfo":{"hasNextPage":false},
            "nodes":[{"comments":{"pageInfo":{"hasNextPage":true,"endCursor":"inner"}}}]
        }}}});
        assert_eq!(outer_page_info(&value).unwrap()["hasNextPage"], false);
        assert!(outer_page_info(&serde_json::json!({"data":{"error":"unavailable"}})).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn stdin_runner_drains_large_bodies_and_kills_its_timed_out_child() {
        let mut echo = std::process::Command::new("/bin/sh");
        echo.args(["-c", "cat"]);
        let body = "x".repeat(512 * 1024);
        let output = run_with_stdin(echo, &body, Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("echo timed out"));
        assert!(output.success);
        assert_eq!(output.stdout, body);

        let directory = tempfile::tempdir().unwrap();
        let pid_file = directory.path().join("pid");
        let mut sleep = std::process::Command::new("/bin/sh");
        sleep.args(["-c", "echo $$ > \"$1\"; exec sleep 30", "fake-gh"]);
        sleep.arg(&pid_file);
        assert!(matches!(
            run_with_stdin(sleep, "", Duration::from_millis(100)),
            Err(TimedFailure::Timeout)
        ));
        let pid: i32 = std::fs::read_to_string(pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // Our runner owns this PID and has reaped it before returning.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }
}
