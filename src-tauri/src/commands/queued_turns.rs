//! Queue editing, and the saved queue that survives a restart.
//!
//! While the app runs, each provider holds its own follow-up queue; edits
//! and moves go to it ([`AgentProvider::edit_queued_turn`],
//! [`AgentProvider::move_queued_turn`]) and come back as events.
//! [`on_event`] mirrors every queue event into `agent_chat_queued_turns`
//! (see [`crate::database::queued_turns`]) from `forward_event`, so the
//! table always holds what is still queued.
//!
//! After a restart no provider holds those turns any more. Rows from an
//! earlier run are *saved*: the pane shows them as queued, and the user can
//! edit, move or remove them, or send one through the normal send path with
//! any delivery ([`agent_chat_send_saved_queued_turn`]). Nothing sends a
//! saved turn on its own; resuming a saved queue automatically is left to a
//! caller of that command.
//!
//! A saved row stays put until its send is accepted, so quitting mid-send
//! loses nothing, and a claim keeps it from being sent twice. The resend
//! keeps the row's nonce: if it queues, the row is replaced by one for the
//! new live turn in a single transaction; if it reached the transcript
//! before a crash, listing the queue sees the nonce there and drops the row.
//!
//! Quitting the app closes sessions, and a closing session cancels its
//! queue. [`mark_exiting`] stops those cancellations from deleting rows.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use super::agent_chat::{
    feature_flag_on, forward_event, lookup_provider, provider_err, send_turn_with_origin,
    ChatImageRef, ProviderRegistry, SendTurnCommandInput, TurnOrigin,
};
use crate::agent_provider::queue_edit;
use crate::agent_provider::types::MessageDelivery;
use crate::agent_provider::{ProviderKind, ProviderRuntimeEvent, ThreadId, TurnStartResult};
use crate::database::queued_turns::{QueuedTurnImage, QueuedTurnRecord};
use crate::database::DatabaseStore;
use crate::observability::ObservabilityStore;

static EXITING: AtomicBool = AtomicBool::new(false);

/// The app is quitting: keep the saved queue as it is.
pub fn mark_exiting() {
    EXITING.store(true, Ordering::SeqCst);
}

pub(crate) fn exiting() -> bool {
    EXITING.load(Ordering::SeqCst)
}

/// This app run. Rows written under another epoch are saved, not live.
fn epoch() -> &'static str {
    static EPOCH: OnceLock<String> = OnceLock::new();
    EPOCH.get_or_init(|| uuid::Uuid::new_v4().to_string())
}

/// What a queued send carries besides its text, stashed by the send path
/// until `TurnQueued` writes the row: the send command's own input, so a
/// resend renders skills the same way.
#[derive(Debug, Clone, Default)]
pub(crate) struct Replay {
    pub prompt: String,
    pub skill_ids: Vec<String>,
    pub skill_text: Option<String>,
    pub include_plugins: bool,
    pub model_override: Option<String>,
    pub effort_override: Option<String>,
    pub permission_mode_override: Option<String>,
}

/// What `forward_event` hands over for a `TurnQueued`.
pub(crate) struct Queued<'a> {
    pub images: Vec<QueuedTurnImage>,
    pub client_nonce: Option<&'a str>,
    pub replay: Option<&'a Replay>,
}

/// Mirror one queue event into the saved queue. `queued` carries the
/// stashed send for a `TurnQueued`. Best-effort: a failed write only
/// affects what a restart can restore.
pub(crate) fn on_event(db: &DatabaseStore, event: &ProviderRuntimeEvent, queued: Option<Queued<'_>>) {
    mirror(db, event, queued, exiting());
}

fn mirror(db: &DatabaseStore, event: &ProviderRuntimeEvent, queued: Option<Queued<'_>>, exiting: bool) {
    let result = match event {
        ProviderRuntimeEvent::TurnQueued {
            thread_id,
            queued_id,
            client_nonce,
            text,
        } => {
            let queued = queued.unwrap_or(Queued {
                images: Vec::new(),
                client_nonce: None,
                replay: None,
            });
            let replay = queued.replay;
            db.insert_queued_turn(&QueuedTurnRecord {
                thread_id: thread_id.0.clone(),
                queued_id: queued_id.clone(),
                position: 0,
                text: text.clone(),
                prompt: replay.map(|r| r.prompt.clone()),
                skill_ids: replay.map(|r| r.skill_ids.clone()).unwrap_or_default(),
                skill_text: replay.and_then(|r| r.skill_text.clone()),
                include_plugins: replay.is_none_or(|r| r.include_plugins),
                images: queued.images,
                client_nonce: client_nonce.clone().or(queued.client_nonce.map(Into::into)),
                model_override: replay.and_then(|r| r.model_override.clone()),
                effort_override: replay.and_then(|r| r.effort_override.clone()),
                permission_mode_override: replay.and_then(|r| r.permission_mode_override.clone()),
                epoch: epoch().into(),
            })
            .map(drop)
        }
        ProviderRuntimeEvent::QueuedTurnDispatched {
            thread_id,
            queued_id,
            ..
        } => db.delete_queued_turn(&thread_id.0, queued_id).map(drop),
        ProviderRuntimeEvent::QueuedTurnCancelled {
            thread_id,
            queued_id,
        } => {
            if exiting {
                return;
            }
            // A live turn's images are cleaned up by `forward_event`; a saved
            // one's only this row knows about.
            db.take_queued_turn(&thread_id.0, queued_id).map(|row| {
                for image in row.filter(|row| row.epoch != epoch()).into_iter().flat_map(|r| r.images) {
                    let _ = std::fs::remove_file(image.path);
                }
            })
        }
        ProviderRuntimeEvent::QueuedTurnEdited {
            thread_id,
            queued_id,
            text,
        } => match db.get_queued_turn(&thread_id.0, queued_id) {
            Some(row) => {
                let prompt = row.prompt.as_deref().map(|prompt| {
                    queue_edit::splice_prompt(prompt, &row.text, text)
                        .unwrap_or_else(|| prompt.to_owned())
                });
                db.update_queued_turn_text(&thread_id.0, queued_id, text, prompt.as_deref())
                    .map(drop)
            }
            None => Ok(()),
        },
        ProviderRuntimeEvent::QueuedTurnsReordered {
            thread_id,
            queued_ids,
        } => db.reorder_queued_turns(&thread_id.0, queued_ids),
        _ => return,
    };
    if let Err(error) = result {
        eprintln!("[codemux::queued_turns] failed to save the queue: {error}");
    }
}

/// Tells a pane its thread's saved queue changed behind its back (rows
/// rehomed by a merge), so it lists the queue again. Payload: the thread id.
pub const QUEUED_TURNS_CHANGED_EVENT: &str = "agent-chat-queued-turns-changed";

/// After duplicate sessions collapse: move rehomed rows' images, then tell
/// panes to list their saved queue again. A pane may already have listed it
/// before the merge moved the rows onto its thread.
pub(crate) fn after_sessions_merged<R: Runtime>(
    app: &AppHandle<R>,
    db: &DatabaseStore,
    dir_of: impl Fn(&str) -> Option<std::path::PathBuf>,
) {
    adopt_moved_images(db, dir_of);
    let mut threads: Vec<String> = db.all_queued_turns().into_iter().map(|row| row.thread_id).collect();
    threads.dedup();
    for thread in threads {
        let _ = app.emit(QUEUED_TURNS_CHANGED_EVENT, thread);
    }
}

/// Move images of rows that changed thread (a merged history resume) into
/// their thread's image dir, before the merged-away dir is deleted: a resend
/// only accepts images from its own thread. `dir_of` is the chat image dir
/// for a thread.
pub(crate) fn adopt_moved_images(
    db: &DatabaseStore,
    dir_of: impl Fn(&str) -> Option<std::path::PathBuf>,
) {
    for row in db.all_queued_turns() {
        let Some(dir) = dir_of(&row.thread_id) else { continue };
        let outside = |image: &QueuedTurnImage| std::path::Path::new(&image.path).parent() != Some(dir.as_path());
        if !row.images.iter().any(outside) {
            continue;
        }
        let _ = std::fs::create_dir_all(&dir);
        let images: Vec<QueuedTurnImage> = row
            .images
            .iter()
            .map(|image| {
                let from = std::path::Path::new(&image.path);
                let Some(name) = from.file_name().filter(|_| outside(image)) else {
                    return image.clone();
                };
                let to = dir.join(name);
                let moved = std::fs::rename(from, &to)
                    .or_else(|_| std::fs::copy(from, &to).map(drop));
                match moved {
                    Ok(()) => QueuedTurnImage {
                        path: to.to_string_lossy().into_owned(),
                        media_type: image.media_type.clone(),
                    },
                    Err(_) => image.clone(),
                }
            })
            .collect();
        if let Err(error) = db.set_queued_turn_images(&row.thread_id, &row.queued_id, &images) {
            eprintln!("[codemux::queued_turns] {error}");
        }
    }
}

/// A row this run did not queue: no provider holds it.
fn saved_row(db: &DatabaseStore, thread_id: &str, queued_id: &str) -> Option<QueuedTurnRecord> {
    db.get_queued_turn(thread_id, queued_id)
        .filter(|row| row.epoch != epoch())
}

/// Saved turns a send, edit, move or removal is working on. One claim per
/// row at a time, so a send never reads a row another change is rewriting.
fn sending() -> &'static Mutex<HashSet<(String, String)>> {
    static SENDING: OnceLock<Mutex<HashSet<(String, String)>>> = OnceLock::new();
    SENDING.get_or_init(Default::default)
}

const BEING_SENT: &str = "This message is being sent or changed. Try again in a moment.";

fn is_sending(thread_id: &str, queued_id: &str) -> bool {
    sending()
        .lock()
        .unwrap()
        .contains(&(thread_id.to_owned(), queued_id.to_owned()))
}

/// Run `op` on saved row `queued_id` while holding its claim, reading the
/// row only once the claim is held. `Ok(None)` when it is no longer saved.
fn under_claim<T>(
    db: &DatabaseStore,
    thread_id: &str,
    queued_id: &str,
    op: impl FnOnce(QueuedTurnRecord) -> Result<T, String>,
) -> Result<Option<T>, String> {
    let Some(_claim) = Claim::take(thread_id, queued_id) else {
        return Err(BEING_SENT.into());
    };
    match saved_row(db, thread_id, queued_id) {
        Some(row) => op(row).map(Some),
        None => Ok(None),
    }
}

/// Releases a claim however the work holding it ends.
struct Claim((String, String));

impl Claim {
    fn take(thread_id: &str, queued_id: &str) -> Option<Self> {
        let key = (thread_id.to_owned(), queued_id.to_owned());
        // Build the claim only once it is ours: dropping a spare one would
        // release the real claim (and relock this mutex).
        let claimed = sending().lock().unwrap().insert(key.clone());
        if claimed { Some(Self(key)) } else { None }
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        sending().lock().unwrap().remove(&self.0);
    }
}

/// The nonce a saved row's bubble and its send share. Older rows without
/// one use the queued id, which is unique too.
fn nonce(row: &QueuedTurnRecord) -> String {
    row.client_nonce.clone().unwrap_or_else(|| row.queued_id.clone())
}

#[derive(Debug, Clone, Serialize)]
pub struct QueuedTurnView {
    pub queued_id: String,
    pub text: String,
    /// Queued before this app run; see the module docs.
    pub saved: bool,
    pub client_nonce: String,
    pub images: Vec<QueuedTurnImage>,
}

/// A thread's queue, in dispatch order: saved turns, and the live turns this
/// run has queued.
#[tauri::command]
pub async fn agent_chat_list_queued_turns<R: Runtime>(
    app: AppHandle<R>,
    thread_id: ThreadId,
) -> Result<Vec<QueuedTurnView>, String> {
    let db: State<'_, DatabaseStore> = app.state();
    let mut rows = db.list_queued_turns(&thread_id.0);
    rows.retain(|row| {
        let saved = row.epoch != epoch();
        if saved && !is_sending(&row.thread_id, &row.queued_id) && db.queued_turn_was_sent(&row.thread_id, &nonce(row)) {
            // Sent before the app stopped, but the row outlived it.
            let _ = db.delete_queued_turn(&row.thread_id, &row.queued_id);
            return false;
        }
        true
    });
    Ok(rows
        .into_iter()
        .map(|row| QueuedTurnView {
            saved: row.epoch != epoch(),
            client_nonce: nonce(&row),
            queued_id: row.queued_id,
            text: row.text,
            images: row.images,
        })
        .collect())
}

/// Replace a queued turn's text. Returns whether it was still queued.
#[tauri::command]
pub async fn agent_chat_edit_queued_turn<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: ThreadId,
    queued_id: String,
    text: String,
) -> Result<bool, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let db: State<'_, DatabaseStore> = app.state();
    if saved_row(&db, &thread_id.0, &queued_id).is_none() {
        let registry: State<'_, ProviderRegistry> = app.state();
        return lookup_provider(&registry, provider)
            .await?
            .edit_queued_turn(thread_id, queued_id, text)
            .await
            .map_err(provider_err);
    }
    let edited = under_claim(&db, &thread_id.0, &queued_id, |row| {
        let text = text.trim().to_owned();
        if text.is_empty() && row.images.is_empty() {
            return Err("A queued message can't be empty. Cancel it instead.".into());
        }
        if let Some(prompt) = &row.prompt {
            if queue_edit::splice_prompt(prompt, &row.text, &text).is_none() {
                return Err(provider_err(queue_edit::cannot_edit()));
            }
        }
        forward_event(
            &app,
            ProviderRuntimeEvent::QueuedTurnEdited {
                thread_id: thread_id.clone(),
                queued_id: queued_id.clone(),
                text,
            },
        );
        Ok(())
    })?;
    Ok(edited.is_some())
}

/// Move `ids`' item `id` to just before `before`, or to the end.
fn move_id(ids: &mut Vec<String>, id: &str, before: Option<&str>) {
    let Some(from) = ids.iter().position(|x| x == id) else { return };
    if before == Some(id) {
        return;
    }
    let item = ids.remove(from);
    let to = match before {
        Some(before) => ids.iter().position(|x| x == before).unwrap_or(from),
        None => ids.len(),
    };
    ids.insert(to, item);
}

/// Move a queued turn to just before `before`, or to the back of its queue.
/// Saved and live turns are separate queues. Returns whether it was still
/// queued.
#[tauri::command]
pub async fn agent_chat_move_queued_turn<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: ThreadId,
    queued_id: String,
    before: Option<String>,
) -> Result<bool, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let db: State<'_, DatabaseStore> = app.state();
    if saved_row(&db, &thread_id.0, &queued_id).is_none() {
        let registry: State<'_, ProviderRegistry> = app.state();
        return lookup_provider(&registry, provider)
            .await?
            .move_queued_turn(thread_id, queued_id, before)
            .await
            .map_err(provider_err);
    }
    let moved = under_claim(&db, &thread_id.0, &queued_id, |_| {
        let mut queued_ids: Vec<String> = db
            .list_queued_turns(&thread_id.0)
            .into_iter()
            .filter(|row| row.epoch != epoch())
            .map(|row| row.queued_id)
            .collect();
        move_id(&mut queued_ids, &queued_id, before.as_deref());
        forward_event(
            &app,
            ProviderRuntimeEvent::QueuedTurnsReordered {
                thread_id: thread_id.clone(),
                queued_ids,
            },
        );
        Ok(())
    })?;
    Ok(moved.is_some())
}

/// Remove a saved turn. Like a live cancel, its text returns to the
/// composer. Returns whether it was still saved.
#[tauri::command]
pub async fn agent_chat_cancel_saved_queued_turn<R: Runtime>(
    app: AppHandle<R>,
    thread_id: ThreadId,
    queued_id: String,
) -> Result<bool, String> {
    let db: State<'_, DatabaseStore> = app.state();
    let cancelled = under_claim(&db, &thread_id.0, &queued_id, |_| {
        forward_event(
            &app,
            ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id: thread_id.clone(),
                queued_id: queued_id.clone(),
            },
        );
        Ok(())
    })?;
    Ok(cancelled.is_some())
}

/// Send a saved turn through the normal send path with `delivery`, as if
/// it had just been typed: it starts a turn, steers the running one, or
/// queues behind it. It keeps its nonce, so its bubble becomes the sent
/// one. The row goes once the send is accepted; on failure it stays saved
/// where it was.
#[tauri::command]
pub async fn agent_chat_send_saved_queued_turn<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: ThreadId,
    queued_id: String,
    delivery: MessageDelivery,
) -> Result<TurnStartResult, String> {
    let db: State<'_, DatabaseStore> = app.state();
    // Claim first, then read: a send that finished meanwhile has removed
    // or replaced the row.
    let Some(_claim) = Claim::take(&thread_id.0, &queued_id) else {
        return Err(BEING_SENT.into());
    };
    let Some(row) = saved_row(&db, &thread_id.0, &queued_id) else {
        return Err("This message is no longer saved.".into());
    };
    let input = SendTurnCommandInput {
        delivery,
        thread_id: thread_id.clone(),
        text: row.prompt.clone().unwrap_or_else(|| row.text.clone()),
        display_text: Some(row.text.clone()),
        skill_ids: row.skill_ids.clone(),
        skill_text: row.skill_text.clone(),
        include_plugins: row.include_plugins,
        images: row
            .images
            .iter()
            .map(|image| ChatImageRef {
                path: image.path.clone(),
                media_type: image.media_type.clone(),
            })
            .collect(),
        model_override: row.model_override.clone(),
        effort_override: row.effort_override.clone(),
        permission_mode_override: row.permission_mode_override.clone(),
        client_nonce: Some(nonce(&row)),
    };
    let result = send_turn_with_origin(app.clone(), provider, input, TurnOrigin::User).await?;
    // Accepted. A started or steered turn is already in the transcript, so
    // the row can go. A queued one is now a live turn: save it under its new
    // id in the same step that drops this row, before the claim is released,
    // so it is never saved twice nor lost. Its `TurnQueued`, mirrored before
    // or after this, finds it already saved.
    let saved = match &result.queued_id {
        None => db.delete_queued_turn(&thread_id.0, &queued_id).map(drop),
        Some(new_id) => db
            .replace_queued_turn(
                Some(&queued_id),
                &QueuedTurnRecord {
                    queued_id: new_id.clone(),
                    client_nonce: Some(nonce(&row)),
                    epoch: epoch().into(),
                    ..row
                },
            )
            .map(drop),
    };
    if let Err(error) = saved {
        eprintln!("[codemux::queued_turns] failed to settle a sent saved turn: {error}");
    }
    let resolved = ProviderRuntimeEvent::QueuedTurnCancelled {
        thread_id,
        queued_id,
    };
    if let Err(error) = db.control_record_queue_event(&resolved) {
        eprintln!("[codemux::queued_turns] failed to journal a sent saved turn: {error}");
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::TurnId;

    fn t() -> ThreadId {
        ThreadId("t".into())
    }

    fn queued(db: &DatabaseStore, id: &str, text: &str, prompt: &str) {
        let replay = Replay {
            prompt: prompt.into(),
            skill_ids: vec!["codex-skill".into()],
            effort_override: Some("high".into()),
            ..Default::default()
        };
        let event = ProviderRuntimeEvent::TurnQueued {
            thread_id: t(),
            queued_id: id.into(),
            client_nonce: Some(format!("nonce-{id}")),
            text: text.into(),
        };
        let stash = Queued {
            images: Vec::new(),
            client_nonce: None,
            replay: Some(&replay),
        };
        mirror(db, &event, Some(stash), false);
    }

    fn ids(db: &DatabaseStore) -> Vec<String> {
        db.list_queued_turns("t").into_iter().map(|r| r.queued_id).collect()
    }

    #[test]
    fn the_saved_queue_mirrors_queue_events() {
        let db = crate::database::init_test_database();
        db.upsert_agent_chat_session("t", "w", None, "claude").unwrap();
        queued(&db, "a", "fix it", "<mode/>\n\nfix it");
        queued(&db, "b", "then test", "then test");
        queued(&db, "c", "ship", "ship");
        let a = db.get_queued_turn("t", "a").unwrap();
        assert_eq!(a.client_nonce.as_deref(), Some("nonce-a"));
        assert_eq!(a.effort_override.as_deref(), Some("high"));
        assert_eq!(a.skill_ids, ["codex-skill"]);
        assert_eq!(a.epoch, epoch());
        assert!(saved_row(&db, "t", "a").is_none(), "this run's rows are live");

        let edited = ProviderRuntimeEvent::QueuedTurnEdited {
            thread_id: t(),
            queued_id: "a".into(),
            text: "fix all".into(),
        };
        mirror(&db, &edited, None, false);
        let a = db.get_queued_turn("t", "a").unwrap();
        assert_eq!((a.text.as_str(), a.prompt.as_deref()), ("fix all", Some("<mode/>\n\nfix all")));

        let order = vec!["c".to_string(), "a".into(), "b".into()];
        mirror(&db, &ProviderRuntimeEvent::QueuedTurnsReordered { thread_id: t(), queued_ids: order.clone() }, None, false);
        assert_eq!(ids(&db), order);

        let dispatched = ProviderRuntimeEvent::QueuedTurnDispatched {
            steered: false,
            thread_id: t(),
            queued_id: "c".into(),
            turn_id: TurnId("turn".into()),
            text: "ship".into(),
        };
        mirror(&db, &dispatched, None, false);
        let cancelled = |id: &str| ProviderRuntimeEvent::QueuedTurnCancelled { thread_id: t(), queued_id: id.into() };
        // Quitting closes sessions; their cancellations keep the rows.
        mirror(&db, &cancelled("a"), None, true);
        assert_eq!(ids(&db), ["a", "b"]);
        mirror(&db, &cancelled("a"), None, false);
        assert_eq!(ids(&db), ["b"]);
    }

    #[test]
    fn rows_from_an_earlier_run_are_saved() {
        let db = crate::database::init_test_database();
        db.upsert_agent_chat_session("t", "w", None, "claude").unwrap();
        let mut row = QueuedTurnRecord {
            thread_id: "t".into(),
            queued_id: "old".into(),
            position: 0,
            text: "from before".into(),
            prompt: None,
            images: Vec::new(),
            client_nonce: None,
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            epoch: "earlier-run".into(),
            skill_ids: Vec::new(),
            skill_text: None,
            include_plugins: true,
        };
        db.insert_queued_turn(&row).unwrap();
        let saved = saved_row(&db, "t", "old").unwrap();
        // A row without a nonce shares its queued id with its bubble.
        assert_eq!(nonce(&saved), "old");
        // One send at a time claims it.
        let claim = Claim::take("t", "old").unwrap();
        assert!(is_sending("t", "old") && Claim::take("t", "old").is_none());
        // An edit, move or removal waits out a send in flight rather than
        // changing a row the send has already read.
        assert_eq!(under_claim(&db, "t", "old", |_| Ok(())), Err(BEING_SENT.to_string()));
        drop(claim);
        assert!(!is_sending("t", "old"));
        // With the claim free the change runs on the row it re-reads, and
        // holds the claim while it does.
        let ran = under_claim(&db, "t", "old", |row| {
            assert!(is_sending("t", "old"));
            Ok(row.queued_id)
        });
        assert_eq!(ran, Ok(Some("old".to_string())));
        assert!(!is_sending("t", "old"));
        db.delete_queued_turn("t", "old").unwrap();
        assert_eq!(under_claim(&db, "t", "old", |_| Ok(())), Ok(None));
        row.client_nonce = Some("n".into());
        assert_eq!(nonce(&row), "n");
    }

    #[test]
    fn adopted_images_move_into_their_new_thread() {
        let db = crate::database::init_test_database();
        db.upsert_agent_chat_session("new", "w", None, "claude").unwrap();
        let base = tempfile::tempdir().unwrap();
        let old_dir = base.path().join("old");
        std::fs::create_dir_all(&old_dir).unwrap();
        let old = old_dir.join("a.png");
        std::fs::write(&old, b"png").unwrap();
        let image = QueuedTurnImage { path: old.to_string_lossy().into_owned(), media_type: "image/png".into() };
        db.insert_queued_turn(&QueuedTurnRecord {
            thread_id: "new".into(),
            queued_id: "q".into(),
            position: 0,
            text: "t".into(),
            prompt: None,
            images: vec![image],
            client_nonce: None,
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            epoch: "earlier-run".into(),
            skill_ids: Vec::new(),
            skill_text: None,
            include_plugins: true,
        })
        .unwrap();
        adopt_moved_images(&db, |thread| Some(base.path().join(thread)));
        let moved = base.path().join("new").join("a.png");
        assert!(moved.is_file() && !old.exists());
        assert_eq!(db.get_queued_turn("new", "q").unwrap().images[0].path, moved.to_string_lossy());
    }

    #[test]
    fn move_id_places_before_the_target_or_last() {
        let mut ids: Vec<String> = ["a", "b", "c"].map(String::from).into();
        move_id(&mut ids, "c", Some("a"));
        assert_eq!(ids, ["c", "a", "b"]);
        move_id(&mut ids, "c", None);
        assert_eq!(ids, ["a", "b", "c"]);
        move_id(&mut ids, "a", Some("gone"));
        assert_eq!(ids, ["a", "b", "c"]);
        move_id(&mut ids, "gone", None);
        assert_eq!(ids, ["a", "b", "c"]);
    }
}
