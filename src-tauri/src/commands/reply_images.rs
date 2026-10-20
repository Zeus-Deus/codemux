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
use tauri::{AppHandle, Manager, Runtime};

use super::agent_chat::{
    chat_image_extension, chat_images_dir, chat_images_root, media_type_from_extension,
    read_bounded_local_image, resolve_within, write_thread_chat_image,
};
use crate::database::DatabaseStore;

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
        drop(row_lock(row_id).lock().await);
        forget_row_lock(row_id);
    }
}

/// One job at a time per row: a resumed, inherited or settled run waits for
/// the one in progress and then finds the job retired.
fn row_lock(row_id: i64) -> Arc<tokio::sync::Mutex<()>> {
    row_locks().lock().unwrap().entry(row_id).or_default().clone()
}

fn forget_row_lock(row_id: i64) {
    row_locks().lock().unwrap().remove(&row_id);
}

fn row_locks() -> &'static Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

async fn run_job<R: Runtime>(app: &AppHandle<R>, row_id: i64) {
    let lock = row_lock(row_id);
    let _guard = lock.lock().await;
    // Taken before the row is read, so queued jobs hold no payloads.
    let Ok(_permit) = COPY_PERMITS.acquire().await else {
        return;
    };
    let db = app.state::<DatabaseStore>();
    // Retired by another run, or abandoned with its chat (a gone row took
    // its job with it).
    if db.has_reply_image_job(row_id) {
        if let Some(payload) = db.get_agent_chat_message_payload(row_id) {
            retain_row(&db, row_id, planned_in_payload(&payload)).await;
        }
    }
    forget_row_lock(row_id);
}

async fn retain_row(db: &DatabaseStore, row_id: i64, planned: Vec<RetainedReplyImage>) {
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
/// the transcript renders them: inline links and images whose destination
/// is an absolute path or a local `file:` URL ending in a supported image
/// extension. Fenced code (also inside block quotes) and code spans are
/// examples, not links, and are skipped. Mirrors `localImagePath` in
/// `src/lib/agent-chat/local-image-links.ts`.
pub(super) fn local_image_sources(markdown: &str) -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in markdown.lines() {
        let body = without_quote_markers(line);
        let trimmed = body.trim_start_matches(' ');
        if body.len() - trimmed.len() <= 3 {
            if let Some(marker) = trimmed.chars().next().filter(|c| *c == '`' || *c == '~') {
                // Fence markers are ASCII, so the run length is a byte offset.
                let run = trimmed.chars().take_while(|c| *c == marker).count();
                if run >= 3 {
                    match fence {
                        None => {
                            fence = Some((marker, run));
                            continue;
                        }
                        Some((open, len))
                            if open == marker && run >= len && trimmed[run..].trim().is_empty() =>
                        {
                            fence = None;
                            continue;
                        }
                        Some(_) => {}
                    }
                }
            }
        }
        if fence.is_some() {
            continue;
        }
        for destination in link_destinations(&without_code_spans(line)) {
            let Some(path) = local_image_path(&destination) else {
                continue;
            };
            if !sources.contains(&path) {
                sources.push(path);
                if sources.len() == MAX_RETAINED_PER_REPLY {
                    return sources;
                }
            }
        }
    }
    sources
}

/// `line` without its block-quote markers (`> > text` → `text`).
fn without_quote_markers(line: &str) -> &str {
    let mut rest = line;
    loop {
        let trimmed = rest.trim_start_matches(' ');
        if rest.len() - trimmed.len() > 3 {
            return rest;
        }
        match trimmed.strip_prefix('>') {
            Some(after) => rest = after.strip_prefix(' ').unwrap_or(after),
            None => return rest,
        }
    }
}

/// `line` with every closed code span blanked out. Linear: each backtick
/// run looks ahead only through the later runs of its own length.
fn without_code_spans(line: &str) -> String {
    let bytes = line.as_bytes();
    let mut runs = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let start = i;
            while i < bytes.len() && bytes[i] == b'`' {
                i += 1;
            }
            runs.push((start, i));
        } else {
            i += 1;
        }
    }
    let mut by_length: HashMap<usize, (Vec<usize>, usize)> = HashMap::new();
    for (index, (start, end)) in runs.iter().enumerate() {
        by_length.entry(end - start).or_default().0.push(index);
    }
    let mut out = String::with_capacity(line.len());
    let mut copied = 0;
    let mut index = 0;
    while index < runs.len() {
        let (start, end) = runs[index];
        let (same, cursor) = by_length.get_mut(&(end - start)).expect("every run is indexed");
        while *cursor < same.len() && same[*cursor] <= index {
            *cursor += 1;
        }
        match same.get(*cursor) {
            Some(&close) => {
                out.push_str(&line[copied..start]);
                out.push(' ');
                copied = runs[close].1;
                index = close + 1;
            }
            None => index += 1,
        }
    }
    out.push_str(&line[copied..]);
    out
}

/// Destinations of the inline links (`[..](dest)`) and images in `line`.
/// Only a `](` closing an unescaped `[` counts.
fn link_destinations(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut open = 0usize;
    let mut chars = line.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            '[' => open += 1,
            ']' if open > 0 => {
                open -= 1;
                if line[at + 1..].starts_with('(') {
                    let after = line[at + 2..].trim_start_matches([' ', '\t']);
                    let (destination, consumed) = parse_destination(after);
                    if let Some(destination) = destination {
                        found.push(destination);
                    }
                    let resume = line.len() - after.len() + consumed;
                    while chars.peek().is_some_and(|(next, _)| *next < resume) {
                        chars.next();
                    }
                }
            }
            _ => {}
        }
    }
    found
}

/// Parse one link destination, returning it (when well formed, with
/// backslash escapes and character references resolved) and how many bytes
/// of `input` it spans.
fn parse_destination(input: &str) -> (Option<String>, usize) {
    let mut out = String::new();
    let mut chars = input.char_indices().peekable();
    let angled = input.starts_with('<');
    if angled {
        chars.next();
    }
    let mut depth = 0usize;
    while let Some((at, c)) = chars.next() {
        match c {
            '>' if angled => return (Some(out), at + 1),
            '<' | '\n' if angled => return (None, at),
            c if !angled && (c.is_ascii_whitespace() || c.is_ascii_control()) => {
                return (Some(out).filter(|_| depth == 0), at)
            }
            '(' if !angled => {
                depth += 1;
                out.push(c);
            }
            ')' if !angled && depth == 0 => return (Some(out), at),
            ')' if !angled => {
                depth -= 1;
                out.push(c);
            }
            '\\' if chars.peek().is_some_and(|(_, next)| next.is_ascii_punctuation()) => {
                out.push(chars.next().map(|(_, next)| next).unwrap_or('\\'));
            }
            '&' => match character_reference(&input[at..]) {
                Some((decoded, len)) => {
                    out.push(decoded);
                    while chars.peek().is_some_and(|(next, _)| *next < at + len) {
                        chars.next();
                    }
                }
                None => out.push('&'),
            },
            _ => out.push(c),
        }
    }
    (None, input.len())
}

/// Decode a character reference at the start of `input` (`&amp;`, `&#38;`,
/// `&#x26;`), returning the character and the reference's byte length.
/// Named references beyond the five XML ones stay as written.
fn character_reference(input: &str) -> Option<(char, usize)> {
    let end = 1 + input[1..]
        .char_indices()
        .take(32)
        .find(|(_, c)| *c == ';')?
        .0;
    let name = &input[1..end];
    let decoded = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let number = name.strip_prefix('#')?;
            let code = match number.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => number.parse().ok()?,
            };
            char::from_u32(code).filter(|c| *c != '\0')?
        }
    };
    Some((decoded, end + 1))
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
    fn destinations_resolve_character_references_like_the_renderer() {
        assert_eq!(
            local_image_sources("[a](/tmp/a&amp;b.png) [c](/tmp/c&#x41;.png) [d](/tmp/&bogus;.png)"),
            ["/tmp/a&b.png", "/tmp/cA.png", "/tmp/&bogus;.png"]
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
