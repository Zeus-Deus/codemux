//! One process-wide budget for every GitHub read, including background jobs.
//!
//! The lock deliberately spans the subprocess: reads cannot stampede a cold
//! quota probe, duplicate a cache miss, or keep launching after a refusal.
//! Mutations remain available and invalidate reads without lifting a pause.
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::git_provider::exec::{TimedFailure, TimedOutput};

const PROBE_TTL: Duration = Duration::from_secs(60);
const MAX_CACHE_ENTRIES: usize = 256;
const MAX_CACHE_BYTES: usize = 16 * 1024 * 1024;
const MAX_ENTRY_BYTES: usize = 2 * 1024 * 1024;
const MAX_HOSTS: usize = 128;

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
    probe_error: Option<(Instant, String)>,
    probe_failures: u32,
    core_pause_until: u64,
    graphql_pause_until: u64,
}

struct Entry {
    expires: Instant,
    result: Result<String, String>,
    failures: u32,
    scope: u64,
}

#[derive(Default)]
struct Coordinator {
    hosts: HashMap<(String, Option<u64>), HostState>,
    reads: HashMap<((String, Option<u64>), u64), Entry>,
}

static COORDINATOR: OnceLock<Mutex<Coordinator>> = OnceLock::new();

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
    state.refusals = state.refusals.saturating_add(1);
    let delay = (60_u64.saturating_mul(1_u64 << state.refusals.saturating_sub(1).min(4))).min(900);
    let until = retry_epoch(message, now).unwrap_or(now.saturating_add(delay));
    state.pause_until = state.pause_until.max(until);
    // A refusal might be a primary exhaustion by another program. Re-probe
    // after the pause instead of reusing our previously healthy snapshot.
    state.snapshot = None;
}

impl Coordinator {
    fn read(
        &mut self,
        host: &str,
        identity: Option<u64>,
        path: &Path,
        args: &[&str],
        mut execute: impl FnMut(&[&str]) -> Result<String, String>,
    ) -> Result<String, String> {
        let host_key = (host.to_string(), identity);
        if !self.hosts.contains_key(&host_key) && self.hosts.len() >= MAX_HOSTS {
            // Never discard a live cooldown to make room for another host.
            return Err("GitHub budget coordinator has too many active accounts".into());
        }
        let now = epoch();
        let state = self.hosts.entry(host_key.clone()).or_default();
        let diagnostic = args.first() == Some(&"api") && args.get(1) == Some(&"rate_limit");
        if state.pause_until > now {
            if diagnostic {
                if let Some(snapshot) = &state.snapshot {
                    return Ok(snapshot.raw.clone());
                }
            }
            return Err(quota_pause(host, state.pause_until));
        }
        let key = (host_key.clone(), read_key(path, args));
        if !diagnostic {
            if let Some(entry) = self.reads.get(&key) {
                if entry.expires > Instant::now() {
                    return entry.result.clone();
                }
            }
        }
        let state = self.hosts.get_mut(&host_key).unwrap();
        let selected = bucket(args);
        if diagnostic && (state.core_pause_until > now || state.graphql_pause_until > now) {
            if let Some(snapshot) = &state.snapshot {
                return Ok(snapshot.raw.clone());
            }
        }
        if !diagnostic {
            let primary_pause = match selected {
                Bucket::Core => state.core_pause_until,
                Bucket::Graphql => state.graphql_pause_until,
            }
            .max(if uses_core_too(args) {
                state.core_pause_until
            } else {
                0
            });
            if primary_pause > now {
                return Err(quota_pause(host, primary_pause));
            }
        }
        if let Some((at, message)) = &state.probe_error {
            let delay = Duration::from_secs(
                (60_u64 << state.probe_failures.saturating_sub(1).min(4)).min(900),
            );
            if at.elapsed() < delay {
                return Err(message.clone());
            }
        }
        let needs_probe = state.snapshot.as_ref().is_none_or(|s| {
            s.fetched.elapsed() >= PROBE_TTL
                || match selected {
                    Bucket::Core => s.core.reset,
                    Bucket::Graphql => s.graphql.reset,
                } <= now
        });
        if needs_probe {
            let probe =
                execute(&["api", "rate_limit", "--hostname", host]).and_then(parse_snapshot);
            match probe {
                Ok(snapshot) => {
                    state.snapshot = Some(snapshot);
                    state.probe_error = None;
                    state.probe_failures = 0;
                }
                Err(mut message) => {
                    if refusal(&message) {
                        record_refusal(state, &message, now);
                        message = quota_pause(host, state.pause_until);
                    }
                    state.probe_error = Some((Instant::now(), message.clone()));
                    state.probe_failures = state.probe_failures.saturating_add(1);
                    return Err(message);
                }
            }
        }
        if diagnostic {
            return Ok(state.snapshot.as_ref().unwrap().raw.clone());
        }
        let snapshot = state.snapshot.as_mut().unwrap();
        let quota = match selected {
            Bucket::Core => &mut snapshot.core,
            Bucket::Graphql => &mut snapshot.graphql,
        };
        let cost = estimated_cost(args);
        if quota.remaining.saturating_sub(cost) < quota.limit.div_ceil(10) {
            let reset = quota.reset;
            match selected {
                Bucket::Core => state.core_pause_until = reset,
                Bucket::Graphql => state.graphql_pause_until = reset,
            };
            return Err(quota_pause(host, reset));
        }
        quota.remaining = quota.remaining.saturating_sub(cost);
        if uses_core_too(args) {
            let core = &mut snapshot.core;
            if core.remaining.saturating_sub(2) < core.limit.div_ceil(10) {
                state.core_pause_until = core.reset;
                return Err(quota_pause(host, core.reset));
            }
            core.remaining = core.remaining.saturating_sub(2);
        }
        let mut measured: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        if args.first() == Some(&"api") && args.get(1) == Some(&"graphql") {
            if let Some(query) = graphql_document(args) {
                if !query.contains("rateLimit") {
                    if let Some(end) = query.rfind('}') {
                        let metered = format!(
                            "{} rateLimit {{ cost limit remaining resetAt }} {}",
                            &query[..end],
                            &query[end..]
                        );
                        if let Some(index) = args.iter().position(|a| a.contains("query=")) {
                            let prefix = &args[index][..args[index].find("query=").unwrap()];
                            measured[index] = format!("{prefix}query={metered}");
                        }
                    }
                }
            }
        }
        let measured: Vec<&str> = measured.iter().map(String::as_str).collect();
        let mut result = execute(&measured);
        let state = self.hosts.get_mut(&host_key).unwrap();
        if let Err(message) = &result {
            if refusal(message) {
                record_refusal(state, message, epoch());
                result = Err(quota_pause(host, state.pause_until));
            }
        } else if state.pause_until <= now {
            state.refusals = 0;
        }
        if let (Ok(raw), Some(snapshot)) = (&result, state.snapshot.as_mut()) {
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) {
                let live = &value["data"]["rateLimit"];
                if let (Some(limit), Some(remaining), Some(reset), Some(_cost)) = (
                    live["limit"].as_u64().filter(|n| *n > 0),
                    live["remaining"].as_u64(),
                    live["resetAt"]
                        .as_str()
                        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                        .and_then(|d| u64::try_from(d.timestamp()).ok()),
                    live["cost"].as_u64(),
                ) {
                    if remaining <= limit {
                        snapshot.graphql = Quota {
                            limit,
                            remaining,
                            reset,
                        };
                        if let Ok(mut raw) =
                            serde_json::from_str::<serde_json::Value>(&snapshot.raw)
                        {
                            raw["resources"]["graphql"] = serde_json::json!({"limit": limit, "remaining": remaining, "reset": reset});
                            snapshot.raw = raw.to_string();
                        }
                    }
                } else if args.get(1) == Some(&"list") {
                    if let Some(rows) = value.as_array() {
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
                        let actual_rows = u64::try_from(rows.len()).unwrap_or(u64::MAX);
                        let used = actual_rows
                            .max(1)
                            .div_ceil(100)
                            .saturating_mul(4)
                            .saturating_add(actual_rows.saturating_mul(nested.max(1)));
                        let q = match selected {
                            Bucket::Core => &mut snapshot.core,
                            Bucket::Graphql => &mut snapshot.graphql,
                        };
                        q.remaining = q
                            .remaining
                            .saturating_add(cost.saturating_sub(used))
                            .min(q.limit);
                    }
                }
            }
        }
        let failures = if result.is_err() {
            self.reads
                .get(&key)
                .map_or(1, |e| e.failures.saturating_add(1))
        } else {
            0
        };
        let expiry = if failures > 0 {
            Duration::from_secs((60_u64 << failures.saturating_sub(1).min(4)).min(900))
        } else {
            ttl(args)
        };
        let size = result.as_ref().map_or_else(|e| e.len(), |s| s.len());
        self.reads
            .retain(|_, e| e.expires > Instant::now() || e.failures > 0);
        let bytes: usize = self
            .reads
            .values()
            .map(|e| e.result.as_ref().map_or_else(|s| s.len(), |s| s.len()))
            .sum();
        if size <= MAX_ENTRY_BYTES {
            if self.reads.len() >= MAX_CACHE_ENTRIES || bytes + size > MAX_CACHE_BYTES {
                if let Some(oldest) = self
                    .reads
                    .iter()
                    .min_by_key(|(_, e)| e.expires)
                    .map(|(k, _)| k.clone())
                {
                    self.reads.remove(&oldest);
                }
            }
            if bytes + size <= MAX_CACHE_BYTES && self.reads.len() < MAX_CACHE_ENTRIES {
                self.reads.insert(
                    key,
                    Entry {
                        expires: Instant::now() + expiry,
                        result: result.clone(),
                        failures,
                        scope: scope_key(path, args),
                    },
                );
            }
        }
        result
    }

    fn mutation_result(
        &mut self,
        host: &str,
        identity: Option<u64>,
        result: &Result<String, String>,
    ) {
        let key = (host.to_string(), identity);
        if result.is_ok() {
            self.reads.retain(|(h, _), _| h != &key);
        } else if let Err(message) = result {
            if refusal(message) {
                if self.hosts.contains_key(&key) || self.hosts.len() < MAX_HOSTS {
                    record_refusal(self.hosts.entry(key).or_default(), message, epoch());
                }
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

pub fn run(path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    run_inner(path, args, timeout, None)
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
        .reads
        .retain(|(key, _), entry| {
            key != &host_key || entry.scope != scope || entry.result.is_err()
        });
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
    let deadline = Instant::now() + timeout;
    if stdin.is_none() && is_read(args, None) && args.contains(&"--paginate") {
        return run_paginated(path, args, timeout);
    }
    let (host, pinned) = context(path, args)?;
    let identity = crate::github::gh_credential_identity(&host);
    let pinned: Vec<&str> = pinned.iter().map(String::as_str).collect();
    if is_read(args, stdin) {
        let path: PathBuf = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        lock_until(deadline)?.read(&host, identity, &path, &pinned, |a| {
            execute(&path, a, time_left(deadline)?, None)
        })
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
fn run_paginated(path: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
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
        let raw = run_inner(path, &request, time_left(deadline)?, None)?;
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
