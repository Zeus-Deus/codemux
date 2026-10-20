//! Several accounts per provider (Claude Code, Codex).
//!
//! The CLI's own sign-in is the implicit [`DEFAULT_ACCOUNT_ID`] account.
//! Every extra account is a separate CLI home (see [`homes`]) selected with
//! `CLAUDE_CONFIG_DIR` / `CODEX_HOME`, so each keeps its own login, plan
//! limits, and history while sharing the user's configuration.
//!
//! Which account a chat runs on, first match wins:
//! 1. the thread's binding (a chat stays on its account until switched);
//! 2. the account the caller picked for a new chat;
//! 3. the workspace's default;
//! 4. the app-wide default;
//! 5. the CLI's own sign-in.
//!
//! A thread that already has native history but no binding predates
//! accounts, so its transcript lives in the CLI's own home: it binds there.
//! Workspaces on another device run with that device's own sign-in and are
//! never given a local account.

pub mod homes;
pub mod probe;
pub mod usage;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tauri::{AppHandle, Manager, Runtime};

use crate::agent_provider::{ProviderKind, StartSessionInput, ThreadId};
use crate::database::provider_accounts::ProviderAccountRow;
use crate::database::DatabaseStore;

/// Id of the CLI's own sign-in.
pub const DEFAULT_ACCOUNT_ID: &str = "default";

/// `extra` key a caller uses to pick the account for a new chat.
pub const START_EXTRA_KEY: &str = "provider_account_id";

/// `extra` key naming the persisted chat a start resumes from (a history
/// pick under a fresh thread id). Its account holds the transcript.
pub const FROM_THREAD_EXTRA_KEY: &str = "provider_account_from_thread";

/// Set on a session that runs on an extra account. Codex reads it to keep
/// the app-owned ChatGPT route from replacing the chosen CLI login.
pub const SESSION_ACCOUNT_ENV: &str = "CODEMUX_PROVIDER_ACCOUNT";

pub fn supports(provider: ProviderKind) -> bool {
    matches!(provider, ProviderKind::Claude | ProviderKind::Codex)
}

pub fn provider_slug(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "claude",
        ProviderKind::Codex => "codex",
        ProviderKind::Cursor => "cursor",
        ProviderKind::Grok => "grok",
        ProviderKind::Hermes => "hermes",
        ProviderKind::Acp => "acp",
        ProviderKind::OpenCode => "opencode",
    }
}

pub fn parse_provider(slug: &str) -> Option<ProviderKind> {
    match slug {
        "claude" => Some(ProviderKind::Claude),
        "codex" => Some(ProviderKind::Codex),
        _ => None,
    }
}

/// Key for an account's plan quota: the bare provider for the CLI's own
/// sign-in (what every existing reader expects), `provider@id` otherwise.
pub fn quota_key(provider: &str, account_id: Option<&str>) -> String {
    match account_id {
        Some(id) if id != DEFAULT_ACCOUNT_ID => format!("{provider}@{id}"),
        _ => provider.to_string(),
    }
}

/// The quota key for whatever account `thread_id` runs on.
pub fn quota_key_for_thread<R: Runtime>(app: &AppHandle<R>, provider: &str, thread_id: &str) -> String {
    let account = app
        .try_state::<DatabaseStore>()
        .and_then(|db| db.provider_account_binding(thread_id, provider).ok().flatten());
    quota_key(provider, account.as_deref())
}

/// What the resolver decided.
#[derive(Debug, Clone, PartialEq)]
pub struct Resolution {
    /// `None` is the CLI's own sign-in.
    pub account: Option<ProviderAccountRow>,
    /// The thread already had a binding.
    pub bound: bool,
}

impl Resolution {
    pub fn account_id(&self) -> &str {
        self.account.as_ref().map(|a| a.id.as_str()).unwrap_or(DEFAULT_ACCOUNT_ID)
    }
}

/// An account id that still names an extra account of `provider`.
/// `Some(None)` is the explicit default; `None` means "not usable, keep
/// looking".
fn lookup(db: &DatabaseStore, provider: &str, id: &str) -> Result<Option<Option<ProviderAccountRow>>, String> {
    if id == DEFAULT_ACCOUNT_ID {
        return Ok(Some(None));
    }
    Ok(db
        .provider_account(id)?
        .filter(|account| account.provider == provider)
        .map(Some))
}

/// Resolve the account for a session (see the module docs for the order).
/// `has_history` is whether the thread already has native history.
pub fn resolve(
    db: &DatabaseStore,
    provider: ProviderKind,
    thread_id: Option<&str>,
    explicit: Option<&str>,
    workspace_id: Option<&str>,
    has_history: bool,
) -> Result<Resolution, String> {
    let slug = provider_slug(provider);
    if let Some(thread_id) = thread_id {
        if let Some(id) = db.provider_account_binding(thread_id, slug)? {
            if let Some(account) = lookup(db, slug, &id)? {
                return Ok(Resolution { account, bound: true });
            }
        }
    }
    let fallback = Resolution { account: None, bound: false };
    if has_history {
        return Ok(fallback);
    }
    let candidates = [
        explicit.map(str::to_string),
        workspace_id.map(|ws| db.provider_account_default(ws, slug)).transpose()?.flatten(),
        db.provider_account_default("", slug)?,
    ];
    for id in candidates.into_iter().flatten() {
        if let Some(account) = lookup(db, slug, &id)? {
            return Ok(Resolution { account, bound: false });
        }
    }
    Ok(fallback)
}

/// The account each live session was last started on, by thread. Lets a
/// switch confirm the session really runs on the new account, and lets
/// removing an account stop the sessions still using it.
static APPLIED: std::sync::Mutex<Option<HashMap<String, (ProviderKind, String)>>> = std::sync::Mutex::new(None);

fn record_applied(thread_id: &str, provider: ProviderKind, account_id: &str) {
    if let Ok(mut map) = APPLIED.lock() {
        map.get_or_insert_with(HashMap::new)
            .insert(thread_id.to_string(), (provider, account_id.to_string()));
    }
}

/// Threads whose session was started on `account_id`.
pub fn threads_on_account(account_id: &str) -> Vec<(ProviderKind, String)> {
    APPLIED
        .lock()
        .ok()
        .and_then(|map| {
            map.as_ref().map(|m| {
                m.iter()
                    .filter(|(_, (_, id))| id == account_id)
                    .map(|(thread, (provider, _))| (*provider, thread.clone()))
                    .collect()
            })
        })
        .unwrap_or_default()
}

/// Chats whose native id left with a removed account. An open pane may
/// still hold the old cursor and offer it on its next restart; it names a
/// transcript that is gone or an older copy, so that start goes fresh.
static STALE_CURSORS: std::sync::Mutex<Option<HashMap<String, HashSet<String>>>> = std::sync::Mutex::new(None);

/// Remember the native sessions a removal cleared, by thread, so a pane
/// still holding one of them starts fresh instead of resuming it.
pub fn forget_cursors(cleared: Vec<(String, String)>) {
    if let Ok(mut map) = STALE_CURSORS.lock() {
        let map = map.get_or_insert_with(Default::default);
        for (thread, native) in cleared {
            map.entry(thread).or_default().insert(native);
        }
    }
}

/// Whether `cursor` resumes a native session that removal cleared for
/// `thread_id`. Only those exact sessions are refused, and they stay refused
/// (a failed start leaves the pane holding the same cursor); a newer session
/// the thread started since is left alone.
fn is_stale_cursor(thread_id: &str, cursor: &serde_json::Value) -> bool {
    let Ok(map) = STALE_CURSORS.lock() else { return false };
    map.as_ref()
        .and_then(|m| m.get(thread_id))
        .is_some_and(|stale| stale.iter().any(|id| cursor_mentions(cursor, id)))
}

/// Cursors are provider-shaped JSON; look for the native id anywhere in it.
fn cursor_mentions(cursor: &serde_json::Value, id: &str) -> bool {
    match cursor {
        serde_json::Value::String(s) => s == id,
        serde_json::Value::Array(items) => items.iter().any(|v| cursor_mentions(v, id)),
        serde_json::Value::Object(fields) => fields.values().any(|v| cursor_mentions(v, id)),
        _ => false,
    }
}

/// Side chats started on an extra account, by side chat id.
static SIDE_CHATS: std::sync::Mutex<Option<HashMap<String, String>>> = std::sync::Mutex::new(None);

/// Remember that side chat `side_id` runs on the account `env` names, so
/// removing that account can close it.
pub fn note_side_chat(side_id: &str, env: &HashMap<String, String>) {
    let (Some(account), Ok(mut map)) = (env.get(SESSION_ACCOUNT_ENV), SIDE_CHATS.lock()) else {
        return;
    };
    map.get_or_insert_with(HashMap::new).insert(side_id.to_string(), account.clone());
}

/// Side chats started on `account_id` (some may already be closed).
pub fn side_chats_on_account(account_id: &str) -> Vec<String> {
    SIDE_CHATS
        .lock()
        .ok()
        .and_then(|map| {
            map.as_ref()
                .map(|m| m.iter().filter(|(_, id)| *id == account_id).map(|(side, _)| side.clone()).collect())
        })
        .unwrap_or_default()
}

/// Point the session's env at the account's home. The CLI's own sign-in
/// needs nothing: the CLI already finds its home unaided.
pub fn apply_env(
    provider: ProviderKind,
    account: Option<&ProviderAccountRow>,
    env: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let (Some(account), Some(key)) = (account, homes::home_env_key(provider)) else {
        return env;
    };
    let home = PathBuf::from(&account.home);
    if let Err(error) = homes::prepare_home(provider, &home) {
        eprintln!("[codemux::provider_accounts] preparing {}: {error}", home.display());
    }
    let mut env = env.unwrap_or_default();
    env.insert(key.to_string(), account.home.clone());
    env.insert(SESSION_ACCOUNT_ENV.to_string(), account.id.clone());
    Some(env)
}

/// Start hook: resolve, bind, and set the env for a session about to start.
/// Called once the workspace env overlay is in place.
/// Never fails the start: an account that cannot be resolved leaves the
/// session on the CLI's own sign-in.
pub fn prepare_start<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    input: &mut StartSessionInput,
    on_device: bool,
) {
    if !supports(provider) || on_device {
        return;
    }
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    if let Err(error) = prepare_start_with(&db, provider, input) {
        eprintln!("[codemux::provider_accounts] start {}: {error}", input.thread_id.0);
    }
}

/// The account a session about to start resolves to. Shared by the start
/// hook and the start path's Claude resume preflight so both look in the
/// same home.
fn start_resolution(
    db: &DatabaseStore,
    provider: ProviderKind,
    input: &StartSessionInput,
) -> Result<Resolution, String> {
    let slug = provider_slug(provider);
    let extra = |key: &str| input.extra.get(key).and_then(serde_json::Value::as_str).map(str::to_string);
    let explicit = extra(START_EXTRA_KEY);
    let has_history = input.resume_cursor.is_some();
    // A binding is only authoritative once this provider has run the
    // thread; one left by a failed first start or handoff is provisional.
    let started = db
        .get_agent_chat_session(&input.thread_id.0)
        .is_some_and(|record| record.provider == slug);
    if started {
        return resolve(db, provider, Some(&input.thread_id.0), None, input.workspace_id.as_deref(), has_history);
    }
    // Reopening another persisted chat: its account has the transcript, and
    // stays the chat's account even if a stale cursor was dropped.
    if let Some(from) = extra(FROM_THREAD_EXTRA_KEY) {
        return resolve(db, provider, Some(&from), None, None, true);
    }
    resolve(db, provider, None, explicit.as_deref(), input.workspace_id.as_deref(), has_history)
}

fn prepare_start_with(
    db: &DatabaseStore,
    provider: ProviderKind,
    input: &mut StartSessionInput,
) -> Result<(), String> {
    if input.resume_cursor.as_ref().is_some_and(|c| is_stale_cursor(&input.thread_id.0, c)) {
        input.resume_cursor = None;
    }
    let resolution = start_resolution(db, provider, input)?;
    db.set_provider_account_binding(&input.thread_id.0, provider_slug(provider), resolution.account_id())?;
    record_applied(&input.thread_id.0, provider, resolution.account_id());
    input.env = apply_env(provider, resolution.account.as_ref(), input.env.take());
    Ok(())
}

/// Start-path Claude resume preflight: is `session_id`'s transcript
/// definitely gone from the home this start will use?
pub fn claude_start_session_missing<R: Runtime>(
    app: &AppHandle<R>,
    input: &StartSessionInput,
    session_id: &str,
) -> bool {
    let account = app
        .try_state::<DatabaseStore>()
        .and_then(|db| start_resolution(&db, ProviderKind::Claude, input).ok())
        .and_then(|resolution| resolution.account);
    match account {
        Some(account) => homes::claude_transcript_missing(std::path::Path::new(&account.home), session_id),
        None => crate::commands::agent_chat::claude_session_file_missing(session_id),
    }
}

/// A provider may hand back a different thread id than requested; the
/// binding follows the session.
pub fn follow_thread<R: Runtime>(app: &AppHandle<R>, from: &str, to: &str) {
    let Some(db) = app.try_state::<DatabaseStore>() else { return };
    let _ = db.copy_provider_account_bindings(from, to);
    let applied = APPLIED.lock().ok().and_then(|m| m.as_ref().and_then(|m| m.get(from).cloned()));
    if let Some((provider, account)) = applied {
        record_applied(to, provider, &account);
    }
}

/// Rebuild hook (`ensure_live_session`): the persisted thread resumes on
/// the account it is bound to.
pub fn rebuild_env<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    thread_id: &str,
    env: Option<HashMap<String, String>>,
    on_device: bool,
) -> Option<HashMap<String, String>> {
    if !supports(provider) || on_device {
        return env;
    }
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return env;
    };
    match resolve(&db, provider, Some(thread_id), None, None, true) {
        Ok(resolution) => {
            if !resolution.bound {
                let _ = db.set_provider_account_binding(thread_id, provider_slug(provider), DEFAULT_ACCOUNT_ID);
            }
            record_applied(thread_id, provider, resolution.account_id());
            apply_env(provider, resolution.account.as_ref(), env)
        }
        Err(error) => {
            eprintln!("[codemux::provider_accounts] resolving {thread_id}: {error}");
            env
        }
    }
}

/// Resume preflight for Claude that looks in the session's own home.
pub fn claude_session_missing(env: Option<&HashMap<String, String>>, session_id: &str) -> bool {
    match env.and_then(|env| env.get("CLAUDE_CONFIG_DIR")) {
        Some(home) => homes::claude_transcript_missing(std::path::Path::new(home), session_id),
        None => crate::commands::agent_chat::claude_session_file_missing(session_id),
    }
}

/// The env a one-off process forking `thread_id`'s conversation (a side
/// chat) needs to find it: the account the thread is bound to.
pub fn session_env_for_thread<R: Runtime>(
    app: &AppHandle<R>,
    provider: ProviderKind,
    thread_id: &str,
) -> HashMap<String, String> {
    if !supports(provider) {
        return HashMap::new();
    }
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return HashMap::new();
    };
    resolve(&db, provider, Some(thread_id), None, None, true)
        .ok()
        .and_then(|resolution| apply_env(provider, resolution.account.as_ref(), None))
        .unwrap_or_default()
}

/// Whether a Codex process started with `env` would run on the app-owned
/// ChatGPT route (`route_selected`): only the CLI-login account does; an
/// extra account always runs its own login. Side chats and forks can't
/// follow that route, so they refuse exactly these sessions.
pub fn on_chatgpt_route(provider: ProviderKind, env: &HashMap<String, String>, route_selected: bool) -> bool {
    provider == ProviderKind::Codex && route_selected && !env.contains_key(SESSION_ACCOUNT_ENV)
}

/// Whether Codex's app-owned ChatGPT route serves the CLI-login account
/// right now. That route keeps its own home, so a transcript cannot be
/// carried into or out of it.
fn chatgpt_route_active(provider: ProviderKind) -> bool {
    provider == ProviderKind::Codex && crate::agent_provider::codex::chatgpt::owner().managed_selected_now()
}

/// Each extra account's history folder, for the usage importer:
/// `(root, provider, account id)`.
pub fn history_roots(accounts: &[ProviderAccountRow]) -> Vec<(PathBuf, &'static str, Option<String>)> {
    accounts
        .iter()
        .filter_map(|account| {
            let home = PathBuf::from(&account.home);
            let (root, provider) = match account.provider.as_str() {
                "claude" => (home.join("projects"), "claude"),
                "codex" => (home.join("sessions"), "codex"),
                _ => return None,
            };
            Some((root, provider, Some(account.id.clone())))
        })
        .collect()
}

/// The env that points `provider`'s CLI at each account, for one-off reads
/// such as plan limits: `(quota key, env)`, CLI sign-in first.
pub fn probe_targets(
    provider: ProviderKind,
    accounts: &[ProviderAccountRow],
) -> Vec<(String, HashMap<String, String>)> {
    let slug = provider_slug(provider);
    let mut targets = vec![(slug.to_string(), HashMap::new())];
    let Some(key) = homes::home_env_key(provider) else {
        return targets;
    };
    for account in accounts.iter().filter(|a| a.provider == slug) {
        let mut env = HashMap::new();
        env.insert(key.to_string(), account.home.clone());
        targets.push((quota_key(slug, Some(&account.id)), env));
    }
    targets
}

/// Extra account `account_id`'s CLI home, ready to use, with its id.
/// `None` for the CLI's own sign-in.
pub fn account_home(
    db: &DatabaseStore,
    provider: ProviderKind,
    account_id: Option<&str>,
) -> Result<Option<(String, PathBuf)>, String> {
    let Some(id) = account_id.filter(|id| *id != DEFAULT_ACCOUNT_ID) else {
        return Ok(None);
    };
    let account = lookup(db, provider_slug(provider), id)?
        .flatten()
        .ok_or_else(|| "That account no longer exists.".to_string())?;
    let home = PathBuf::from(&account.home);
    homes::prepare_home(provider, &home).map_err(|e| format!("Could not prepare the account folder: {e}"))?;
    Ok(Some((account.id, home)))
}

/// The home a thread's account lives in.
fn home_of(provider: ProviderKind, account: Option<&ProviderAccountRow>) -> Option<PathBuf> {
    match account {
        Some(account) => Some(PathBuf::from(&account.home)),
        None => homes::default_home(provider),
    }
}

/// What a switch did, for the toast.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SwitchOutcome {
    pub account_id: String,
    /// The conversation's native transcript came along, so the agent keeps
    /// its context. `false` means the chat continues with fresh context.
    pub carried: bool,
}

/// Held for a whole switch and a whole account removal, so an account can
/// never disappear between a switch choosing it and the chat running on it.
static ACCOUNT_CHANGE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Serialize with in-flight switches (see [`ACCOUNT_CHANGE`]).
pub async fn account_change_lock() -> tokio::sync::MutexGuard<'static, ()> {
    ACCOUNT_CHANGE.lock().await
}

/// Move a chat to another account: stop its session, bring its native
/// transcript along, rebind, and start it again on the new account.
pub async fn switch_thread<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: &str,
    account_id: &str,
) -> Result<SwitchOutcome, String> {
    let _account_change = account_change_lock().await;
    let db = app.state::<DatabaseStore>();
    let record = db
        .get_agent_chat_session(thread_id)
        .ok_or_else(|| "Start the chat before choosing its account.".to_string())?;
    let provider = parse_provider(&record.provider)
        .ok_or_else(|| "This chat's provider has no accounts to switch between.".to_string())?;
    if workspace_on_device(app, &record.workspace_id) {
        return Err("This chat runs on another device, which uses its own sign-in.".into());
    }
    let slug = provider_slug(provider);
    let target = lookup(&db, slug, account_id)?
        .ok_or_else(|| "That account no longer exists.".to_string())?;
    let current = resolve(&db, provider, Some(thread_id), None, None, record.sdk_session_id.is_some())?;
    let target_id = target.as_ref().map(|a| a.id.clone()).unwrap_or_else(|| DEFAULT_ACCOUNT_ID.into());
    if current.account_id() == target_id {
        return Ok(SwitchOutcome { account_id: target_id, carried: true });
    }

    let thread = ThreadId(thread_id.to_string());
    let crosses_chatgpt_route =
        chatgpt_route_active(provider) && (current.account.is_none() || target.is_none());
    let from = home_of(provider, current.account.as_ref());
    let to = home_of(provider, target.as_ref());
    let native = record.sdk_session_id.clone();
    drop(db);
    // Stop, carry, rebind, and start again without letting a send in
    // between: it would run on the old account after the copy was taken.
    let carried = crate::commands::agent_chat::restart_session_holding_gate(app, provider, &thread, async {
        let db = app.state::<DatabaseStore>();
        // Read the history into the ledger first so turns made on the old
        // account are attributed to it before the copy duplicates them.
        if let Err(error) = crate::commands::usage_import::usage_scan_provider_history(app.state()).await {
            eprintln!("[codemux::provider_accounts] usage scan before switch: {error}");
        }
        let carried = match (native.as_deref(), from.as_ref(), to.as_ref()) {
            (Some(native), Some(from), Some(to)) if !crosses_chatgpt_route => {
                if target.is_some() {
                    let _ = homes::prepare_home(provider, to);
                }
                homes::carry_transcript(provider, from, to, native).unwrap_or_else(|error| {
                    eprintln!("[codemux::provider_accounts] carrying {native}: {error}");
                    false
                })
            }
            _ => false,
        };
        // Without the transcript the old native id names nothing on the
        // new account; start fresh rather than resume a stranger's id.
        if !carried && native.is_some() {
            db.clear_agent_chat_sdk_session_id(thread_id)?;
        }
        db.set_provider_account_binding(thread_id, slug, &target_id)?;
        Ok(carried)
    })
    .await?;
    persist_live_cursor(app, provider, &thread).await;
    Ok(SwitchOutcome { account_id: target_id, carried })
}

/// Record the rebuilt session's native id, so the next switch or reopen
/// resumes the conversation that now runs (a fresh start has a new one).
async fn persist_live_cursor<R: Runtime>(app: &AppHandle<R>, provider: ProviderKind, thread: &ThreadId) {
    let registry = app.state::<crate::commands::agent_chat::ProviderRegistry>();
    let Ok(provider_impl) = crate::commands::agent_chat::lookup_provider(&registry, provider).await else {
        return;
    };
    let Ok(sessions) = provider_impl.list_sessions().await else { return };
    let native = sessions
        .into_iter()
        .find(|s| &s.thread_id == thread)
        .and_then(|s| s.resume_cursor)
        .and_then(|cursor| crate::commands::agent_chat::extract_sdk_session_id(&cursor));
    if let Some(native) = native {
        let _ = app.state::<DatabaseStore>().set_agent_chat_sdk_session_id(&thread.0, &native);
    }
}

fn workspace_on_device<R: Runtime>(app: &AppHandle<R>, workspace_id: &str) -> bool {
    app.try_state::<crate::state::AppStateStore>().is_some_and(|state| {
        state
            .snapshot()
            .workspaces
            .iter()
            .any(|w| w.workspace_id.0 == workspace_id && w.host_id.is_some())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with(accounts: &[(&str, &str)]) -> DatabaseStore {
        let db = DatabaseStore::new_in_memory();
        for (i, (id, provider)) in accounts.iter().enumerate() {
            db.insert_provider_account(&ProviderAccountRow {
                id: (*id).into(),
                provider: (*provider).into(),
                label: (*id).into(),
                home: format!("/homes/{id}"),
                created_at_ms: i as i64,
            })
            .unwrap();
        }
        db
    }

    fn resolved(db: &DatabaseStore, thread: &str, explicit: Option<&str>, ws: Option<&str>, history: bool) -> String {
        resolve(db, ProviderKind::Claude, Some(thread), explicit, ws, history)
            .unwrap()
            .account_id()
            .to_string()
    }

    #[test]
    fn resolution_order_is_binding_pick_workspace_global_cli() {
        let db = db_with(&[("work", "claude"), ("side", "claude"), ("cx", "codex")]);
        assert_eq!(resolved(&db, "t", None, None, false), "default");
        db.set_provider_account_default("", "claude", Some("work")).unwrap();
        assert_eq!(resolved(&db, "t", None, Some("ws"), false), "work");
        db.set_provider_account_default("ws", "claude", Some("side")).unwrap();
        assert_eq!(resolved(&db, "t", None, Some("ws"), false), "side");
        assert_eq!(resolved(&db, "t", Some("default"), Some("ws"), false), "default");
        // Another provider's account is never used.
        assert_eq!(resolved(&db, "t", Some("cx"), Some("ws"), false), "side");
        db.set_provider_account_binding("t", "claude", "work").unwrap();
        assert_eq!(resolved(&db, "t", Some("side"), Some("ws"), false), "work");
    }

    #[test]
    fn unbound_history_stays_on_the_cli_sign_in() {
        let db = db_with(&[("work", "claude")]);
        db.set_provider_account_default("", "claude", Some("work")).unwrap();
        assert_eq!(resolved(&db, "old", None, None, true), "default");
    }

    #[test]
    fn a_removed_account_falls_through() {
        let db = db_with(&[("work", "claude")]);
        db.set_provider_account_binding("t", "claude", "gone").unwrap();
        db.set_provider_account_default("", "claude", Some("work")).unwrap();
        assert_eq!(resolved(&db, "t", None, None, false), "work");
        // A binding for another provider (the chat switched providers) does
        // not pin this one.
        db.set_provider_account_binding("t2", "codex", "work").unwrap();
        assert_eq!(resolved(&db, "t2", None, None, false), "work");
    }

    fn input(thread: &str, resume: bool, extra: serde_json::Value) -> StartSessionInput {
        StartSessionInput {
            thread_id: ThreadId(thread.into()),
            cwd: "/repo".into(),
            model: None,
            resume_cursor: resume.then(|| serde_json::json!({ "resume": "native-1" })),
            fresh_session: false,
            permission_mode: None,
            effort: None,
            context_window: None,
            fast_mode: false,
            service_tier: None,
            additional_directories: vec![],
            env: None,
            workspace_id: None,
            remote: None,
            extra,
            recorded_usage_baseline: None,
        }
    }

    #[test]
    fn a_binding_from_a_failed_first_start_does_not_pin_the_retry() {
        let db = db_with(&[("a", "claude"), ("b", "claude")]);
        db.set_provider_account_binding("draft", "claude", "a").unwrap();
        let pick_b = serde_json::json!({ START_EXTRA_KEY: "b" });
        let start = |extra| start_resolution(&db, ProviderKind::Claude, &input("draft", false, extra)).unwrap();
        assert_eq!(start(pick_b.clone()).account_id(), "b");
        // Once the provider has run the thread, its binding wins.
        db.upsert_agent_chat_session("draft", "ws", Some("/repo"), "claude").unwrap();
        assert_eq!(start(pick_b.clone()).account_id(), "a");
        // A row another provider wrote (failed handoff) is not this one's.
        db.set_provider_account_binding("draft", "codex", "x").unwrap();
        let codex = start_resolution(&db, ProviderKind::Codex, &input("draft", false, serde_json::Value::Null)).unwrap();
        assert_eq!(codex.account_id(), "default");
    }

    #[test]
    fn a_history_pick_resumes_on_the_account_that_holds_it() {
        let db = db_with(&[("work", "codex")]);
        db.set_provider_account_binding("old-thread", "codex", "work").unwrap();
        let extra = serde_json::json!({ FROM_THREAD_EXTRA_KEY: "old-thread" });
        let picked = start_resolution(&db, ProviderKind::Codex, &input("new-thread", true, extra)).unwrap();
        assert_eq!(picked.account_id(), "work");
        let unbound = start_resolution(&db, ProviderKind::Codex, &input("new-thread", true, serde_json::Value::Null)).unwrap();
        assert_eq!(unbound.account_id(), "default");
    }

    #[test]
    fn only_the_removed_native_session_is_dropped() {
        forget_cursors(vec![("stale-t".into(), "native-1".into())]);
        let newer = serde_json::json!({ "resume": "native-2" });
        let removed = serde_json::json!({ "resume": "native-1" });
        assert!(!is_stale_cursor("stale-t", &newer));
        assert!(!is_stale_cursor("other-t", &removed));
        assert!(is_stale_cursor("stale-t", &removed));
        // A retry after a failed start is refused too.
        assert!(is_stale_cursor("stale-t", &removed));
    }

    #[test]
    fn env_points_the_cli_at_the_account_home() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("work");
        let account = ProviderAccountRow {
            id: "work".into(),
            provider: "codex".into(),
            label: "Work".into(),
            home: home.to_string_lossy().into_owned(),
            created_at_ms: 0,
        };
        let env = apply_env(ProviderKind::Codex, Some(&account), None).unwrap();
        assert_eq!(env["CODEX_HOME"], account.home);
        assert_eq!(env[SESSION_ACCOUNT_ENV], "work");
        assert!(home.is_dir());
        assert_eq!(apply_env(ProviderKind::Codex, None, None), None);
    }

    #[test]
    fn only_the_cli_login_follows_the_chatgpt_route() {
        let cli = HashMap::new();
        let extra = HashMap::from([(SESSION_ACCOUNT_ENV.to_string(), "work".to_string())]);
        assert!(on_chatgpt_route(ProviderKind::Codex, &cli, true));
        assert!(!on_chatgpt_route(ProviderKind::Codex, &extra, true));
        assert!(!on_chatgpt_route(ProviderKind::Codex, &cli, false));
        assert!(!on_chatgpt_route(ProviderKind::Claude, &cli, true));
    }

    #[test]
    fn model_discovery_finds_the_extra_accounts_own_home() {
        let tmp = tempfile::tempdir().unwrap();
        let db = DatabaseStore::new_in_memory();
        db.insert_provider_account(&ProviderAccountRow {
            id: "work".into(),
            provider: "codex".into(),
            label: "Work".into(),
            home: tmp.path().join("work").to_string_lossy().into_owned(),
            created_at_ms: 0,
        })
        .unwrap();
        assert_eq!(account_home(&db, ProviderKind::Codex, None).unwrap(), None);
        assert_eq!(account_home(&db, ProviderKind::Codex, Some("default")).unwrap(), None);
        let (id, home) = account_home(&db, ProviderKind::Codex, Some("work")).unwrap().unwrap();
        assert_eq!((id.as_str(), home.as_path()), ("work", tmp.path().join("work").as_path()));
        assert!(home.is_dir());
        assert!(account_home(&db, ProviderKind::Codex, Some("gone")).is_err());
        assert!(account_home(&db, ProviderKind::Claude, Some("work")).is_err());
    }

    #[test]
    fn quota_keys_keep_the_bare_provider_for_the_cli_sign_in() {
        assert_eq!(quota_key("claude", None), "claude");
        assert_eq!(quota_key("claude", Some("default")), "claude");
        assert_eq!(quota_key("codex", Some("work")), "codex@work");
    }
}
