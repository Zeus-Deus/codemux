//! Recover image copies already named by older assistant replies.
//!
//! Agents hand visual proof back as Markdown links to files they wrote under
//! temp directories (`[Mockup](/tmp/shots/f1.png)`). A path alone cannot prove
//! that a source is on a local volume: following a mapped drive or symlink in
//! a background copy could contact a network share without the reader's
//! knowledge. Existing copies may be recovered, but new unverified sources
//! are never opened by these background jobs.
//!
//! Old copy jobs record copies that already landed before a quit and retire
//! themselves; a fork inherits jobs of replies it copies. New replies do not
//! plan copies. Original paths load only after an explicit action in the chat UI.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::agent_chat::{chat_images_root, resolve_within};
#[cfg(test)]
use super::agent_chat::read_bounded_local_image;
use crate::database::DatabaseStore;

/// Announces the copies a reply job recorded (their paths), so a card that
/// gave up while its copy was still landing can load it.
pub const REPLY_IMAGES_KEPT_EVENT: &str = "agent_chat_reply_images_kept";

/// Most bytes one reply keeps, across all of its images.
const MAX_RETAINED_BYTES_PER_REPLY: usize = 64 * 1024 * 1024;
const UNVERIFIED_SOURCE: &str = "unverified image source requires explicit loading";
/// Bound the number of legacy copy jobs settling at once.
static COPY_PERMITS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

/// An image an assistant reply linked to, and the conversation's own copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetainedReplyImage {
    /// The local path as the reply wrote it (decoded), which the frontend
    /// matches the rendered link against.
    pub source: String,
    /// The copy under the thread's chat-image folder.
    pub path: String,
}

/// Settle reply `row_id`'s stored images, recording only copies that already
/// landed and retiring the job without opening unverified source paths.
pub(super) fn spawn_copies<R: Runtime>(app: &AppHandle<R>, row_id: i64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { run_job(&app, row_id).await });
}

/// Resume every reply whose copy records were not finalized before a quit,
/// including a fork's inherited replies.
pub fn resume_pending<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    for row_id in db.list_reply_image_jobs(None) {
        spawn_copies(app, row_id);
    }
}

/// Settle `thread_id`'s pending records before it is forked, so the fork
/// inherits every already-kept image the source has.
pub(super) async fn settle_thread<R: Runtime>(app: &AppHandle<R>, thread_id: &str) {
    let rows = app.state::<DatabaseStore>().list_reply_image_jobs(Some(thread_id));
    for row_id in rows {
        run_job(app, row_id).await;
    }
}

/// Drop `thread_id`'s pending legacy jobs before it is deleted, and wait out
/// any job in progress before the chat's images are removed.
pub(super) async fn abandon_thread<R: Runtime>(app: &AppHandle<R>, thread_id: &str) {
    let rows = app.state::<DatabaseStore>().take_reply_image_jobs(thread_id);
    for row_id in rows {
        let lock = row_lock(row_id);
        drop(lock.lock().await);
        drop(lock);
        forget_row_lock(row_id);
    }
}

/// One job at a time per row: a resumed, inherited or settled run waits for
/// the one in progress and then finds the job retired.
fn row_lock(row_id: i64) -> Arc<tokio::sync::Mutex<()>> {
    row_locks().lock().unwrap().entry(row_id).or_default().clone()
}

/// Drop row `row_id`'s lock once nobody holds or awaits it, so a later run
/// can never get a second lock while an earlier one is still in use.
fn forget_row_lock(row_id: i64) {
    let mut locks = row_locks().lock().unwrap();
    if locks.get(&row_id).is_some_and(|lock| Arc::strong_count(lock) == 1) {
        locks.remove(&row_id);
    }
}

fn row_locks() -> &'static Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

async fn run_job<R: Runtime>(app: &AppHandle<R>, row_id: i64) {
    let lock = row_lock(row_id);
    {
        let _guard = lock.lock().await;
        // Taken before the row is read, so queued jobs hold no payloads.
        if let Ok(_permit) = COPY_PERMITS.acquire().await {
            let db = app.state::<DatabaseStore>();
            // Retired by another run, or abandoned with its chat (a gone row
            // took its job with it).
            if db.has_reply_image_job(row_id) {
                if let Some(payload) = db.get_agent_chat_message_payload(row_id) {
                    retain_row(app, &db, row_id, planned_in_payload(&payload)).await;
                }
            }
        }
    }
    drop(lock);
    forget_row_lock(row_id);
}

async fn retain_row<R: Runtime>(
    app: &AppHandle<R>,
    db: &DatabaseStore,
    row_id: i64,
    planned: Vec<RetainedReplyImage>,
) {
    let Some(root) = chat_images_root() else {
        return;
    };
    let canonical_root = match tokio::fs::create_dir_all(&root).await {
        Ok(()) => tokio::fs::canonicalize(&root).await.ok(),
        Err(_) => None,
    };
    let Some(canonical_root) = canonical_root else {
        return;
    };
    let mut budget = MAX_RETAINED_BYTES_PER_REPLY;
    let mut kept = Vec::new();
    for image in planned {
        match keep_one(&root, &canonical_root, &image, &mut budget).await {
            Ok(()) => kept.push(image),
            Err(error) if error == UNVERIFIED_SOURCE => {}
            Err(error) => eprintln!(
                "[codemux::reply_images] not keeping {}: {error}",
                image.source
            ),
        }
    }
    match db.finish_reply_image_job(row_id, &kept) {
        Ok(true) if !kept.is_empty() => {
            let paths: Vec<&str> = kept.iter().map(|image| image.path.as_str()).collect();
            let _ = app.emit(REPLY_IMAGES_KEPT_EVENT, paths);
        }
        Ok(true) => {}
        Ok(false) => remove_copies(&root, &kept),
        // The job stays queued for the next launch.
        Err(error) => eprintln!("[codemux::reply_images] couldn't record kept images: {error}"),
    }
}

/// The copies a stored reply names.
fn planned_in_payload(payload: &str) -> Vec<RetainedReplyImage> {
    serde_json::from_str::<serde_json::Value>(payload)
        .ok()
        .and_then(|value| value.pointer("/item/retained_images").cloned())
        .and_then(|images| serde_json::from_value(images).ok())
        .unwrap_or_default()
}

/// Record a copy that already landed before a quit. New agent-authored source
/// paths are never read in the background because path spelling cannot prove
/// that a drive, mount or symlink is local.
///
/// The destination comes from a stored row, so it must be exactly
/// `<root>/<folder>/<stem>.<ext>` with `<folder>` a real directory inside
/// the root, not a link out of it. The copy lands at that path or not at
/// all, so a live reply and the stored row always name the same file.
async fn keep_one(
    root: &std::path::Path,
    canonical_root: &std::path::Path,
    image: &RetainedReplyImage,
    budget: &mut usize,
) -> Result<(), String> {
    let (folder, _, _) =
        copy_target(root, &image.path).ok_or_else(|| "not a chat-image copy path".to_string())?;
    let dir = root.join(&folder);
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|error| format!("couldn't create its folder: {error}"))?;
    let canonical_dir = tokio::fs::canonicalize(&dir)
        .await
        .map_err(|error| format!("couldn't resolve its folder: {error}"))?;
    if canonical_dir.parent() != Some(canonical_root) {
        return Err("its folder leads outside the chat-image folder".to_string());
    }
    if let Ok(existing) = tokio::fs::symlink_metadata(&image.path).await {
        if !existing.is_file() {
            return Err("its destination is not a file".to_string());
        }
        *budget = budget.saturating_sub(existing.len() as usize);
        return Ok(());
    }
    Err(UNVERIFIED_SOURCE.to_string())
}

/// Split a planned copy path into its chat-image folder, file stem and
/// extension, or `None` when it is not a plain file directly inside a
/// folder of `root`.
fn copy_target(root: &std::path::Path, path: &str) -> Option<(String, String, String)> {
    let path = std::path::Path::new(path);
    let folder = path.parent()?.file_name()?.to_str()?;
    let stem = path.file_stem()?.to_str()?;
    let extension = path.extension()?.to_str()?;
    let expected = root.join(folder).join(format!("{stem}.{extension}"));
    (expected == path).then(|| (folder.to_string(), stem.to_string(), extension.to_string()))
}

/// Remove copies whose reply is gone. Each must resolve inside the root.
fn remove_copies(root: &std::path::Path, kept: &[RetainedReplyImage]) {
    let canonical_root = root.canonicalize().ok();
    for image in kept {
        let Ok(path) = resolve_within(root, &image.path) else {
            continue;
        };
        let _ = std::fs::remove_file(&path);
        // Fails, harmlessly, while the folder still holds files.
        if let Some(dir) = path.parent().filter(|dir| Some(*dir) != canonical_root.as_deref()) {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn network_image_paths_are_rejected_before_reading() {
        for path in ["//server/share/a.png", r"\\server\share\a.png", r"\\?\C:\a.png", r"\??\UNC\server\a.png"] {
            let error = read_bounded_local_image(path).await.unwrap_err();
            assert!(error.contains("network or device"), "{path}: {error}");
        }
    }

    #[test]
    fn a_reply_job_records_only_copies_that_landed_and_dies_with_its_row() {
        let db = DatabaseStore::new_in_memory();
        db.upsert_agent_chat_session("t", "ws", Some("/repo"), "claude").unwrap();
        let planned = |path: &str| RetainedReplyImage {
            source: "/tmp/a.png".into(),
            path: path.into(),
        };
        let payload = serde_json::json!({
            "type": "item_completed", "thread_id": "t", "turn_id": "u",
            "item": {"kind": "assistant_text", "text": "[a](/tmp/a.png)",
                     "retained_images": [planned("/img/t/k.png"), planned("/img/t/l.png")]},
        });
        let row = db
            .append_agent_chat_reply_with_image_job("t", &payload.to_string())
            .unwrap()
            .unwrap();
        // The row and its job commit together; a restart finds the job.
        assert_eq!(db.list_reply_image_jobs(None), vec![row]);
        let stored = db.get_agent_chat_message_payload(row).unwrap();
        assert_eq!(planned_in_payload(&stored).len(), 2);

        // One copy landed: the row names only it, and the job is retired.
        assert!(db.finish_reply_image_job(row, &[planned("/img/t/l.png")]).unwrap());
        let item = |db: &DatabaseStore| {
            let rows = db.list_agent_chat_messages("t");
            serde_json::from_str::<serde_json::Value>(&rows[0]).unwrap()["item"].clone()
        };
        assert_eq!(item(&db)["retained_images"], serde_json::json!([planned("/img/t/l.png")]));
        assert!(db.list_reply_image_jobs(None).is_empty());
        // None landed: the reply claims no copy at all, its text untouched.
        assert!(db.finish_reply_image_job::<RetainedReplyImage>(row, &[]).unwrap());
        assert!(item(&db).get("retained_images").is_none());
        assert_eq!(item(&db)["text"], "[a](/tmp/a.png)");

        // A deleted session takes a pending job with it, and a late finish
        // learns its copies are unowned.
        let again = db
            .append_agent_chat_reply_with_image_job("t", &payload.to_string())
            .unwrap()
            .unwrap();
        db.delete_agent_chat_session("t").unwrap();
        assert!(db.list_reply_image_jobs(None).is_empty());
        assert!(db.get_agent_chat_message_payload(again).is_none());
        assert!(!db.finish_reply_image_job::<RetainedReplyImage>(again, &[]).unwrap());
    }

    #[test]
    fn a_row_keeps_one_lock_while_anyone_holds_or_awaits_it() {
        // Run A holds the lock while run B waits on it.
        let row = -7001;
        let a = row_lock(row);
        let b = row_lock(row);
        // A finishes; its cleanup must not drop the lock B is waiting on.
        drop(a);
        forget_row_lock(row);
        let c = row_lock(row);
        assert!(Arc::ptr_eq(&b, &c), "a later run C shares B's lock");
        // Once nobody uses it, the entry goes.
        drop(b);
        drop(c);
        forget_row_lock(row);
        assert!(!row_locks().lock().unwrap().contains_key(&row));
    }

    #[test]
    fn copies_land_only_at_plain_paths_inside_the_root() {
        let root = std::path::Path::new("/cfg/images");
        assert_eq!(
            copy_target(root, "/cfg/images/t1/k.png"),
            Some(("t1".into(), "k".into(), "png".into()))
        );
        assert_eq!(copy_target(root, "/cfg/images/k.png"), None, "no thread folder");
        assert_eq!(copy_target(root, "/cfg/images/t1/sub/k.png"), None);
        assert_eq!(copy_target(root, "/cfg/images/../t1/k.png"), None);
        assert_eq!(copy_target(root, "/elsewhere/t1/k.png"), None);
        assert_eq!(copy_target(root, "/cfg/images/t1/k"), None, "no extension");
    }

    #[tokio::test]
    async fn a_resumed_reply_charges_existing_copies_without_reading_new_sources() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let folder = root.path().join("t");
        std::fs::create_dir_all(&folder).unwrap();
        // A copy the first run landed before the quit.
        let landed = std::fs::File::create(folder.join("landed.png")).unwrap();
        landed.set_len((MAX_RETAINED_BYTES_PER_REPLY - 1024 * 1024) as u64).unwrap();
        // A source the resumed run has not copied yet must not be opened.
        let sources = tempfile::tempdir().unwrap();
        let source = sources.path().join("big.png");
        std::fs::write(&source, b"\x89PNG\r\n\x1a\n").unwrap();
        std::fs::OpenOptions::new().write(true).open(&source).unwrap().set_len(2 * 1024 * 1024).unwrap();
        let image = |name: &str, source: &std::path::Path| RetainedReplyImage {
            source: source.to_string_lossy().into_owned(),
            path: folder.join(name).to_string_lossy().into_owned(),
        };

        let mut budget = MAX_RETAINED_BYTES_PER_REPLY;
        keep_one(root.path(), &canonical, &image("landed.png", &source), &mut budget)
            .await
            .unwrap();
        assert_eq!(budget, 1024 * 1024);
        let refused = keep_one(root.path(), &canonical, &image("next.png", &source), &mut budget).await;
        assert_eq!(refused.unwrap_err(), UNVERIFIED_SOURCE);
        assert!(!folder.join("next.png").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_copy_never_lands_through_a_folder_linked_out_of_the_root() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("k.png"), b"not mine").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("evil")).unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let image = RetainedReplyImage {
            source: "/tmp/a.png".into(),
            path: root.path().join("evil/k.png").to_string_lossy().into_owned(),
        };

        let mut budget = MAX_RETAINED_BYTES_PER_REPLY;
        let refused = keep_one(root.path(), &canonical, &image, &mut budget).await;
        assert!(refused.unwrap_err().contains("outside"));
        // Nor does cleanup follow the link.
        remove_copies(root.path(), &[image]);
        assert!(outside.path().join("k.png").exists());
    }
}
