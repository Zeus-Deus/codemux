//! Shared helpers for running commands on a host over the system `ssh`.
//!
//! Older call sites (probe, bootstrap, push, inventory) each build their own
//! `ssh` argv. New code that runs work on a device — host-side project and
//! worktree setup, agent-chat providers spawned over `ssh -T` — goes through
//! these helpers so flags, `--` placement and shell quoting stay consistent.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde::de::DeserializeOwned;
use tokio::process::Command;
use tokio::time::timeout;

use crate::json_rpc_child::SpawnConfig;

/// Where Codemux installs its helper binaries on a host.
pub const REMOTE_BIN_DIR: &str = "~/.local/bin";

/// PATH entries prepended for commands run on a host. Non-interactive SSH
/// shells usually miss the per-user install locations agent CLIs use
/// (`claude`'s native installer, npm/bun/cargo globals, Homebrew).
const REMOTE_PATH_PREFIX: &str = "$HOME/.local/bin:$HOME/.claude/local:$HOME/.npm-global/bin:$HOME/.bun/bin:$HOME/.cargo/bin:/usr/local/bin:/opt/homebrew/bin";

/// Reject SSH targets that `ssh` would parse as an option or that cannot be a
/// single destination argument. Targets also arrive from account sync, so a
/// value like `-oProxyCommand=…` must never reach an argv.
pub fn validate_ssh_target(target: &str) -> Result<(), String> {
    if target.is_empty() {
        return Err("SSH target is empty".into());
    }
    if target.starts_with('-') {
        return Err("SSH target can't start with '-'".into());
    }
    if target.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("SSH target can't contain spaces or control characters".into());
    }
    Ok(())
}

/// Single-quote a value for a remote shell.
///
/// `'` and `\` are written outside the quotes (`\'`, `\\`), so no backslash
/// ever sits inside a quoted run. POSIX shells and fish then read the result
/// the same way — fish treats `\'` and `\\` as escapes even inside single
/// quotes — which keeps nested quoting intact whatever the device's login
/// shell is.
pub fn sh_quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for c in value.chars() {
        match c {
            '\'' => out.push_str(r"'\''"),
            '\\' => out.push_str(r"'\\'"),
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// Quote a host path, keeping a leading `~` expandable as `"$HOME"`.
pub fn sh_path(path: &str) -> String {
    if path == "~" {
        return "\"$HOME\"".into();
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("\"$HOME\"/{}", sh_quote(rest)),
        None => sh_quote(path),
    }
}

/// Resolve a leading `~` in a host path against that host's `$HOME`.
pub fn expand_remote_tilde(path: &str, home: &str) -> String {
    let home = home.trim_end_matches('/');
    if path == "~" {
        return home.to_string();
    }
    match path.strip_prefix("~/") {
        Some(rest) => format!("{home}/{rest}"),
        None => path.to_string(),
    }
}

fn is_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    matches!(chars.next(), Some(c) if c == '_' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}

/// Flags shared by every one-shot `ssh` call. `BatchMode` makes a missing
/// key fail fast instead of hanging on a password prompt.
pub fn one_shot_args(connect_timeout_secs: u64) -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={connect_timeout_secs}"),
        "-o".into(),
        "StrictHostKeyChecking=accept-new".into(),
        "-o".into(),
        "LogLevel=ERROR".into(),
    ]
}

/// Build the remote command string that runs `program args` in `cwd` with
/// `env` overlaid.
///
/// The work runs under `sh -l` so `~/.profile` can extend PATH and the
/// script stays valid when the user's login shell is fish or another
/// non-POSIX shell. `exec` lets the program replace the shell, so closing
/// ssh's stdin reaches it as EOF.
pub fn remote_exec_script(
    program: &str,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&str>,
) -> Result<String, String> {
    let mut inner = String::new();
    if let Some(cwd) = cwd {
        inner.push_str(&format!("cd -- {} && ", sh_path(cwd)));
    }
    inner.push_str(&format!("exec env \"PATH={REMOTE_PATH_PREFIX}:$PATH\""));
    let mut keys: Vec<&String> = env.keys().collect();
    keys.sort();
    for key in keys {
        if !is_env_key(key) {
            return Err(format!("invalid environment variable name {key:?}"));
        }
        inner.push(' ');
        inner.push_str(&sh_quote(&format!("{key}={}", env[key])));
    }
    inner.push(' ');
    inner.push_str(&sh_path(program));
    for arg in args {
        inner.push(' ');
        inner.push_str(&sh_quote(arg));
    }
    Ok(format!("exec sh -lc {}", sh_quote(&inner)))
}

/// Turn a provider spawn into an `ssh -T` spawn that runs it on a host. The
/// JSON-RPC stdio stream flows over the SSH channel unchanged.
pub fn stdio_spawn_config(
    ssh_target: &str,
    program: &str,
    args: &[String],
    env: &HashMap<String, String>,
    cwd: Option<&str>,
    default_timeout: Duration,
) -> Result<SpawnConfig, String> {
    validate_ssh_target(ssh_target)?;
    let script = remote_exec_script(program, args, env, cwd)?;
    let mut ssh_args: Vec<String> = vec!["-T".into(), "-e".into(), "none".into()];
    ssh_args.extend(one_shot_args(15));
    ssh_args.extend([
        "-o".into(),
        "ServerAliveInterval=15".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "--".into(),
        ssh_target.into(),
        script,
    ]);
    Ok(SpawnConfig {
        program: PathBuf::from("ssh"),
        args: ssh_args,
        // Nothing to overlay on the local ssh client; the workspace env is
        // part of the remote script.
        env: HashMap::new(),
        // Never the host path: it does not exist locally.
        cwd: None,
        default_timeout,
    })
}

/// Run the POSIX `script` on the host and return stdout. Failures carry
/// ssh's stderr. The script runs under `sh -c` so it works whatever the
/// user's login shell is (fish, zsh, …).
pub async fn run_remote(
    ssh_target: &str,
    script: &str,
    budget: Duration,
) -> Result<String, String> {
    validate_ssh_target(ssh_target)?;
    let connect_timeout = budget.as_secs().clamp(1, 15);
    let mut cmd = Command::new("ssh");
    cmd.args(one_shot_args(connect_timeout))
        .arg("--")
        .arg(ssh_target)
        .arg(format!("exec sh -c {}", sh_quote(script)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::execution::sanitize_appimage_env_tokio(&mut cmd);
    let output = timeout(budget, cmd.output())
        .await
        .map_err(|_| format!("timed out after {}s", budget.as_secs()))?
        .map_err(|e| format!("couldn't run ssh: {e}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(match (output.status.code(), stderr.is_empty()) {
        (Some(255), true) => format!("couldn't connect to {ssh_target} over SSH"),
        (_, false) => stderr,
        (Some(code), true) => format!("remote command exited with status {code}"),
        (None, true) => "remote command was terminated".into(),
    })
}

/// Run `script` and parse the last JSON object line it prints. Login shells
/// may print banners before the payload, so earlier lines are ignored.
pub async fn run_remote_json<T: DeserializeOwned>(
    ssh_target: &str,
    script: &str,
    budget: Duration,
) -> Result<T, String> {
    let stdout = run_remote(ssh_target, script, budget).await?;
    parse_last_json_line(&stdout)
}

pub(crate) fn parse_last_json_line<T: DeserializeOwned>(stdout: &str) -> Result<T, String> {
    for line in stdout.lines().rev() {
        let line = line.trim();
        if line.starts_with('{') {
            if let Ok(value) = serde_json::from_str(line) {
                return Ok(value);
            }
        }
    }
    let preview: String = stdout.trim().chars().take(200).collect();
    Err(format!("unexpected output from host: {preview}"))
}

fn home_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The host's `$HOME`, cached per SSH target for the life of the process.
pub async fn remote_home(ssh_target: &str) -> Result<String, String> {
    if let Some(home) = home_cache()
        .lock()
        .ok()
        .and_then(|c| c.get(ssh_target).cloned())
    {
        return Ok(home);
    }
    let out = run_remote(
        ssh_target,
        "printf '%s\\n' \"$HOME\"",
        Duration::from_secs(20),
    )
    .await?;
    let home = out
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('/'))
        .ok_or_else(|| "couldn't resolve the host's home directory".to_string())?
        .to_string();
    if let Ok(mut cache) = home_cache().lock() {
        cache.insert(ssh_target.to_string(), home.clone());
    }
    Ok(home)
}

/// What [`env_var_set_script`] prints when the variable is set.
const ENV_VAR_SET_MARKER: &str = "CODEMUX_ENV_VAR_SET";

/// Print [`ENV_VAR_SET_MARKER`] when `name` is non-empty in the `sh -l`
/// environment [`remote_exec_script`] starts providers in. The value itself
/// is never printed.
fn env_var_set_script(name: &str) -> Result<String, String> {
    if !is_env_key(name) {
        return Err(format!("invalid environment variable name {name:?}"));
    }
    let inner = format!("if [ -n \"${{{name}:-}}\" ]; then echo {ENV_VAR_SET_MARKER}; fi");
    Ok(format!("exec sh -lc {}", sh_quote(&inner)))
}

/// Whether `name` is set to a non-empty value in the environment a provider
/// started on the host gets. Only the answer crosses the wire, never the
/// value; login shells may print banners, so the marker is matched by line.
pub async fn remote_env_var_set(ssh_target: &str, name: &str) -> Result<bool, String> {
    let script = env_var_set_script(name)?;
    let stdout = run_remote(ssh_target, &script, Duration::from_secs(15)).await?;
    Ok(stdout.lines().any(|line| line.trim() == ENV_VAR_SET_MARKER))
}

type ProgramCache = Mutex<HashMap<(String, String), Option<String>>>;

fn program_cache() -> &'static ProgramCache {
    static CACHE: OnceLock<ProgramCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A bare command name that is safe to splice into a remote script.
fn is_plain_program_name(program: &str) -> bool {
    !program.is_empty()
        && !program.starts_with('-')
        && program
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Ask the user's own shell on the host where `program` lives. It runs as a
/// login and interactive shell so rc files run: nvm, mise and asdf usually
/// extend PATH only in `~/.bashrc` or `~/.zshrc`, which the `sh -l` provider
/// launch never reads.
fn resolve_program_script(program: &str) -> String {
    format!(
        "exec \"${{SHELL:-/bin/sh}}\" -lic {}",
        sh_quote(&format!("command -v {program}"))
    )
}

/// The last absolute path the shell printed. rc files may print banners, and
/// `command -v` names aliases and functions without a `/`.
fn last_absolute_path(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('/'))
        .map(str::to_string)
}

/// The absolute path of `program` on the host as the user's shell resolves
/// it, or `None` when it can't. Cached per (target, program) for the life of
/// the process, misses included, so a slow or broken rc file costs one round
/// trip; [`forget_remote_programs`] clears a target after a failed start.
pub async fn resolve_remote_program(ssh_target: &str, program: &str) -> Option<String> {
    if !is_plain_program_name(program) {
        return None;
    }
    let key = (ssh_target.to_string(), program.to_string());
    if let Some(cached) = program_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(&key).cloned())
    {
        return cached;
    }
    let resolved = run_remote(
        ssh_target,
        &resolve_program_script(program),
        Duration::from_secs(15),
    )
    .await
    .ok()
    .and_then(|stdout| last_absolute_path(&stdout));
    if let Ok(mut cache) = program_cache().lock() {
        cache.insert(key, resolved.clone());
    }
    resolved
}

/// Drop the resolved programs for a host, so the next start asks its shell
/// again (the CLI was installed, moved, or the host was unreachable).
pub fn forget_remote_programs(ssh_target: &str) {
    if let Ok(mut cache) = program_cache().lock() {
        cache.retain(|(target, _), _| target != ssh_target);
    }
}

/// Seed a lookup result, so tests elsewhere never reach a real host.
#[cfg(test)]
pub(crate) fn cache_remote_program(ssh_target: &str, program: &str, path: Option<&str>) {
    program_cache().lock().unwrap().insert(
        (ssh_target.to_string(), program.to_string()),
        path.map(str::to_string),
    );
}

#[cfg(test)]
pub(crate) fn is_remote_program_cached(ssh_target: &str, program: &str) -> bool {
    program_cache()
        .lock()
        .unwrap()
        .contains_key(&(ssh_target.to_string(), program.to_string()))
}

/// Invoke the installed `codemux-remote` with pre-quoted `args`. The copy in
/// `~/.local/bin` wins over PATH: it is the one bootstrap installs and the
/// version check inspects, so an older copy elsewhere on PATH can't shadow it.
pub fn codemux_remote_command(args: &str) -> String {
    format!(
        "if [ -x \"$HOME/.local/bin/codemux-remote\" ]; then CMR=\"$HOME/.local/bin/codemux-remote\"; \
         elif command -v codemux-remote >/dev/null 2>&1; then CMR=codemux-remote; \
         else echo 'codemux-remote is not installed on this device' >&2; exit 127; fi; \
         \"$CMR\" {args}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_option_like_targets() {
        assert!(validate_ssh_target("-oProxyCommand=evil").is_err());
        assert!(validate_ssh_target("user@host -p 22").is_err());
        assert!(validate_ssh_target("").is_err());
        assert!(validate_ssh_target("deus@zeus").is_ok());
        assert!(validate_ssh_target("homelab").is_ok());
    }

    #[test]
    fn quotes_paths_and_values() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        assert_eq!(sh_path("~/a b"), "\"$HOME\"/'a b'");
        assert_eq!(sh_path("~"), "\"$HOME\"");
        assert_eq!(sh_path("/srv/x"), "'/srv/x'");
        assert_eq!(expand_remote_tilde("~/w", "/home/u/"), "/home/u/w");
        assert_eq!(expand_remote_tilde("/abs", "/home/u"), "/abs");
    }

    #[test]
    fn env_var_set_script_reports_presence_without_the_value() {
        let script = env_var_set_script("CODEMUX_TEST_PROBE_KEY").unwrap();
        let run = |value: Option<&str>| {
            let mut cmd = std::process::Command::new("sh");
            cmd.arg("-c")
                .arg(&script)
                .env_remove("CODEMUX_TEST_PROBE_KEY");
            if let Some(value) = value {
                cmd.env("CODEMUX_TEST_PROBE_KEY", value);
            }
            String::from_utf8(cmd.output().unwrap().stdout).unwrap()
        };
        let set = run(Some("very-secret"));
        assert!(
            set.lines().any(|line| line.trim() == ENV_VAR_SET_MARKER),
            "{set}"
        );
        assert!(!set.contains("very-secret"));
        assert!(!run(Some("")).contains(ENV_VAR_SET_MARKER));
        assert!(!run(None).contains(ENV_VAR_SET_MARKER));
        assert!(env_var_set_script("BAD NAME; rm -rf ~").is_err());
    }

    #[test]
    fn exec_script_runs_program_in_cwd_with_env() {
        let env = HashMap::from([
            ("B".to_string(), "two words".to_string()),
            ("A".to_string(), "it's".to_string()),
        ]);
        let script = remote_exec_script(
            "~/.local/bin/sidecar",
            &["--flag".into()],
            &env,
            Some("/home/u/repo"),
        )
        .unwrap();
        assert!(script.starts_with("exec sh -lc '"));
        // The inner script is single-quoted once more, so inner quotes are escaped.
        assert!(script.contains("cd -- '\\''/home/u/repo'\\'' && exec env"));
        let a = script.find("A=it").unwrap();
        let b = script.find("B=two words").unwrap();
        assert!(a < b, "env keys are sorted for stable argv");
        assert!(script.contains("\"$HOME\"/'\\''.local/bin/sidecar'\\''"));
    }

    /// Split a command line the way sshd's login shell would, for the
    /// single-quote/backslash subset these helpers emit. `fish` also treats
    /// `\'` and `\\` as escapes inside single quotes.
    fn tokenize(line: &str, fish: bool) -> Vec<String> {
        let mut words = Vec::new();
        let mut word = String::new();
        let mut in_word = false;
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\'' => {
                    in_word = true;
                    while let Some(q) = chars.next() {
                        match q {
                            '\'' => break,
                            '\\' if fish && matches!(chars.peek(), Some('\'' | '\\')) => {
                                word.push(chars.next().unwrap());
                            }
                            q => word.push(q),
                        }
                    }
                }
                '\\' => {
                    in_word = true;
                    if let Some(n) = chars.next() {
                        word.push(n);
                    }
                }
                c if c.is_whitespace() => {
                    if in_word {
                        words.push(std::mem::take(&mut word));
                        in_word = false;
                    }
                }
                c => {
                    in_word = true;
                    word.push(c);
                }
            }
        }
        if in_word {
            words.push(word);
        }
        words
    }

    #[test]
    fn quoting_survives_posix_and_fish_login_shells() {
        let tricky = r"it's a \ path \' and 'more'";
        for fish in [false, true] {
            assert_eq!(tokenize(&sh_quote(tricky), fish), vec![tricky.to_string()]);
        }
        // Two layers: the device's login shell parses the outer quoting, then
        // `sh -lc` parses the inner script.
        let env = HashMap::from([("CODEMUX_WORKSPACE_NAME".to_string(), tricky.to_string())]);
        let script = remote_exec_script(
            "sh",
            &["-c".into(), "echo 'claude: command not found'".into()],
            &env,
            Some("/srv/o'neil"),
        )
        .unwrap();
        let posix = tokenize(&script, false);
        let fish = tokenize(&script, true);
        assert_eq!(posix, fish);
        assert_eq!(posix[..3], ["exec", "sh", "-lc"]);
        let inner = tokenize(&posix[3], false);
        assert!(inner.contains(&format!("CODEMUX_WORKSPACE_NAME={tricky}")));
        assert!(inner.contains(&"echo 'claude: command not found'".to_string()));
        assert!(inner.contains(&"/srv/o'neil".to_string()));
    }

    #[test]
    fn exec_script_rejects_bad_env_keys() {
        let env = HashMap::from([("BAD KEY".to_string(), "x".to_string())]);
        assert!(remote_exec_script("prog", &[], &env, None).is_err());
    }

    #[test]
    fn stdio_spawn_puts_target_after_double_dash() {
        let cfg = stdio_spawn_config(
            "deus@zeus",
            "codex",
            &["app-server".into()],
            &HashMap::new(),
            Some("/srv/repo"),
            Duration::from_secs(20),
        )
        .unwrap();
        assert_eq!(cfg.program, PathBuf::from("ssh"));
        assert!(cfg.cwd.is_none());
        let dd = cfg.args.iter().position(|a| a == "--").unwrap();
        assert_eq!(cfg.args[dd + 1], "deus@zeus");
        assert!(cfg.args[dd + 2].contains("app-server"));
        assert!(cfg.args.contains(&"BatchMode=yes".to_string()));
    }

    #[test]
    fn program_lookup_runs_in_the_users_interactive_login_shell() {
        let script = resolve_program_script("cursor-agent");
        assert_eq!(
            script,
            "exec \"${SHELL:-/bin/sh}\" -lic 'command -v cursor-agent'"
        );
        assert!(is_plain_program_name("codex"));
        assert!(!is_plain_program_name("codex; rm -rf ~"));
        assert!(!is_plain_program_name("-oops"));
        assert!(!is_plain_program_name(""));
        assert_eq!(
            last_absolute_path("Welcome!\n/home/u/.nvm/versions/node/v22/bin/codex\n"),
            Some("/home/u/.nvm/versions/node/v22/bin/codex".to_string())
        );
        assert_eq!(last_absolute_path("alias codex='npx codex'\n"), None);
    }

    #[tokio::test]
    async fn unsafe_program_names_are_never_sent_to_the_host() {
        assert_eq!(resolve_remote_program("deus@zeus", "a b").await, None);
    }

    #[tokio::test]
    async fn resolved_programs_are_cached_per_host_until_forgotten() {
        cache_remote_program("cache-a@host", "codex", Some("/opt/codex/bin/codex"));
        cache_remote_program("cache-b@host", "codex", None);
        // Cache hits answer without reaching either (unresolvable) host.
        assert_eq!(
            resolve_remote_program("cache-a@host", "codex")
                .await
                .as_deref(),
            Some("/opt/codex/bin/codex")
        );
        assert_eq!(resolve_remote_program("cache-b@host", "codex").await, None);
        forget_remote_programs("cache-a@host");
        assert!(!is_remote_program_cached("cache-a@host", "codex"));
        assert!(is_remote_program_cached("cache-b@host", "codex"));
    }

    #[test]
    fn parses_last_json_line_after_banner() {
        #[derive(serde::Deserialize)]
        struct P {
            path: String,
        }
        let p: P = parse_last_json_line("Welcome!\n{\"path\":\"/x\"}\n").unwrap();
        assert_eq!(p.path, "/x");
        assert!(parse_last_json_line::<P>("no json").is_err());
    }
}
