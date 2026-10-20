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

/// The local image paths `markdown` links to, in order and deduplicated:
/// link and image destinations that are an absolute path or a local `file:`
/// URL ending in a supported image extension, as the transcript turns them
/// into image cards (`src/lib/agent-chat/local-image-links.ts`). Code is not
/// scanned, an image's alt text is not, and a footnote counts only once it
/// is referenced. Reference-style links are kept too, though the transcript
/// can leave them as text: an unused copy is cheap, a missing one is the
/// loss this module exists to prevent. They only take slots that links
/// known to render leave free.
pub(super) fn local_image_sources(markdown: &str) -> Vec<String> {
    use pulldown_cmark::{Event, LinkType, Options, Parser, Tag, TagEnd};
    use unicase::UniCase;

    let markdown = bounded_for_scan(markdown);
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_FOOTNOTES;
    // Candidates, in order and deduplicated: links the transcript always
    // renders; reference links it may leave as text; and links inside
    // footnotes, which render only once the footnote is referenced. Each
    // holds a part of the reply at most once, so they stay linear in it.
    let mut rendered = Candidates::default();
    let mut referenced_links = Candidates::default();
    let mut in_footnotes: Candidates<(UniCase<String>, bool, String)> = Candidates::default();
    // Footnote labels match like the parser matches them: case-folded.
    let mut referenced = std::collections::HashSet::new();
    let mut footnote: Option<UniCase<String>> = None;
    let mut inside_image = 0usize;
    for event in Parser::new_ext(&markdown, options) {
        let (link_type, dest_url) = match event {
            Event::FootnoteReference(label) => {
                referenced.insert(UniCase::new(label.to_string()));
                continue;
            }
            Event::Start(Tag::FootnoteDefinition(label)) => {
                footnote = Some(UniCase::new(label.to_string()));
                continue;
            }
            Event::End(TagEnd::FootnoteDefinition) => {
                footnote = None;
                continue;
            }
            Event::Start(Tag::Image { link_type, dest_url, .. }) => {
                inside_image += 1;
                // An image inside another's description is only alt text.
                if inside_image > 1 {
                    continue;
                }
                (link_type, dest_url)
            }
            Event::End(TagEnd::Image) => {
                inside_image = inside_image.saturating_sub(1);
                continue;
            }
            Event::Start(Tag::Link { link_type, dest_url, .. }) if inside_image == 0 => {
                (link_type, dest_url)
            }
            _ => continue,
        };
        if link_type == LinkType::Email {
            continue;
        }
        let renders = matches!(link_type, LinkType::Inline | LinkType::Autolink);
        let Some(path) = local_image_path(&dest_url) else {
            continue;
        };
        match &footnote {
            Some(label) => in_footnotes.add((label.clone(), renders, path)),
            None if renders => rendered.add(path),
            None => referenced_links.add(path),
        }
    }
    // Links known to render come first; reference links only take slots
    // those leave free, so a speculative copy never displaces a visible one.
    let (noted_rendering, noted_references): (Vec<_>, Vec<_>) = in_footnotes
        .order
        .into_iter()
        .filter(|(label, _, _)| referenced.contains(label))
        .partition(|(_, renders, _)| *renders);
    let mut sources: Vec<String> = Vec::new();
    let ordered = rendered
        .order
        .into_iter()
        .chain(noted_rendering.into_iter().map(|(_, _, path)| path))
        .chain(referenced_links.order)
        .chain(noted_references.into_iter().map(|(_, _, path)| path));
    for path in ordered {
        if !sources.contains(&path) {
            sources.push(path);
            if sources.len() == MAX_RETAINED_PER_REPLY {
                break;
            }
        }
    }
    sources
}

/// Most leading list, block-quote and footnote markers a line keeps for the
/// scan.
const MAX_LINE_MARKERS: usize = 32;
/// Most leading indentation, in columns, a line keeps for the scan.
const MAX_LINE_INDENT: usize = 64;

/// Distinct values in the order first seen.
struct Candidates<T> {
    order: Vec<T>,
    seen: std::collections::HashSet<T>,
}

impl<T> Default for Candidates<T> {
    fn default() -> Self {
        Self { order: Vec::new(), seen: std::collections::HashSet::new() }
    }
}

impl<T: Clone + Eq + std::hash::Hash> Candidates<T> {
    fn add(&mut self, value: T) {
        if self.seen.insert(value.clone()) {
            self.order.push(value);
        }
    }
}

/// `markdown` with `\n` line endings and every line's leading container
/// markers and indentation cut to the bounds above.
///
/// The parser walks every open list, block quote and footnote on every line,
/// so thousands of nested containers held open across blank lines make it
/// quadratic. A container opens only on a line that shows it: through its
/// own marker, or through indentation deep enough to nest. With both bounded
/// per line, so is the nesting, and the scan stays linear. Lines within the
/// bounds (all ordinary text and code) keep their content; a cut line only
/// loses nesting, and code is never scanned for links anyway. Bare `\r`
/// endings become `\n`, as the transcript reads them: the parser can let a
/// line after indented code run on across a bare `\r`.
fn bounded_for_scan(markdown: &str) -> std::borrow::Cow<'_, str> {
    let within = |line: &str| {
        let prefix = line_prefix(line);
        prefix.markers <= MAX_LINE_MARKERS && prefix.indent <= MAX_LINE_INDENT
    };
    if !markdown.contains('\r') && markdown_lines(markdown).all(|(line, _)| within(line)) {
        return std::borrow::Cow::Borrowed(markdown);
    }
    let mut out = String::with_capacity(markdown.len());
    for (line, ending) in markdown_lines(markdown) {
        if within(line) {
            out.push_str(line);
        } else {
            // Same order as written: indentation and markers interleave, so
            // a quoted code line stays in its quote. Indentation past the
            // budget and markers past the limit are left out.
            let prefix = line_prefix(line);
            let mut indent_left = MAX_LINE_INDENT;
            for part in &prefix.parts {
                match *part {
                    PrefixPart::Indent(columns) => {
                        let kept = columns.min(indent_left);
                        indent_left -= kept;
                        out.extend(std::iter::repeat(' ').take(kept));
                    }
                    PrefixPart::Marker(marker, ordinal) if ordinal < MAX_LINE_MARKERS => {
                        out.push_str(marker);
                        out.push(' ');
                    }
                    PrefixPart::Marker(..) => {}
                }
            }
            out.push_str(&line[prefix.content..]);
        }
        if !ending.is_empty() {
            out.push('\n');
        }
    }
    std::borrow::Cow::Owned(out)
}

/// Where the label of a footnote definition ends: the byte offset of the
/// first unescaped `]` in `label`, if a `:` follows it. A backslash escapes
/// the next character, as in the parser.
fn footnote_label_end(label: &str) -> Option<usize> {
    let mut chars = label.char_indices();
    while let Some((at, c)) = chars.next() {
        match c {
            '\\' => {
                chars.next();
            }
            ']' => return label[at + 1..].starts_with(':').then_some(at),
            _ => {}
        }
    }
    None
}

/// `markdown`'s lines with their endings, split at `\n`, `\r\n` and a bare
/// `\r`, as the parser splits them.
fn markdown_lines(markdown: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut rest = markdown;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let (line, ending, next) = match rest.find(['\n', '\r']) {
            Some(at) if rest[at..].starts_with("\r\n") => (&rest[..at], &rest[at..at + 2], at + 2),
            Some(at) => (&rest[..at], &rest[at..at + 1], at + 1),
            None => (rest, "", rest.len()),
        };
        rest = &rest[next..];
        Some((line, ending))
    })
}

/// A line's leading container markers (`>`, list markers, `[^label]:`) and
/// the indentation around them in columns (a marker's own following space
/// excluded), in order, and the byte offset where its content starts.
struct LinePrefix<'a> {
    parts: Vec<PrefixPart<'a>>,
    markers: usize,
    indent: usize,
    content: usize,
}

/// Indentation in columns, or a marker with its position among the line's
/// markers.
enum PrefixPart<'a> {
    Indent(usize),
    Marker(&'a str, usize),
}

fn line_prefix(line: &str) -> LinePrefix<'_> {
    let mut parts = Vec::new();
    let mut markers = 0;
    let mut indent = 0;
    let mut at = 0;
    loop {
        let rest = &line[at..];
        let trimmed = rest.trim_start_matches([' ', '\t']);
        let columns = rest[..rest.len() - trimmed.len()]
            .chars()
            .map(|c| if c == '\t' { 4 } else { 1 })
            .sum::<usize>();
        if columns > 0 {
            parts.push(PrefixPart::Indent(columns));
            indent += columns;
        }
        at = line.len() - trimmed.len();
        let marker = if trimmed.starts_with('>') {
            1
        } else if trimmed.starts_with(['-', '*', '+']) {
            1
        } else if let Some(label) = trimmed.strip_prefix("[^") {
            footnote_label_end(label).map_or(0, |end| end + 4)
        } else {
            let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
            if (1..=9).contains(&digits) && trimmed[digits..].starts_with(['.', ')']) {
                digits + 1
            } else {
                0
            }
        };
        let after = &trimmed[marker..];
        let opens = marker > 0
            && (trimmed.starts_with(['>', '['])
                || after.is_empty()
                || after.starts_with([' ', '\t']));
        if !opens {
            return LinePrefix { parts, markers, indent, content: at };
        }
        parts.push(PrefixPart::Marker(&trimmed[..marker], markers));
        markers += 1;
        // The one space after a marker belongs to it, not to indentation.
        at += marker + usize::from(after.starts_with([' ', '\t']));
    }
}

/// The native absolute path a Markdown destination names, when it is a
/// supported local image. Relative paths have no stable base in a chat.
fn local_image_path(destination: &str) -> Option<String> {
    let is_file_url = destination
        .get(..5)
        .is_some_and(|scheme| scheme.eq_ignore_ascii_case("file:"));
    let path = if is_file_url {
        let url = url::Url::parse(&normalize_uri(destination)).ok()?;
        if url.scheme() != "file" || url.host_str().is_some_and(|host| host != "localhost") {
            return None;
        }
        // `decodeURIComponent`: a path that will not decode is no image.
        let path = decode_uri(url.path(), "")?;
        // file:///C:/foo.png is the standard Windows file-URL spelling.
        let bytes = path.as_bytes();
        if bytes.len() > 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' && bytes[3] == b'/' {
            path[1..].to_string()
        } else {
            path
        }
    } else {
        // The transcript normalizes the destination, then `decodeURI`s it
        // (leaving reserved characters encoded), and keeps the normalized
        // form when that fails.
        let normalized = normalize_uri(destination);
        decode_uri(&normalized, ";/?:@&=+$,#").unwrap_or(normalized)
    };
    let bytes = path.as_bytes();
    let absolute = path.starts_with('/')
        || (bytes.len() > 2
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'/' | b'\\'));
    // The same test as the transcript's `/\.(?:png|jpe?g|gif|webp)$/i`.
    let lower = path.to_ascii_lowercase();
    let supported = [".png", ".jpg", ".jpeg", ".gif", ".webp"]
        .iter()
        .any(|extension| lower.ends_with(extension));
    (absolute && supported).then_some(path)
}

/// micromark's `normalizeUri`, which the transcript applies to a link before
/// decoding it: a `%` followed by two ASCII letters or digits stays, other
/// characters outside its URL-safe ASCII set are percent-encoded as UTF-8.
fn normalize_uri(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = String::with_capacity(value.len());
    let mut chars = value.char_indices();
    while let Some((at, c)) = chars.next() {
        let escape = c == '%'
            && bytes.get(at + 1).is_some_and(u8::is_ascii_alphanumeric)
            && bytes.get(at + 2).is_some_and(u8::is_ascii_alphanumeric);
        if escape {
            out.push_str(&value[at..at + 3]);
            chars.next();
            chars.next();
            continue;
        }
        let safe = matches!(c, '!' | '#' | '$' | '&'..=';' | '=' | '?'..='Z' | '_' | 'a'..='z' | '~');
        if safe {
            out.push(c);
        } else {
            let mut buffer = [0; 4];
            for byte in c.encode_utf8(&mut buffer).bytes() {
                out.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    out
}

/// JavaScript's `decodeURI` (or `decodeURIComponent`, with `keep` empty):
/// `%XX` escapes decoded, except those of a character in `keep`, or `None`
/// when any escape is malformed or the result is not UTF-8. The transcript
/// decodes link paths this way, so both sides name a path alike.
fn decode_uri(input: &str, keep: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'%' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let hex = bytes.get(i + 1..i + 3).filter(|hex| hex.iter().all(u8::is_ascii_hexdigit))?;
        let value = u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
        if value.is_ascii() && keep.as_bytes().contains(&value) {
            out.extend_from_slice(&bytes[i..i + 3]);
        } else {
            out.push(value);
        }
        i += 3;
    }
    String::from_utf8(out).ok()
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
    fn rendered_forms_and_reference_links_count_but_raw_html_does_not() {
        let text = "Shot: <file:///tmp/auto.png>\n\n\
                    | a | b |\n| - | - |\n| [cell](/tmp/cell.png) | x |\n\n\
                    # Title [h](/tmp/h.png)\n\n\
                    - [Listed][l]\n\n  [l]: /tmp/listed.png\n\n\
                    <a href=\"/tmp/html.png\">h</a>";
        assert_eq!(
            local_image_sources(text),
            ["/tmp/auto.png", "/tmp/cell.png", "/tmp/h.png", "/tmp/listed.png"]
        );
    }

    #[test]
    fn images_in_alt_text_and_unreferenced_footnotes_take_no_slot() {
        let hidden: String = (0..7).map(|i| format!("![{i}](/tmp/{i}.png) ")).collect();
        let notes: String = (0..8).map(|i| format!("[^n{i}]: [hidden](/tmp/n{i}.png)\n\n")).collect();
        let text = format!(
            "![a {hidden}](/tmp/outer.png)\n\n{notes}See [^used].\n\n\
             [^used]: [Noted](/tmp/noted.png)\n\n[![In link](/tmp/inner.png)](/tmp/linked.png)\n\n\
             [Proof](/tmp/proof.png)"
        );
        assert_eq!(
            local_image_sources(&text),
            // Footnotes render after the body, so their images come last.
            ["/tmp/outer.png", "/tmp/linked.png", "/tmp/inner.png", "/tmp/proof.png", "/tmp/noted.png"]
        );
    }

    #[test]
    fn notes_referenced_after_the_candidate_bound_still_count() {
        // More footnote images than the scan stores, all referenced only at
        // the end: the first stored ones still count.
        let notes: String = (0..70).map(|i| format!("[^n{i}]: [N{i}](/tmp/n{i}.png)\n\n")).collect();
        let references: String = (0..70).map(|i| format!("[^n{i}] ")).collect();
        let text = format!("{notes}[Proof](/tmp/proof.png)\n\nSee {references}.");
        assert_eq!(
            local_image_sources(&text),
            ["/tmp/proof.png", "/tmp/n0.png", "/tmp/n1.png", "/tmp/n2.png", "/tmp/n3.png",
             "/tmp/n4.png", "/tmp/n5.png", "/tmp/n6.png"]
        );
    }

    #[test]
    fn extensions_match_the_transcripts_test() {
        assert_eq!(
            local_image_sources("[a](/tmp/.png) [b](/tmp/a.png/) [c](/tmp/C.JPEG)"),
            ["/tmp/.png", "/tmp/C.JPEG"]
        );
    }

    #[test]
    fn deep_nesting_is_cut_so_the_scan_stays_linear_and_still_finds_links() {
        let n = 10_000;
        for opener in [
            "- ".repeat(n),
            format!("[^n]: {}", "- ".repeat(n)),
            "> ".repeat(n),
            format!("{}- ", " ".repeat(4 * n)),
        ] {
            for ending in ["\n", "\r", "\r\n"] {
                let text = format!("{opener}x{}{ending}[Proof](/tmp/proof.png)", ending.repeat(n));
                let started = std::time::Instant::now();
                assert_eq!(local_image_sources(&text), ["/tmp/proof.png"], "{ending:?}");
                assert!(started.elapsed() < std::time::Duration::from_millis(500), "{ending:?}");
            }
        }
        // Ordinary nesting is left alone.
        let nested = "> - 1. item\n>    - [Proof](/tmp/proof.png)";
        assert!(matches!(bounded_for_scan(nested), std::borrow::Cow::Borrowed(_)));
        assert_eq!(local_image_sources(nested), ["/tmp/proof.png"]);
        let prefix = line_prefix("> - 1. [^a]: item");
        let markers: Vec<_> = prefix
            .parts
            .iter()
            .filter_map(|part| match part {
                PrefixPart::Marker(marker, _) => Some(*marker),
                PrefixPart::Indent(_) => None,
            })
            .collect();
        assert_eq!((markers, prefix.indent), (vec![">", "-", "1.", "[^a]:"], 0));
        assert_eq!(line_prefix("        - item").indent, 8);
        assert_eq!(line_prefix("-x *** 1.5").markers, 0);
    }

    #[test]
    fn code_never_stops_a_reply_from_being_scanned() {
        let code = format!(
            "```text\n{}deeply indented\n{}\n{}- yaml\n```\n\n[Proof](/tmp/proof.png)",
            " ".repeat(4 * MAX_LINE_INDENT),
            "> ".repeat(MAX_LINE_MARKERS + 5),
            " ".repeat(2 * MAX_LINE_INDENT),
        );
        assert_eq!(local_image_sources(&code), ["/tmp/proof.png"]);
        let indented = format!(
            "{}code line\n\n{}\n\n[Proof](/tmp/proof.png)",
            " ".repeat(400),
            "- ".repeat(MAX_LINE_MARKERS + 5)
        );
        assert_eq!(local_image_sources(&indented), ["/tmp/proof.png"]);
    }

    #[test]
    fn a_cut_line_keeps_its_markers_where_they_were() {
        let text = format!(
            "> ```text\n> {}example\n> ```\n>\n> [Proof](/tmp/proof.png)",
            " ".repeat(100)
        );
        assert_eq!(local_image_sources(&text), ["/tmp/proof.png"]);
        assert_eq!(
            bounded_for_scan(&format!("> {}x", " ".repeat(100))),
            format!("> {}x", " ".repeat(MAX_LINE_INDENT))
        );
    }

    #[test]
    fn unused_footnotes_never_crowd_out_a_used_one() {
        let unused: String = (0..70).map(|i| format!("> [^u{i}]: [hidden](/tmp/u{i}.png)\n>\n")).collect();
        let text = format!("> See [^used].\n>\n{unused}> [^used]: [Proof](/tmp/proof.png)");
        assert_eq!(local_image_sources(&text), ["/tmp/proof.png"]);
    }

    #[test]
    fn footnotes_match_like_the_parser_and_unused_ones_crowd_out_nothing() {
        assert_eq!(
            local_image_sources("> See [^SS].\n>\n> [^ß]: [Proof](/tmp/proof.png)"),
            ["/tmp/proof.png"]
        );
        let unused: String = (0..70).map(|i| format!("> [^u{i}]: [hidden](/tmp/u{i}.png)\n>\n")).collect();
        assert_eq!(
            local_image_sources(&format!("{unused}> [Proof](/tmp/proof.png)")),
            ["/tmp/proof.png"]
        );
    }

    #[test]
    fn paths_decode_like_the_transcripts_decode_uri() {
        assert_eq!(
            local_image_sources(
                "[a](/tmp/a%zz%20b.png) [b](/tmp/c%2Fd%20e.png) [c](/tmp/f%+1.png) \
                 [d](file:///tmp/g%FF.png) [e](file:///tmp/h%zz.png) [f](file:///tmp/i%20j.png)"
            ),
            ["/tmp/a%zz%20b.png", "/tmp/c%2Fd e.png", "/tmp/f%+1.png", "/tmp/i j.png"]
        );
    }

    #[test]
    fn paths_are_normalized_before_decoding_like_the_transcript() {
        assert_eq!(
            local_image_sources(
                "[a](</tmp/a%zz b.png>) [b](/tmp/c%%20d.png) [c](/tmp/e%2%20f.png) [d](</tmp/g%zz é.png>)"
            ),
            ["/tmp/a%zz%20b.png", "/tmp/c% d.png", "/tmp/e%2 f.png", "/tmp/g%zz%20%C3%A9.png"]
        );
        assert_eq!(local_image_sources("[a](<file:///tmp/a.png >)"), Vec::<String>::new());
    }

    #[test]
    fn an_escaped_bracket_does_not_end_a_footnote_label() {
        assert_eq!(footnote_label_end(r"a\]:b]: x"), Some(5));
        assert_eq!(footnote_label_end("a] x"), None);
        let n = 10_000;
        let text = format!(r"[^a\]:b]: {}x{}[Proof](/tmp/proof.png)", "- ".repeat(n), "\n".repeat(n));
        let started = std::time::Instant::now();
        assert_eq!(local_image_sources(&text), ["/tmp/proof.png"]);
        assert!(started.elapsed() < std::time::Duration::from_millis(500));
    }

    #[test]
    fn reference_links_only_take_slots_rendered_links_leave() {
        let uses: String = (0..8).map(|i| format!("[r{i}][r{i}] ")).collect();
        let definitions: String = (0..8).map(|i| format!("[r{i}]: /tmp/r{i}.png\n")).collect();
        let text = format!("See {uses}.\n\n[Proof](/tmp/proof.png)\n\n{definitions}");
        let sources = local_image_sources(&text);
        assert_eq!(sources.len(), MAX_RETAINED_PER_REPLY);
        assert_eq!(sources[0], "/tmp/proof.png");
        assert_eq!(sources[1..], ["/tmp/r0.png", "/tmp/r1.png", "/tmp/r2.png", "/tmp/r3.png",
            "/tmp/r4.png", "/tmp/r5.png", "/tmp/r6.png"]);
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
