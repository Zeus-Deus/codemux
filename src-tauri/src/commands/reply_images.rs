//! Keep the local images an assistant reply links to.
//!
//! Agents hand visual proof back as Markdown links to files they wrote under
//! temp directories (`[Mockup](/tmp/shots/f1.png)`). Those files are routinely
//! cleaned up, which left saved replies pointing at nothing. When a top-level
//! reply completes, each linked local image gets a copy in the thread's
//! chat-image folder, recorded on the reply as `retained_images` so live and
//! replayed transcripts show the copy and fall back to the original path.
//!
//! The event bridge only scans the text and names the destinations, and
//! stores the reply together with a copy job. The copies run afterwards on a
//! background task, which records the destinations that actually landed on
//! the reply's row and retires the job. A job a quit cut short resumes on
//! the next launch; a fork inherits the jobs of replies it copies.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use super::agent_chat::{
    chat_image_extension, chat_images_dir, chat_images_root, media_type_from_extension,
    read_bounded_local_image, resolve_within, write_thread_chat_image,
};
use crate::database::DatabaseStore;

/// Announces the copies a reply job recorded (their paths), so a card that
/// gave up while its copy was still landing can load it.
pub const REPLY_IMAGES_KEPT_EVENT: &str = "agent_chat_reply_images_kept";

/// Most images one reply keeps.
const MAX_RETAINED_PER_REPLY: usize = 8;
/// Most bytes one reply keeps, across all of its images.
const MAX_RETAINED_BYTES_PER_REPLY: usize = 64 * 1024 * 1024;
/// Replies copying at once. Each holds at most one image (≤ 25 MB) in memory.
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

/// Name a copy for each local image `text` links to. No filesystem access:
/// this runs on the event bridge. Links into the chat-image folder are kept
/// too: the reply then owns its copy, so a fork carries it along.
pub(super) fn plan(thread_id: &str, text: &str) -> Vec<RetainedReplyImage> {
    let Some(dir) = chat_images_dir(thread_id) else {
        return Vec::new();
    };
    local_image_sources(text)
        .into_iter()
        .map(|source| {
            let media_type = media_type_from_extension(std::path::Path::new(&source));
            let name = format!(
                "{}.{}",
                uuid::Uuid::new_v4(),
                chat_image_extension(&media_type)
            );
            RetainedReplyImage {
                path: dir.join(name).to_string_lossy().into_owned(),
                source,
            }
        })
        .collect()
}

/// Copy reply `row_id`'s planned images in the background, then record the
/// copies that landed on its row and retire its job.
pub(super) fn spawn_copies<R: Runtime>(app: &AppHandle<R>, row_id: i64) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { run_job(&app, row_id).await });
}

/// Resume every reply whose copies were not recorded yet: those cut short
/// by a quit, and a fork's inherited replies that were still copying.
pub fn resume_pending<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    for row_id in db.list_reply_image_jobs(None) {
        spawn_copies(app, row_id);
    }
}

/// Finish `thread_id`'s pending copies before it is forked, so the fork's
/// file copy finds every kept image the source will have.
pub(super) async fn settle_thread<R: Runtime>(app: &AppHandle<R>, thread_id: &str) {
    let rows = app.state::<DatabaseStore>().list_reply_image_jobs(Some(thread_id));
    for row_id in rows {
        run_job(app, row_id).await;
    }
}

/// Drop `thread_id`'s pending copies before it is deleted, and wait out any
/// copy in progress, so none lands after the chat's images are removed.
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

/// Make sure `image.path` holds a copy of `image.source`, charging its size
/// to the reply's `budget` (a copy an earlier run left counts too).
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
    let (folder, stem, extension) =
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
    let read = read_bounded_local_image(&image.source).await?;
    if chat_image_extension(&read.media_type) != extension {
        return Err(format!("its content is {}, not .{extension}", read.media_type));
    }
    if read.bytes.len() > *budget {
        return Err("this reply's image budget is spent".to_string());
    }
    *budget -= read.bytes.len();
    write_thread_chat_image(&folder, &stem, &read.bytes).await?;
    Ok(())
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

/// The local image paths `markdown` links to, in order and deduplicated, as
/// the transcript renders them: inline links, images and autolinks whose
/// destination is an absolute path or a local `file:` URL ending in a
/// supported image extension. Code is not scanned for links. Reference-style
/// links are skipped because the transcript leaves them as text (it renders
/// each block on its own, so a definition never reaches its use). Mirrors
/// `localImagePath` in `src/lib/agent-chat/local-image-links.ts`.
pub(super) fn local_image_sources(markdown: &str) -> Vec<String> {
    use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};

    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    let mut sources: Vec<String> = Vec::new();
    for event in Parser::new_ext(markdown, options) {
        let (Event::Start(Tag::Link { link_type, dest_url, .. })
        | Event::Start(Tag::Image { link_type, dest_url, .. })) = event
        else {
            continue;
        };
        if !matches!(link_type, LinkType::Inline | LinkType::Autolink) {
            continue;
        }
        let Some(path) = local_image_path(&dest_url) else {
            continue;
        };
        if !sources.contains(&path) {
            sources.push(path);
            if sources.len() == MAX_RETAINED_PER_REPLY {
                break;
            }
        }
    }
    sources
}

/// The native absolute path a Markdown destination names, when it is a
/// supported local image. Relative paths have no stable base in a chat.
fn local_image_path(destination: &str) -> Option<String> {
    let is_file_url = destination
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"));
    let path = if is_file_url {
        let url = url::Url::parse(destination).ok()?;
        if url.scheme() != "file" || url.host_str().is_some_and(|host| host != "localhost") {
            return None;
        }
        let path = percent_decode(url.path(), "");
        // file:///C:/foo.png is the standard Windows file-URL spelling.
        let bytes = path.as_bytes();
        if bytes.len() > 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' && bytes[3] == b'/' {
            path[1..].to_string()
        } else {
            path
        }
    } else {
        // `decodeURI` leaves reserved characters encoded.
        percent_decode(destination, ";/?:@&=+$,#")
    };
    let bytes = path.as_bytes();
    let absolute = path.starts_with('/')
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    let extension = std::path::Path::new(&path)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    let supported = matches!(
        extension.as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp")
    );
    (absolute && supported).then_some(path)
}

/// Decode `%XX` escapes except those producing a character in `keep`. Input
/// that does not decode to UTF-8 is returned unchanged, as `decodeURI`
/// callers in the frontend fall back to the raw text.
fn percent_decode(input: &str, keep: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(value) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                if !(value.is_ascii() && keep.as_bytes().contains(&value)) {
                    out.push(value);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_linked_and_embedded_local_images_once() {
        let text = "Here: [Mockup](/tmp/wf/f1.png) and ![alt](/tmp/wf/f2.JPG \"title\").\n\
                    Again [same](/tmp/wf/f1.png), [spaced](</tmp/my shots/a.webp>), \
                    [url](file:///tmp/wf/f3.gif), [enc](/tmp/wf/caf%C3%A9.png), \
                    [win](C:\\shots\\w.png)";
        assert_eq!(
            local_image_sources(text),
            [
                "/tmp/wf/f1.png",
                "/tmp/wf/f2.JPG",
                "/tmp/my shots/a.webp",
                "/tmp/wf/f3.gif",
                "/tmp/wf/café.png",
                "C:\\shots\\w.png",
            ]
        );
    }

    #[test]
    fn skips_code_relative_remote_and_non_image_links() {
        let text = "```md\n[fenced](/tmp/a.png)\n```\n\
                    ~~~\n![tilde](/tmp/b.png)\n~~~\n\
                    Inline `[code](/tmp/c.png)` and ``x `[d](/tmp/d.png)` y``.\n\
                    [rel](shots/e.png) [web](https://example.com/f.png) \
                    [host](file://server/g.png) [doc](/tmp/h.pdf) [dir](/tmp/)\n\
                    [kept](/tmp/i.png)";
        assert_eq!(local_image_sources(text), ["/tmp/i.png"]);
    }

    #[test]
    fn an_unclosed_fence_hides_the_rest() {
        assert!(local_image_sources("```\n[a](/tmp/a.png)").is_empty());
    }

    #[test]
    fn caps_images_per_reply() {
        let text: String = (0..20).map(|i| format!("[{i}](/tmp/{i}.png) ")).collect();
        assert_eq!(local_image_sources(&text).len(), MAX_RETAINED_PER_REPLY);
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

    #[test]
    fn multibyte_destinations_never_split_a_character() {
        assert_eq!(local_image_sources("[x](/中文.png) [y](é.png) [z](中文)"), ["/中文.png"]);
    }

    #[test]
    fn only_rendered_links_count() {
        let text = "Not a link: ](/tmp/a.png) and \\[b](/tmp/b.png).\n\
                    > ```\n> [quoted](/tmp/c.png)\n> ```\n\
                    Runs `` ` `` ``` and `[d](/tmp/d.png)` then [e](/tmp/e.png)";
        assert_eq!(local_image_sources(text), ["/tmp/e.png"]);
    }

    #[test]
    fn a_fence_opened_in_a_list_item_hides_only_its_own_lines() {
        let text = "- ```markdown\n  [Example](/tmp/example.png)\n  ```\n\n[Actual proof](/tmp/actual.png)";
        assert_eq!(local_image_sources(text), ["/tmp/actual.png"]);
    }

    #[test]
    fn destinations_on_their_own_line_count() {
        assert_eq!(
            local_image_sources("[Proof](\n/tmp/proof.png\n) and ![Shot](\n/tmp/shot.png\n)"),
            ["/tmp/proof.png", "/tmp/shot.png"]
        );
    }

    #[test]
    fn rendered_forms_count_and_reference_links_do_not() {
        let text = "Shot: <file:///tmp/auto.png>\n\n\
                    | a | b |\n| - | - |\n| [cell](/tmp/cell.png) | x |\n\n\
                    # Title [h](/tmp/h.png)\n\n\
                    See [the mockup][m].\n\n[m]: /tmp/mockup.png\n\n\
                    <a href=\"/tmp/html.png\">h</a>";
        assert_eq!(
            local_image_sources(text),
            ["/tmp/auto.png", "/tmp/cell.png", "/tmp/h.png"]
        );
    }

    #[test]
    fn a_quoted_fence_line_does_not_close_an_outer_fence() {
        let text = "~~~\n> ~~~\n[example](/tmp/a.png)\n~~~\n[proof](/tmp/proof.png)";
        assert_eq!(local_image_sources(text), ["/tmp/proof.png"]);
    }

    #[test]
    fn bare_ampersands_stay_linear() {
        let text = format!("[a](/tmp/{}.png)", "&".repeat(200_000));
        let started = std::time::Instant::now();
        assert_eq!(local_image_sources(&text).len(), 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[test]
    fn a_fence_ends_with_the_quote_that_holds_it() {
        let text = "> ~~~\n> [example](/tmp/a.png)\n\n[proof](/tmp/proof.png)";
        assert_eq!(local_image_sources(text), ["/tmp/proof.png"]);
    }

    #[test]
    fn destinations_resolve_character_references_like_the_renderer() {
        assert_eq!(
            local_image_sources(
                "[a](/tmp/a&amp;b.png) [c](/tmp/c&#x41;.png) [d](/tmp/&copy;.png) [e](/tmp/e&f.png)"
            ),
            ["/tmp/a&b.png", "/tmp/cA.png", "/tmp/©.png", "/tmp/e&f.png"]
        );
    }

    #[test]
    fn code_span_stripping_stays_linear_on_unmatched_runs() {
        let line: String = (1..400).map(|n| "`".repeat(n) + " x ").collect();
        let started = std::time::Instant::now();
        assert!(local_image_sources(&format!("{line}[a](/tmp/a.png)")).len() == 1);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
    }

    #[tokio::test]
    async fn a_resumed_reply_charges_copies_already_landed_to_its_budget() {
        let root = tempfile::tempdir().unwrap();
        let canonical = root.path().canonicalize().unwrap();
        let folder = root.path().join("t");
        std::fs::create_dir_all(&folder).unwrap();
        // A copy the first run landed before the quit.
        let landed = std::fs::File::create(folder.join("landed.png")).unwrap();
        landed.set_len((MAX_RETAINED_BYTES_PER_REPLY - 1024 * 1024) as u64).unwrap();
        // A source the resumed run has not copied yet, larger than what is left.
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
        assert!(refused.unwrap_err().contains("budget"));
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

    #[test]
    fn destinations_unescape_and_balance_parentheses() {
        assert_eq!(
            local_image_sources(r"[a](/tmp/x\(1\).png) [b](/tmp/y(2).png)"),
            ["/tmp/x(1).png", "/tmp/y(2).png"]
        );
    }
}
