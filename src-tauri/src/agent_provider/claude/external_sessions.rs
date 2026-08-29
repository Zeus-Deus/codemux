//! Discovery of Claude Code conversations that already exist on this
//! machine but were never created inside Codemux.
//!
//! The SDK exposes a supported session-history API; the sidecar's
//! `list-sessions` JSON-RPC method wraps it. Codemux never reads the
//! provider's on-disk transcript layout — that format is documented as
//! internal and unstable, so the SDK is the only route.
//!
//! Unlike `list-models` / `list-commands` this probe needs no transient
//! `query()` handshake and no path to the CLI binary: it is a pure
//! metadata read, cheap enough to run every time the picker opens.
//! Results are deliberately NOT cached — history changes while the app
//! runs, and a stale picker is exactly the problem this solves.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use crate::agent_provider::errors::ProviderError;
use crate::agent_provider::types::{
    ExternalSession, ExternalSessionScope, ExternalSessionTitleSource,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

use super::protocol::METHOD_LIST_SESSIONS;

/// Spawn + request budget for the metadata probe. Same 20s ceiling the
/// other transient sidecar harvests use.
const PROBE_TIMEOUT: Duration = Duration::from_secs(20);

/// Below this transcript size a session with no title of any kind is
/// treated as a zero-message stub — a conversation that was opened and
/// abandoned before the first turn. Offering those is pure noise.
pub const EXTERNAL_SESSION_MIN_BYTES: u64 = 512;

/// Wire shape of one entry in the sidecar's `list-sessions` response.
/// Fields arrive camelCase; every one is `#[serde(default)]` so a future
/// SDK addition (or omission) cannot break decode.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SdkExternalSession {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    git_branch: Option<String>,
    #[serde(default)]
    last_modified: String,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    file_size: u64,
    #[serde(default)]
    title_source: Option<ExternalSessionTitleSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ListSessionsResponse {
    #[serde(default)]
    sessions: Vec<SdkExternalSession>,
    /// Rows the sidecar dropped because they carried no usable `cwd`.
    /// Diagnostic only — logged, never surfaced.
    #[serde(default)]
    skipped_without_cwd: u32,
}

/// Live-list the adoptable Claude conversations for `scope`.
///
/// Spawns a transient sidecar, sends one `list-sessions` request, and
/// shuts the child down on every path. `sidecar_binary` comes from the
/// provider that already resolved it at construction time, so this never
/// re-resolves the bundle and never looks for the CLI on PATH.
pub async fn list_adoptable_sessions(
    sidecar_binary: &Path,
    scope: &ExternalSessionScope,
) -> Result<Vec<ExternalSession>, ProviderError> {
    let mut params = json!({
        // Sent explicitly so the SDK-side default can never drift
        // underneath us. Meaningless without `dir`, hence false when the
        // scope is widened to every project.
        "includeWorktrees": !scope.all_projects && scope.include_worktrees,
        "limit": scope.limit,
    });
    if !scope.all_projects {
        params["dir"] = json!(scope.current_cwd.to_string_lossy());
    }

    let child = tokio::time::timeout(
        PROBE_TIMEOUT,
        JsonRpcChild::spawn(SpawnConfig {
            program: sidecar_binary.to_path_buf(),
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            default_timeout: PROBE_TIMEOUT,
        }),
    )
    .await
    .map_err(|_| ProviderError::Timeout {
        operation: "list-sessions sidecar spawn".to_string(),
        elapsed_ms: PROBE_TIMEOUT.as_millis() as u64,
    })?
    .map_err(|e| ProviderError::ProcessError {
        message: "failed to spawn claude-agent sidecar".to_string(),
        source: Some(e.to_string()),
    })?;

    let response = child.request(METHOD_LIST_SESSIONS, params).await;
    let _ = child.shutdown().await;
    let response = response.map_err(|e| ProviderError::RpcError {
        message: format!("list-sessions RPC failed: {e}"),
    })?;

    let parsed: ListSessionsResponse =
        serde_json::from_value(response).map_err(|e| ProviderError::RpcError {
            message: format!("list-sessions decode failed: {e}"),
        })?;

    if parsed.skipped_without_cwd > 0 {
        eprintln!(
            "[codemux::claude] list-sessions skipped {} session(s) with no working directory",
            parsed.skipped_without_cwd
        );
    }

    Ok(adoptable_from_response(parsed.sessions))
}

/// Convert decoded wire rows into [`ExternalSession`]s, dropping the ones
/// that can never be adopted or that are pure picker noise.
///
/// Split from the RPC so the whole conversion is unit-testable without
/// spawning anything.
fn adoptable_from_response(rows: Vec<SdkExternalSession>) -> Vec<ExternalSession> {
    let temp_dir = std::env::temp_dir();
    rows.into_iter()
        .filter_map(to_external_session)
        .filter(|s| is_offerable_in(s, &temp_dir))
        .collect()
}

/// Normalise one wire row. `None` for rows with no id or no working
/// directory: adoption attaches to the folder the session lives in, so a
/// session without one is not adoptable at all.
fn to_external_session(row: SdkExternalSession) -> Option<ExternalSession> {
    let session_id = row.session_id.trim().to_string();
    let cwd = row.cwd.trim().to_string();
    if session_id.is_empty() || cwd.is_empty() {
        return None;
    }
    let title = row.title.trim();
    let (title, title_source) = if title.is_empty() {
        // The sidecar always resolves a title; this only covers a
        // provider that starts returning blanks. Synthesise one rather
        // than rendering an empty row, and mark it as a fallback so the
        // stub filter below can still act on it.
        (
            fallback_title(&cwd, &session_id),
            ExternalSessionTitleSource::Fallback,
        )
    } else {
        (
            title.to_string(),
            row.title_source
                .unwrap_or(ExternalSessionTitleSource::Fallback),
        )
    };
    Some(ExternalSession {
        session_id,
        title,
        cwd,
        git_branch: row.git_branch.filter(|b| !b.trim().is_empty()),
        last_modified: row.last_modified,
        created_at: row.created_at,
        file_size: row.file_size,
        title_source,
    })
}

/// Last-resort label: the directory's basename plus a short id, so two
/// untitled sessions in the same folder stay distinguishable.
fn fallback_title(cwd: &str, session_id: &str) -> String {
    let base = Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| cwd.to_string());
    let short: String = session_id.chars().take(8).collect();
    format!("{base} ({short})")
}

/// Whether a discovered session is worth offering, judged against
/// `temp_dir` so the rule is testable without touching the real one.
///
/// Three drops, all cheap:
/// 1. cwd under the temp directory — throwaway fixture runs, not work
///    the user wants back (a live scan of one machine's history found a
///    quarter of it was exactly this).
/// 2. cwd that is DEFINITIVELY gone. The check is asymmetric on purpose:
///    only a confirmed `NotFound` drops the row. A permission error or
///    any other IO doubt keeps it, because silently thinning the list on
///    a transient failure looks identical to "you have no sessions".
/// 3. a titleless transcript too small to hold a turn — a conversation
///    abandoned before it started.
///
/// The command layer re-applies the same rules over the assembled picker
/// page; the predicate is idempotent, so running it twice is harmless.
fn is_offerable_in(session: &ExternalSession, temp_dir: &Path) -> bool {
    let cwd = Path::new(&session.cwd);
    if cwd.starts_with(temp_dir) {
        return false;
    }
    if cwd_confirmed_missing(cwd) {
        return false;
    }
    if session.title_source == ExternalSessionTitleSource::Fallback
        && session.file_size < EXTERNAL_SESSION_MIN_BYTES
    {
        return false;
    }
    true
}

/// POSITIVE-CONFIRMATION absence check: `true` only when the filesystem
/// says the path is not there. Anything else — including an unreadable
/// parent — is treated as "still might exist" and keeps the session.
fn cwd_confirmed_missing(cwd: &Path) -> bool {
    match std::fs::symlink_metadata(cwd) {
        Ok(_) => false,
        Err(e) => e.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn session(cwd: &str) -> ExternalSession {
        ExternalSession {
            session_id: "0f9a1c2d-3e4f-5a6b-7c8d-9e0f1a2b3c4d".into(),
            title: "Fix the resume cursor".into(),
            cwd: cwd.into(),
            git_branch: Some("main".into()),
            last_modified: "2026-08-28T10:11:12.000Z".into(),
            created_at: Some("2026-08-27T09:00:00.000Z".into()),
            file_size: 48_000,
            title_source: ExternalSessionTitleSource::Summary,
        }
    }

    /// A path that is guaranteed absent but is NOT under the temp dir,
    /// so the dead-cwd rule is exercised on its own.
    fn missing_dir() -> PathBuf {
        std::env::current_dir()
            .expect("cwd")
            .join("codemux-external-session-absent-fixture")
    }

    #[test]
    fn keeps_a_normal_session_in_an_existing_directory() {
        let here = std::env::current_dir().expect("cwd");
        let s = session(&here.to_string_lossy());
        assert!(is_offerable_in(&s, Path::new("/nonexistent-temp-root")));
    }

    #[test]
    fn drops_sessions_under_the_temp_directory() {
        let temp = std::env::temp_dir();
        let fixture = temp.join("pytest-of-someone").join("test_run0");
        let s = session(&fixture.to_string_lossy());
        assert!(!is_offerable_in(&s, &temp));
    }

    #[test]
    fn drops_sessions_whose_directory_is_confirmed_gone() {
        let s = session(&missing_dir().to_string_lossy());
        assert!(!is_offerable_in(&s, Path::new("/nonexistent-temp-root")));
    }

    #[test]
    fn drops_titleless_stubs_below_the_size_floor() {
        let here = std::env::current_dir().expect("cwd");
        let mut s = session(&here.to_string_lossy());
        s.title_source = ExternalSessionTitleSource::Fallback;
        s.file_size = 120;
        assert!(!is_offerable_in(&s, Path::new("/nonexistent-temp-root")));
    }

    #[test]
    fn keeps_a_titleless_session_that_actually_has_content() {
        // No title is not itself disqualifying — only a tiny transcript
        // with no title is.
        let here = std::env::current_dir().expect("cwd");
        let mut s = session(&here.to_string_lossy());
        s.title_source = ExternalSessionTitleSource::Fallback;
        s.file_size = EXTERNAL_SESSION_MIN_BYTES;
        assert!(is_offerable_in(&s, Path::new("/nonexistent-temp-root")));
    }

    #[test]
    fn keeps_a_small_session_that_has_a_real_title() {
        let here = std::env::current_dir().expect("cwd");
        let mut s = session(&here.to_string_lossy());
        s.file_size = 10;
        assert!(is_offerable_in(&s, Path::new("/nonexistent-temp-root")));
    }

    #[test]
    fn decodes_a_representative_response_and_drops_unusable_rows() {
        let here = std::env::current_dir()
            .expect("cwd")
            .to_string_lossy()
            .to_string();
        let parsed: ListSessionsResponse = serde_json::from_value(json!({
            "sessions": [
                {
                    "sessionId": "aaaaaaaa-1111-2222-3333-444444444444",
                    "title": "Wire up the picker",
                    "cwd": here,
                    "gitBranch": "resume-session",
                    "lastModified": "2026-08-28T10:11:12.000Z",
                    "createdAt": "2026-08-27T09:00:00.000Z",
                    "fileSize": 91_234,
                    "titleSource": "custom"
                },
                {
                    // Minimal row: every optional field absent.
                    "sessionId": "bbbbbbbb-1111-2222-3333-444444444444",
                    "cwd": here,
                    "fileSize": 91_234
                },
                {
                    // No cwd at all — not adoptable, dropped here.
                    "sessionId": "cccccccc-1111-2222-3333-444444444444",
                    "title": "Homeless",
                    "cwd": ""
                }
            ],
            "skippedWithoutCwd": 2
        }))
        .expect("decodes");

        assert_eq!(parsed.skipped_without_cwd, 2);
        let out = adoptable_from_response(parsed.sessions);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].title, "Wire up the picker");
        assert_eq!(out[0].title_source, ExternalSessionTitleSource::Custom);
        assert_eq!(out[0].git_branch.as_deref(), Some("resume-session"));
        // Minimal row: synthesised title, no branch, fallback source.
        assert!(out[1].title.contains("bbbbbbbb"));
        assert_eq!(out[1].title_source, ExternalSessionTitleSource::Fallback);
        assert_eq!(out[1].git_branch, None);
        assert_eq!(out[1].created_at, None);
    }

    #[test]
    fn decode_tolerates_an_unknown_extra_field() {
        let parsed: ListSessionsResponse = serde_json::from_value(json!({
            "sessions": [{
                "sessionId": "dddddddd-1111-2222-3333-444444444444",
                "title": "Future SDK",
                "cwd": "/some/where",
                "tag": "something-new"
            }],
            "unexpected": true
        }))
        .expect("decodes");
        assert_eq!(parsed.sessions.len(), 1);
    }
}
