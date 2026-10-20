//! Settings and chat commands for extra provider accounts
//! (`crate::provider_accounts`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Runtime, State};

use crate::agent_provider::ProviderKind;
use crate::database::provider_accounts::ProviderAccountRow;
use crate::database::DatabaseStore;
use crate::provider_accounts::probe::{self, AccountIdentity};
use crate::provider_accounts::{self as accounts, homes, DEFAULT_ACCOUNT_ID};
use crate::state::AppStateStore;
use crate::terminal::PtyState;

const MAX_LABEL_CHARS: usize = 40;

/// One account as Settings and the composer show it.
#[derive(Debug, Clone, Serialize)]
pub struct ProviderAccountView {
    pub id: String,
    pub provider: String,
    /// `None` for the CLI's own sign-in.
    pub label: Option<String>,
    pub home: String,
    pub identity: AccountIdentity,
    /// Signs this account in when run in a terminal.
    pub login_command: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProviderAccountsState {
    /// The CLI's own sign-in, then extra accounts oldest first, per provider.
    pub accounts: Vec<ProviderAccountView>,
    /// App-wide default per provider; absent means the CLI's own sign-in.
    pub defaults: HashMap<String, String>,
}

/// Which account a chat runs on.
#[derive(Debug, Clone, Serialize)]
pub struct ThreadAccount {
    pub account_id: String,
    /// The chat has started and is pinned to the account; changing it is a
    /// switch rather than a pick for the next start.
    pub bound: bool,
}

fn provider_kind(slug: &str) -> Result<ProviderKind, String> {
    accounts::parse_provider(slug).ok_or_else(|| format!("{slug} does not support extra accounts"))
}

fn quote_unix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn quote_powershell(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// The CLI's login command, pointed at `home` for an extra account.
fn login_command(provider: ProviderKind, home: Option<&Path>) -> String {
    let base = crate::agent_provider::health::provider_login_command(provider).unwrap_or_default();
    let (Some(home), Some(key)) = (home, homes::home_env_key(provider)) else {
        return base.to_string();
    };
    let home = home.to_string_lossy();
    if cfg!(windows) {
        format!("$env:{key}={}; {base}", quote_powershell(&home))
    } else {
        format!("{key}={} {base}", quote_unix(&home))
    }
}

fn view(provider: ProviderKind, account: Option<&ProviderAccountRow>) -> ProviderAccountView {
    let slug = accounts::provider_slug(provider);
    match account {
        Some(account) => {
            let home = PathBuf::from(&account.home);
            ProviderAccountView {
                id: account.id.clone(),
                provider: slug.into(),
                label: Some(account.label.clone()),
                home: account.home.clone(),
                identity: probe::identity_now(provider, &home, false),
                login_command: login_command(provider, Some(&home)),
            }
        }
        None => {
            let home = homes::default_home(provider).unwrap_or_default();
            // With Codex's ChatGPT plan connected in Codemux, that route is
            // what the CLI-login account runs on, not the CLI's own login.
            let identity = if provider == ProviderKind::Codex
                && crate::agent_provider::codex::chatgpt::owner().managed_selected_now()
            {
                AccountIdentity {
                    signed_in: Some(true),
                    email: None,
                    organization: None,
                    plan: Some("ChatGPT plan connected in Codemux".into()),
                }
            } else {
                probe::identity_now(provider, &home, true)
            };
            ProviderAccountView {
                id: DEFAULT_ACCOUNT_ID.into(),
                provider: slug.into(),
                label: None,
                home: home.to_string_lossy().into_owned(),
                identity,
                login_command: login_command(provider, None),
            }
        }
    }
}

fn snapshot(db: &DatabaseStore) -> Result<ProviderAccountsState, String> {
    let rows = db.provider_accounts()?;
    let mut list = Vec::new();
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        list.push(view(provider, None));
        let slug = accounts::provider_slug(provider);
        list.extend(rows.iter().filter(|a| a.provider == slug).map(|a| view(provider, Some(a))));
    }
    Ok(ProviderAccountsState {
        accounts: list,
        defaults: db.provider_account_defaults("")?,
    })
}

/// Every account, without spawning anything.
#[tauri::command]
pub async fn provider_accounts_list(db: State<'_, DatabaseStore>) -> Result<ProviderAccountsState, String> {
    snapshot(&db)
}

/// Every account, after asking Codex who each of its homes is signed in as.
#[tauri::command]
pub async fn provider_accounts_refresh(db: State<'_, DatabaseStore>) -> Result<ProviderAccountsState, String> {
    let rows = db.provider_accounts()?;
    let mut homes_to_probe: Vec<(PathBuf, bool)> = rows
        .iter()
        .filter(|a| a.provider == "codex")
        .map(|a| (PathBuf::from(&a.home), false))
        .collect();
    if let Some(home) = homes::default_home(ProviderKind::Codex) {
        homes_to_probe.insert(0, (home, true));
    }
    futures_util::future::join_all(
        homes_to_probe
            .iter()
            .map(|(home, is_default)| probe::codex_identity(home, *is_default)),
    )
    .await;
    snapshot(&db)
}

fn clean_label(label: &str) -> Result<String, String> {
    let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
    if label.is_empty() {
        return Err("Give the account a name.".into());
    }
    if label.chars().count() > MAX_LABEL_CHARS {
        return Err(format!("Keep the name under {MAX_LABEL_CHARS} characters."));
    }
    Ok(label)
}

fn ensure_unique_label(db: &DatabaseStore, provider: &str, label: &str, except: Option<&str>) -> Result<(), String> {
    let taken = label.eq_ignore_ascii_case("default")
        || db.provider_accounts()?.iter().any(|a| {
            a.provider == provider && Some(a.id.as_str()) != except && a.label.eq_ignore_ascii_case(label)
        });
    if taken {
        return Err(format!("An account named \u{201c}{label}\u{201d} already exists."));
    }
    Ok(())
}

fn slug_of(label: &str) -> String {
    let mut slug = String::new();
    for c in label.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
        if slug.len() >= 24 {
            break;
        }
    }
    slug.trim_end_matches('-').to_string()
}

/// Add an account and prepare its home. Signing in is a separate step.
#[tauri::command]
pub async fn provider_account_add(
    db: State<'_, DatabaseStore>,
    provider: String,
    label: String,
) -> Result<ProviderAccountView, String> {
    let kind = provider_kind(&provider)?;
    let label = clean_label(&label)?;
    ensure_unique_label(&db, &provider, &label, None)?;
    let root = homes::accounts_root().ok_or("Could not find the app's config folder")?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let slug = slug_of(&label);
    let id = if slug.is_empty() {
        format!("{provider}-{}", &suffix[..8])
    } else {
        format!("{provider}-{slug}-{}", &suffix[..6])
    };
    let home = homes::account_home(&root, &provider, &id);
    homes::prepare_home(kind, &home).map_err(|e| format!("Could not create the account folder: {e}"))?;
    let row = ProviderAccountRow {
        id,
        provider,
        label,
        home: home.to_string_lossy().into_owned(),
        created_at_ms: chrono::Utc::now().timestamp_millis(),
    };
    db.insert_provider_account(&row)?;
    Ok(view(kind, Some(&row)))
}

#[tauri::command]
pub async fn provider_account_rename(
    db: State<'_, DatabaseStore>,
    account_id: String,
    label: String,
) -> Result<ProviderAccountsState, String> {
    let account = db.provider_account(&account_id)?.ok_or("That account no longer exists.")?;
    let label = clean_label(&label)?;
    ensure_unique_label(&db, &account.provider, &label, Some(&account.id))?;
    db.rename_provider_account(&account_id, &label)?;
    snapshot(&db)
}

/// Remove an account and delete its folder, which holds its sign-in and
/// the chat history made on it. Usage already counted stays counted.
#[tauri::command]
pub async fn provider_account_remove<R: Runtime>(
    app: AppHandle<R>,
    db: State<'_, DatabaseStore>,
    account_id: String,
) -> Result<ProviderAccountsState, String> {
    let _account_change = accounts::account_change_lock().await;
    let account = db.provider_account(&account_id)?.ok_or("That account no longer exists.")?;
    // Forget it first so nothing new resolves to it, then stop what still
    // runs signed in as it. Its chats restart on the next default.
    accounts::delete_account(&db, &account_id)?;
    for (provider, thread) in accounts::threads_on_account(&account_id) {
        // Stopping is thread-wide: check the chat still runs there, and stop
        // it, with no provider switch able to move it in between.
        let _still = crate::commands::provider_switch::hold_still(&thread).await;
        if !accounts::still_on_account(&db, provider, &thread, &account_id) {
            continue;
        }
        if let Err(error) = crate::commands::agent_chat::agent_chat_stop_session(
            app.clone(),
            provider,
            crate::agent_provider::ThreadId(thread.clone()),
        )
        .await
        {
            eprintln!("[codemux::provider_accounts] stopping a session on {account_id}: {error}");
        }
    }
    for side_id in accounts::side_chats_on_account(&account_id) {
        let _ = crate::commands::side_chat::agent_chat_side_chat_close(app.clone(), side_id).await;
    }
    let home = PathBuf::from(&account.home);
    // Only ever delete a folder Codemux made.
    let owned = homes::accounts_root().is_some_and(|root| home.starts_with(&root) && home != root);
    if owned && home.is_dir() {
        if let Err(error) = std::fs::remove_dir_all(&home) {
            eprintln!("[codemux::provider_accounts] removing {}: {error}", home.display());
        }
    }
    snapshot(&db)
}

/// Set the app-wide default for new chats. `account_id: None` clears it,
/// leaving the CLI's own sign-in.
#[tauri::command]
pub async fn provider_account_set_default(
    db: State<'_, DatabaseStore>,
    provider: String,
    account_id: Option<String>,
) -> Result<ProviderAccountsState, String> {
    provider_kind(&provider)?;
    if let Some(id) = account_id.as_deref().filter(|id| *id != DEFAULT_ACCOUNT_ID) {
        let account = db.provider_account(id)?.ok_or("That account no longer exists.")?;
        if account.provider != provider {
            return Err("That account belongs to another provider.".into());
        }
    }
    // The CLI's own sign-in is what an empty default means.
    let stored = account_id.as_deref().filter(|id| *id != DEFAULT_ACCOUNT_ID);
    db.set_provider_account_default("", &provider, stored)?;
    snapshot(&db)
}

/// Open a terminal tab in `workspace_id` that signs `account_id` in.
#[tauri::command]
pub fn provider_account_open_sign_in<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppStateStore>,
    pty_state: State<'_, PtyState>,
    db: State<'_, DatabaseStore>,
    workspace_id: String,
    account_id: String,
) -> Result<(), String> {
    // The CLI's own sign-in goes through `open_provider_login_terminal`.
    let account = db.provider_account(&account_id)?.ok_or("That account no longer exists.")?;
    let kind = provider_kind(&account.provider)?;
    let home = PathBuf::from(&account.home);
    homes::prepare_home(kind, &home).map_err(|e| format!("Could not prepare the account folder: {e}"))?;
    let command = login_command(kind, Some(&home));
    let title = format!("Sign in · {}", account.label);
    let (tab_id, session_id) = state.create_tab(&workspace_id, crate::state::TabKind::Terminal)?;
    let _ = state.rename_tab(&workspace_id, &tab_id, title);
    if let Some(session_id) = session_id {
        crate::terminal::spawn_pty_for_session(app.clone(), session_id.0.clone());
        state.update_terminal_session_command(&session_id.0, command.clone());
        crate::commands::presets::write_command_when_ready(pty_state.sessions.clone(), session_id.0, command, 120);
    }
    crate::state::emit_app_state(&app);
    Ok(())
}

/// Which account `thread_id` runs (or would start) on for `provider`.
#[tauri::command]
pub async fn provider_account_thread(
    db: State<'_, DatabaseStore>,
    thread_id: String,
    provider: String,
) -> Result<ThreadAccount, String> {
    let kind = provider_kind(&provider)?;
    let record = db.get_agent_chat_session(&thread_id).filter(|r| r.provider == provider);
    let resolution = accounts::resolve(
        &db,
        kind,
        Some(&thread_id),
        None,
        record.as_ref().is_some_and(|r| r.sdk_session_id.is_some()),
    )?;
    Ok(ThreadAccount {
        account_id: resolution.account_id().to_string(),
        // Any chat that has started runs somewhere: changing it is a switch,
        // even for a chat from before accounts existed (it has no binding).
        bound: record.is_some(),
    })
}

/// Move a started chat to another account.
#[tauri::command]
pub async fn provider_account_switch_thread<R: Runtime>(
    app: AppHandle<R>,
    thread_id: String,
    account_id: String,
) -> Result<accounts::SwitchOutcome, String> {
    accounts::switch_thread(&app, &thread_id, &account_id).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_commands_point_the_cli_at_the_account() {
        let home = Path::new("/cfg/provider-accounts/claude/claude-work-1");
        let command = login_command(ProviderKind::Claude, Some(home));
        if cfg!(windows) {
            assert!(command.starts_with("$env:CLAUDE_CONFIG_DIR="));
        } else {
            assert_eq!(command, "CLAUDE_CONFIG_DIR='/cfg/provider-accounts/claude/claude-work-1' claude auth login");
        }
        assert_eq!(login_command(ProviderKind::Codex, None), "codex login");
        assert_eq!(quote_unix("/a/it's"), "'/a/it'\\''s'");
    }

    #[test]
    fn labels_are_trimmed_bounded_and_unique() {
        assert_eq!(clean_label("  Work   laptop ").unwrap(), "Work laptop");
        assert!(clean_label("   ").is_err());
        assert!(clean_label(&"x".repeat(41)).is_err());
        let db = DatabaseStore::new_in_memory();
        db.insert_provider_account(&ProviderAccountRow {
            id: "a".into(),
            provider: "claude".into(),
            label: "Work".into(),
            home: String::new(),
            created_at_ms: 0,
        })
        .unwrap();
        assert!(ensure_unique_label(&db, "claude", "work", None).is_err());
        assert!(ensure_unique_label(&db, "claude", "Default", None).is_err());
        assert!(ensure_unique_label(&db, "claude", "work", Some("a")).is_ok());
        assert!(ensure_unique_label(&db, "codex", "Work", None).is_ok());
        assert_eq!(slug_of("Work laptop!"), "work-laptop");
        assert_eq!(slug_of("日本"), "");
    }
}
