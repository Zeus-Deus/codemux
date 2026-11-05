//! Flags and values shared by the control verbs: who is calling, how to
//! print, how long to wait, and where a message or a workspace comes from.

use super::output::CliError;
use crate::agent_control::{ClientName, ControlAccess};
use std::io::Read;
use std::time::Duration;

/// The env defaults of `--client` and `--access`.
pub(crate) const CLIENT_ENV: &str = "CODEMUX_CLIENT";
pub(crate) const ACCESS_ENV: &str = "CODEMUX_ACCESS";

/// Who the control socket records this call as. A name attributes; an access
/// level narrows. Neither is a credential: every process of this OS user is
/// trusted alike.
///
/// The env defaults are not clap `env` attributes: clap would parse them for
/// every command line, so one blank or misspelled variable would stop the
/// app, `serve` and the internal children from starting. Only the commands
/// that call the control socket read them, through [`CallerArgs::or_env`].
#[derive(clap::Args, Clone, Debug, Default, PartialEq, Eq)]
#[command(next_help_heading = "Caller")]
pub struct CallerArgs {
    /// Name to record this client's calls under (`local:<NAME>`; default
    /// $CODEMUX_CLIENT). Without one, calls run as you.
    #[arg(long, value_name = "NAME", value_parser = client_name)]
    pub client: Option<String>,
    /// The most this client's calls may do (default $CODEMUX_ACCESS, else
    /// full-access).
    #[arg(long, value_enum, value_name = "LEVEL")]
    pub access: Option<AccessArg>,
}

impl CallerArgs {
    /// Fills what the flags left out from `CODEMUX_CLIENT` and
    /// `CODEMUX_ACCESS`. `var` stands in for the process env, so tests never
    /// read it. A blank variable counts as unset; one that is set but unusable
    /// is an error, never a silent owner or full access.
    pub(crate) fn or_env(self, var: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let set = |key: &str| var(key).filter(|value| !value.trim().is_empty());
        let client = match self.client {
            Some(client) => Some(client),
            None => set(CLIENT_ENV)
                .map(|raw| client_name(&raw).map_err(|why| format!("{CLIENT_ENV}={raw:?} is not a usable client name: {why}")))
                .transpose()?,
        };
        let access = match self.access {
            Some(access) => Some(access),
            None => set(ACCESS_ENV)
                .map(|raw| {
                    <AccessArg as clap::ValueEnum>::from_str(raw.trim(), false)
                        .map_err(|_| format!("{ACCESS_ENV}={raw:?} is not an access level: use read-only, supervised or full-access"))
                })
                .transpose()?,
        };
        Ok(Self { client, access })
    }
}

/// The env default of a root caller flag, by its id, for `capabilities`.
pub(crate) fn caller_env(id: &str) -> Option<&'static str> {
    match id {
        "client" => Some(CLIENT_ENV),
        "access" => Some(ACCESS_ENV),
        _ => None,
    }
}

/// `--access` values. The snake_case spellings the wire uses are accepted too.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessArg {
    #[value(name = "read-only", alias = "read_only")]
    ReadOnly,
    Supervised,
    #[value(name = "full-access", aliases = ["full_access", "full"])]
    FullAccess,
}

impl From<AccessArg> for ControlAccess {
    fn from(access: AccessArg) -> Self {
        match access {
            AccessArg::ReadOnly => ControlAccess::ReadOnly,
            AccessArg::Supervised => ControlAccess::Supervised,
            AccessArg::FullAccess => ControlAccess::FullAccess,
        }
    }
}

/// The CLI's own spelling of an access level, as `--access` takes it.
pub(crate) fn access_label(access: ControlAccess) -> &'static str {
    match access {
        ControlAccess::ReadOnly => "read-only",
        ControlAccess::Supervised => "supervised",
        ControlAccess::FullAccess => "full-access",
    }
}

/// A name with nothing usable in it would silently run as the owner, so it
/// is a usage error instead.
pub(crate) fn client_name(raw: &str) -> Result<String, String> {
    match ClientName::parse(raw) {
        Some(_) => Ok(raw.trim().to_owned()),
        None => Err("a client name needs at least one letter or digit".into()),
    }
}

#[derive(clap::Args, Clone, Debug, Default)]
pub struct OutputArgs {
    /// Print one JSON object on stdout instead of text, even on failure.
    #[arg(long)]
    pub json: bool,
}

/// Flags of the verbs that start or continue an agent's run.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct RunArgs {
    /// Idempotency key. Reuse it to retry this exact command safely
    /// (default: a new one).
    #[arg(long, value_name = "ID")]
    pub request_id: Option<String>,
    /// Wait for the agent's run to finish. Exits 1 if it fails, 2 if it
    /// needs input, 3 on --timeout.
    #[arg(long)]
    pub wait: bool,
    /// Stop waiting after this long, e.g. 90s, 10m, 1h. The work keeps going.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub timeout: Option<Duration>,
}

/// The longest `--timeout` the CLI takes.
const MAX_DURATION: Duration = Duration::from_secs(24 * 60 * 60);

/// `500ms`, `90` (seconds), `90s`, `10m` or `1h`, above zero and up to 24 h.
pub(crate) fn parse_duration(raw: &str) -> Result<Duration, String> {
    let raw = raw.trim();
    let split = raw.find(|c: char| !c.is_ascii_digit()).unwrap_or(raw.len());
    let (number, unit) = raw.split_at(split);
    let invalid = || format!("`{raw}` is not a duration like 500ms, 90s, 10m or 1h");
    let number: u64 = number.parse().map_err(|_| invalid())?;
    let duration = match unit {
        "ms" => Duration::from_millis(number),
        "" | "s" => Duration::from_secs(number),
        "m" => Duration::from_secs(number.saturating_mul(60)),
        "h" => Duration::from_secs(number.saturating_mul(3600)),
        _ => return Err(invalid()),
    };
    if duration.is_zero() || duration > MAX_DURATION {
        return Err(format!("`{raw}` must be above zero and at most 24h"));
    }
    Ok(duration)
}

/// A personal token's lifetime, `<N>d` or `<N>h`, in whole hours within the
/// connector's limits (1 hour to 365 days).
pub(crate) fn parse_expiry(raw: &str) -> Result<u32, String> {
    use crate::database::mcp_connector::{PERSONAL_TOKEN_MAX_TTL_MS, PERSONAL_TOKEN_MIN_TTL_MS};
    const HOUR_MS: i64 = 3_600_000;
    let raw = raw.trim();
    let invalid = || format!("`{raw}` is not a lifetime like 30d or 12h");
    let (number, hours_per_unit) = match raw.strip_suffix('d') {
        Some(days) => (days, 24),
        None => (raw.strip_suffix('h').ok_or_else(invalid)?, 1),
    };
    let number: u32 = number.parse().map_err(|_| invalid())?;
    let hours = number.checked_mul(hours_per_unit).ok_or_else(invalid)?;
    let (min, max) = (PERSONAL_TOKEN_MIN_TTL_MS / HOUR_MS, PERSONAL_TOKEN_MAX_TTL_MS / HOUR_MS);
    if !(min..=max).contains(&i64::from(hours)) {
        return Err(format!("`{raw}` must be between 1h and 365d"));
    }
    Ok(hours)
}

/// A value given inline, or `-` to read it from stdin.
pub(crate) fn inline_or_stdin(value: &str) -> Result<String, CliError> {
    if value != "-" {
        return Ok(value.to_owned());
    }
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .map_err(|error| CliError::failed("invalid_arguments", format!("Could not read stdin: {error}")))?;
    Ok(text)
}

/// A message for an agent. Trailing whitespace from a heredoc or pipe goes;
/// an empty message is refused here, before any call.
pub(crate) fn message(value: &str) -> Result<String, CliError> {
    let text = inline_or_stdin(value)?.trim_end().to_owned();
    if text.trim().is_empty() {
        return Err(CliError::failed("invalid_arguments", "The message is empty"));
    }
    Ok(text)
}

/// The idempotency key a mutation carries: the caller's, or a fresh one.
pub(crate) fn request_id(given: Option<&str>) -> String {
    given.map_or_else(|| format!("cli-{}", uuid::Uuid::new_v4()), str::to_owned)
}

/// `--workspace`, else `$CODEMUX_WORKSPACE_ID` (set in Codemux terminals).
pub(crate) fn workspace(flag: Option<&str>) -> Result<String, CliError> {
    resolve_workspace(flag, std::env::var("CODEMUX_WORKSPACE_ID").ok())
}

/// Pure core of [`workspace`]: `env` stands in for the process env.
pub(crate) fn resolve_workspace(flag: Option<&str>, env: Option<String>) -> Result<String, CliError> {
    flag.map(str::to_owned)
        .or(env)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| CliError::failed("invalid_arguments", "Pass --workspace <id> (see codemux workspace list)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_take_common_units_within_a_day() {
        assert_eq!(parse_duration("500ms"), Ok(Duration::from_millis(500)));
        assert_eq!(parse_duration("90"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("10m"), Ok(Duration::from_secs(600)));
        assert_eq!(parse_duration("1h"), Ok(Duration::from_secs(3600)));
        assert_eq!(parse_duration("24h"), Ok(MAX_DURATION));
        for bad in ["", "0s", "25h", "1d", "s", "-5s", "1.5s", "ten"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn token_lifetimes_stay_within_the_connector_limits() {
        assert_eq!(parse_expiry("30d"), Ok(720));
        assert_eq!(parse_expiry("1h"), Ok(1));
        assert_eq!(parse_expiry("365d"), Ok(8760));
        for bad in ["0h", "366d", "30", "1w", "d", "-1d", "99999999999d"] {
            assert!(parse_expiry(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_workspace_comes_from_the_flag_then_the_env() {
        let env = || Some("ws-env".to_string());
        assert_eq!(resolve_workspace(Some("ws-flag"), env()).unwrap(), "ws-flag");
        assert_eq!(resolve_workspace(None, env()).unwrap(), "ws-env");
        let missing = resolve_workspace(None, Some(" ".into())).unwrap_err();
        assert_eq!(missing.error.code, "invalid_arguments");
        assert!(resolve_workspace(None, None).is_err());
    }

    #[test]
    fn request_ids_are_kept_or_generated() {
        assert_eq!(request_id(Some("retry-1")), "retry-1");
        let generated = request_id(None);
        assert!(generated.starts_with("cli-") && generated.len() > 4 + 30, "{generated}");
        assert_ne!(generated, request_id(None));
    }

    #[test]
    fn empty_messages_are_refused_before_any_call() {
        assert_eq!(message("hi\n\n").unwrap(), "hi");
        assert_eq!(message("  keep leading").unwrap(), "  keep leading");
        assert_eq!(message(" \n\t").unwrap_err().error.code, "invalid_arguments");
    }

    #[test]
    fn caller_env_fills_only_what_the_flags_left_out() {
        let env = |pairs: &[(&str, &str)]| {
            let pairs: Vec<(String, String)> = pairs.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
            move |key: &str| pairs.iter().find(|(name, _)| name == key).map(|(_, value)| value.clone())
        };
        let none = CallerArgs::default();
        assert_eq!(none.clone().or_env(env(&[])), Ok(CallerArgs::default()));
        assert_eq!(
            none.clone().or_env(env(&[(CLIENT_ENV, "Hermes Bot"), (ACCESS_ENV, "supervised")])),
            Ok(CallerArgs { client: Some("Hermes Bot".into()), access: Some(AccessArg::Supervised) })
        );
        for (spelling, access) in [("read_only", AccessArg::ReadOnly), (" full ", AccessArg::FullAccess), ("full-access", AccessArg::FullAccess)] {
            assert_eq!(none.clone().or_env(env(&[(ACCESS_ENV, spelling)])).unwrap().access, Some(access), "{spelling}");
        }
        // A blank variable (an env file's `CODEMUX_CLIENT=`) is unset.
        assert_eq!(none.clone().or_env(env(&[(CLIENT_ENV, ""), (ACCESS_ENV, "  ")])), Ok(CallerArgs::default()));
        // Given flags win, so a bad variable they override does not matter.
        let flags = CallerArgs { client: Some("cli".into()), access: Some(AccessArg::ReadOnly) };
        assert_eq!(flags.clone().or_env(env(&[(CLIENT_ENV, "---"), (ACCESS_ENV, "readonly")])), Ok(flags));
        // An unusable value fails closed instead of running as the owner at full access.
        for (key, value) in [(CLIENT_ENV, "---"), (ACCESS_ENV, "readonly"), (ACCESS_ENV, "root")] {
            let error = none.clone().or_env(env(&[(key, value)])).unwrap_err();
            assert!(error.starts_with(&format!("{key}=\"{value}\"")), "{error}");
        }
    }

    #[test]
    fn client_names_need_a_usable_slug() {
        assert_eq!(client_name(" Hermes Agent "), Ok("Hermes Agent".to_string()));
        assert!(client_name("---").is_err());
        assert!(client_name("").is_err());
    }
}
