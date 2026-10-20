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

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

use super::agent_chat::{
    chat_image_extension, chat_images_dir, chat_images_root, media_type_from_extension,
    read_bounded_local_image, write_thread_chat_image,
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
/// this runs on the event bridge. Images already in the chat-image folder
/// (a re-linked attachment) are owned already and are skipped.
pub(super) fn plan(thread_id: &str, text: &str) -> Vec<RetainedReplyImage> {
    let (Some(root), Some(dir)) = (chat_images_root(), chat_images_dir(thread_id)) else {
        return Vec::new();
    };
    local_image_sources(text)
        .into_iter()
        .filter(|source| !std::path::Path::new(source).starts_with(&root))
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
/// copies that landed on its row and retire its job. Each run copies only
/// what is still missing, so a resumed or repeated job is harmless; one row
/// never runs twice at once.
pub(super) fn spawn_copies<R: Runtime>(app: &AppHandle<R>, row_id: i64) {
    if !in_flight().lock().unwrap().insert(row_id) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        retain_row(&app, row_id).await;
        in_flight().lock().unwrap().remove(&row_id);
    });
}

/// Resume every reply whose copies were not recorded yet: those cut short
/// by a quit, and a fork's inherited replies that were still copying.
pub fn resume_pending<R: Runtime>(app: &AppHandle<R>) {
    let Some(db) = app.try_state::<DatabaseStore>() else {
        return;
    };
    for row_id in db.list_reply_image_jobs() {
        spawn_copies(app, row_id);
    }
}

fn in_flight() -> &'static std::sync::Mutex<std::collections::HashSet<i64>> {
    static ROWS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
        std::sync::OnceLock::new();
    ROWS.get_or_init(Default::default)
}

async fn retain_row<R: Runtime>(app: &AppHandle<R>, row_id: i64) {
    let db = app.state::<DatabaseStore>();
    // A row that is gone took its job with it.
    let Some(payload) = db.get_agent_chat_message_payload(row_id) else {
        return;
    };
    let planned = planned_in_payload(&payload);
    let Ok(_permit) = COPY_PERMITS.acquire().await else {
        return;
    };
    let Some(root) = chat_images_root() else {
        return;
    };
    let mut budget = MAX_RETAINED_BYTES_PER_REPLY;
    let mut kept = Vec::new();
    for image in planned {
        match keep_one(&root, &image, &mut budget).await {
            Ok(()) => kept.push(image),
            Err(error) => eprintln!(
                "[codemux::reply_images] not keeping {}: {error}",
                image.source
            ),
        }
    }
    match db.finish_reply_image_job(row_id, &kept) {
        Ok(true) => {}
        Ok(false) => remove_copies(&kept),
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

/// Make sure `image.path` holds a copy of `image.source`. The destination
/// comes from a stored row, so it must be exactly
/// `<root>/<folder>/<stem>.<ext>`; the copy lands at that path or not at
/// all, so a live reply and the stored row always name the same file.
async fn keep_one(
    root: &std::path::Path,
    image: &RetainedReplyImage,
    budget: &mut usize,
) -> Result<(), String> {
    let (folder, stem, extension) =
        copy_target(root, &image.path).ok_or_else(|| "not a chat-image copy path".to_string())?;
    if tokio::fs::try_exists(&image.path).await.unwrap_or(false) {
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

fn remove_copies(kept: &[RetainedReplyImage]) {
    for image in kept {
        let path = std::path::Path::new(&image.path);
        let _ = std::fs::remove_file(path);
        // Fails, harmlessly, while the folder still holds files.
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir(dir);
        }
    }
}

/// The local image paths `markdown` links to, in order and deduplicated, as
/// the transcript renders them: inline links and images whose destination
/// is an absolute path or a local `file:` URL ending in a supported image
/// extension. Fenced code and code spans are examples, not links, and are
/// skipped. Mirrors `localImagePath` in `src/lib/agent-chat/local-image-links.ts`.
pub(super) fn local_image_sources(markdown: &str) -> Vec<String> {
    let mut sources: Vec<String> = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    for line in markdown.lines() {
        let trimmed = line.trim_start_matches(' ');
        if line.len() - trimmed.len() <= 3 {
            if let Some(marker) = trimmed.chars().next().filter(|c| *c == '`' || *c == '~') {
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

/// `line` with every closed code span blanked out.
fn without_code_spans(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '`' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let run = chars[i..].iter().take_while(|c| **c == '`').count();
        let mut close = None;
        let mut j = i + run;
        while j < chars.len() {
            if chars[j] == '`' {
                let other = chars[j..].iter().take_while(|c| **c == '`').count();
                if other == run {
                    close = Some(j + other);
                    break;
                }
                j += other;
            } else {
                j += 1;
            }
        }
        match close {
            Some(end) => {
                out.push(' ');
                i = end;
            }
            None => {
                out.extend(&chars[i..i + run]);
                i += run;
            }
        }
    }
    out
}

/// Destinations of the inline links (`[..](dest)`) and images in `line`,
/// with Markdown backslash escapes removed.
fn link_destinations(line: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = line;
    while let Some(at) = rest.find("](") {
        let after = rest[at + 2..].trim_start_matches([' ', '\t']);
        let (destination, consumed) = parse_destination(after);
        if let Some(destination) = destination {
            found.push(destination);
        }
        rest = &after[consumed..];
    }
    found
}

/// Parse one link destination, returning it (when well formed) and how many
/// bytes of `input` it spans.
fn parse_destination(input: &str) -> (Option<String>, usize) {
    let mut out = String::new();
    let mut chars = input.char_indices().peekable();
    if input.starts_with('<') {
        chars.next();
        while let Some((at, c)) = chars.next() {
            match c {
                '>' => return (Some(out), at + 1),
                '<' | '\n' => return (None, at),
                '\\' if chars.peek().is_some_and(|(_, next)| next.is_ascii_punctuation()) => {
                    out.push(chars.next().map(|(_, next)| next).unwrap_or('\\'));
                }
                _ => out.push(c),
            }
        }
        return (None, input.len());
    }
    let mut depth = 0usize;
    while let Some((at, c)) = chars.next() {
        match c {
            c if c.is_ascii_whitespace() || c.is_ascii_control() => {
                return (Some(out).filter(|_| depth == 0), at)
            }
            '(' => {
                depth += 1;
                out.push(c);
            }
            ')' if depth == 0 => return (Some(out), at),
            ')' => {
                depth -= 1;
                out.push(c);
            }
            '\\' if chars.peek().is_some_and(|(_, next)| next.is_ascii_punctuation()) => {
                out.push(chars.next().map(|(_, next)| next).unwrap_or('\\'));
            }
            _ => out.push(c),
        }
    }
    (None, input.len())
}

/// The native absolute path a Markdown destination names, when it is a
/// supported local image. Relative paths have no stable base in a chat.
fn local_image_path(destination: &str) -> Option<String> {
    let path = if destination.len() >= 5 && destination[..5].eq_ignore_ascii_case("file:") {
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
        assert_eq!(db.list_reply_image_jobs(), vec![row]);
        let stored = db.get_agent_chat_message_payload(row).unwrap();
        assert_eq!(planned_in_payload(&stored).len(), 2);

        // One copy landed: the row names only it, and the job is retired.
        assert!(db.finish_reply_image_job(row, &[planned("/img/t/l.png")]).unwrap());
        let item = |db: &DatabaseStore| {
            let rows = db.list_agent_chat_messages("t");
            serde_json::from_str::<serde_json::Value>(&rows[0]).unwrap()["item"].clone()
        };
        assert_eq!(item(&db)["retained_images"], serde_json::json!([planned("/img/t/l.png")]));
        assert!(db.list_reply_image_jobs().is_empty());
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
        assert!(db.list_reply_image_jobs().is_empty());
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
    fn destinations_unescape_and_balance_parentheses() {
        assert_eq!(
            local_image_sources(r"[a](/tmp/x\(1\).png) [b](/tmp/y(2).png)"),
            ["/tmp/x(1).png", "/tmp/y(2).png"]
        );
    }
}
